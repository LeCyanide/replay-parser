//! Game file access: the client's resource packages, read in place.
//!
//! An install keeps one directory per client build under `bin/<build>/`.
//! Its `idx/*.idx` files each index one package in `res_packages/`; a few
//! files sit loose under `bin/<build>/res/`. `res_mods/` (mods) is ignored, so
//! what is read is what the unmodified client reads.
//!
//! `.idx` layout, all little-endian:
//!
//! ```text
//! magic "ISFP", u32 0x2000000, u32 id, u32 0x40
//! u32 node_count, u32 file_count, u64 1, u64 nodes_offset,
//! u64 files_offset, u64 footer_offset          offsets from 0x10
//! node[node_count]  (32 bytes)  u64 name_len, u64 name_offset (from the
//!                               node), u64 id, u64 parent_id
//! file[file_count]  (48 bytes)  u64 node_id, u64 package_id, u64 offset,
//!                               u32 compression (0 none, 5 raw deflate),
//!                               u32 flags, u32 stored_size, u32 crc32 of
//!                               the unpacked bytes, u64 unpacked_size
//! footer            u64 name_len, u64 name_offset (from the footer; usually
//!                   24, right after it), u64 package_id; the package's
//!                   file name at name_offset
//! ```

use std::collections::BTreeMap;
use std::collections::HashMap;
use std::io::Error;
use std::io::ErrorKind;
use std::io::Read;
use std::io::Result;
use std::io::Seek;
use std::io::SeekFrom;
use std::path::Path;
use std::path::PathBuf;

use crate::defs::ScriptSource;

#[derive(Clone, Debug)]
struct Entry {
    package: usize,
    offset: u64,
    compression: u32,
    stored: u32,
    crc: u32,
    size: u64,
}

/// Every file of one client build, by path (`/`-separated, as the client
/// names them, e.g. `scripts/entity_defs/Avatar.def`).
pub struct Resources {
    build: u32,
    loose: PathBuf,
    packages: Vec<PathBuf>,
    files: BTreeMap<String, Entry>,
}

fn bad(msg: String) -> Error {
    Error::new(ErrorKind::InvalidData, msg)
}

fn u32_at(d: &[u8], at: usize) -> Result<u32> {
    let b = d.get(at..at + 4).ok_or_else(|| bad(format!("index truncated at {at:#x}")))?;
    Ok(u32::from_le_bytes(b.try_into().expect("4 bytes")))
}

fn u64_at(d: &[u8], at: usize) -> Result<u64> {
    let b = d.get(at..at + 8).ok_or_else(|| bad(format!("index truncated at {at:#x}")))?;
    Ok(u64::from_le_bytes(b.try_into().expect("8 bytes")))
}

fn str_at(d: &[u8], at: usize, len: usize) -> Result<String> {
    let b = d.get(at..at + len).ok_or_else(|| bad(format!("index truncated at {at:#x}")))?;
    let b = b.strip_suffix(&[0]).unwrap_or(b);
    String::from_utf8(b.to_vec()).map_err(|_| bad(format!("name at {at:#x} is not UTF-8")))
}

/// Parses one `.idx`: the package file it indexes, and its files by path.
fn parse_index(d: &[u8]) -> Result<(String, Vec<(String, Entry)>)> {
    if d.get(..4) != Some(b"ISFP") {
        return Err(bad("not an ISFP index".into()));
    }
    let nodes = u32_at(d, 0x10)? as usize;
    let files = u32_at(d, 0x14)? as usize;
    let nodes_at = 0x10 + u64_at(d, 0x20)? as usize;
    let files_at = 0x10 + u64_at(d, 0x28)? as usize;
    let footer_at = 0x10 + u64_at(d, 0x30)? as usize;
    if files_at + files * 48 != footer_at {
        return Err(bad(format!("{files} file records do not end at the footer")));
    }

    let mut names: BTreeMap<u64, (String, u64)> = BTreeMap::new();
    for i in 0..nodes {
        let at = nodes_at + i * 32;
        let name = str_at(d, at + u64_at(d, at + 8)? as usize, u64_at(d, at)? as usize)?;
        names.insert(u64_at(d, at + 16)?, (name, u64_at(d, at + 24)?));
    }
    let path = |mut id: u64| -> Result<String> {
        let mut parts = Vec::new();
        while let Some((name, parent)) = names.get(&id) {
            parts.push(name.as_str());
            // A cycle would loop forever; real trees are shallow.
            if parts.len() > 64 {
                return Err(bad("node tree deeper than 64".into()));
            }
            id = *parent;
        }
        parts.reverse();
        Ok(parts.join("/"))
    };

    let package = str_at(d, footer_at + u64_at(d, footer_at + 8)? as usize, u64_at(d, footer_at)? as usize)?;
    let mut out = Vec::with_capacity(files);
    for i in 0..files {
        let at = files_at + i * 48;
        out.push((
            path(u64_at(d, at)?)?,
            Entry {
                package: 0,
                offset: u64_at(d, at + 16)?,
                compression: u32_at(d, at + 24)?,
                stored: u32_at(d, at + 32)?,
                crc: u32_at(d, at + 36)?,
                size: u64_at(d, at + 40)?,
            },
        ));
    }
    Ok((package, out))
}

/// The client builds installed under `game_dir`, oldest first.
pub fn builds(game_dir: &Path) -> Result<Vec<u32>> {
    let mut out: Vec<u32> =
        std::fs::read_dir(game_dir.join("bin"))?.filter_map(|e| e.ok()?.file_name().to_str()?.parse().ok()).collect();
    out.sort();
    Ok(out)
}

impl Resources {
    /// The newest build in the install.
    pub fn open(game_dir: &Path) -> Result<Self> {
        let build = *builds(game_dir)?.last().ok_or_else(|| bad(format!("no builds under {}", game_dir.display())))?;
        Self::open_build(game_dir, build)
    }

    pub fn open_build(game_dir: &Path, build: u32) -> Result<Self> {
        let bin = game_dir.join("bin").join(build.to_string());
        let mut idx: Vec<PathBuf> = std::fs::read_dir(bin.join("idx"))?
            .filter_map(|e| Some(e.ok()?.path()))
            .filter(|p| p.extension().is_some_and(|e| e == "idx"))
            .collect();
        idx.sort();
        let mut res = Resources { build, loose: bin.join("res"), packages: Vec::new(), files: BTreeMap::new() };
        for path in idx {
            let (package, files) =
                parse_index(&std::fs::read(&path)?).map_err(|e| bad(format!("{}: {e}", path.display())))?;
            let n = res.packages.len();
            res.packages.push(game_dir.join("res_packages").join(package));
            res.files.extend(files.into_iter().map(|(p, e)| (p, Entry { package: n, ..e })));
        }
        Ok(res)
    }

    pub fn build(&self) -> u32 {
        self.build
    }

    /// Packaged file paths, sorted.
    pub fn paths(&self) -> impl Iterator<Item = &str> {
        self.files.keys().map(String::as_str)
    }

    /// A file's bytes: loose under `bin/<build>/res` if there, else from its
    /// package, checked against the index's CRC32.
    pub fn read(&self, path: &str) -> Result<Vec<u8>> {
        match std::fs::read(self.loose.join(path)) {
            Ok(b) => return Ok(b),
            Err(e) if e.kind() != ErrorKind::NotFound => return Err(e),
            Err(_) => {}
        }
        let e = self.files.get(path).ok_or_else(|| Error::new(ErrorKind::NotFound, path.to_owned()))?;
        let mut f = std::fs::File::open(&self.packages[e.package])?;
        f.seek(SeekFrom::Start(e.offset))?;
        let mut stored = vec![0; e.stored as usize];
        f.read_exact(&mut stored)?;
        let data = match e.compression {
            0 => stored,
            5 => {
                let mut out = Vec::with_capacity(e.size as usize);
                flate2::read::DeflateDecoder::new(&stored[..]).read_to_end(&mut out)?;
                out
            }
            c => return Err(bad(format!("{path}: compression {c} not supported"))),
        };
        if data.len() as u64 != e.size || crc32(&data) != e.crc {
            return Err(bad(format!("{path}: size or CRC32 does not match the index")));
        }
        Ok(data)
    }
}

impl Resources {
    /// The client's text catalogue for a language (`en`, `de`, …): ship
    /// names under `IDS_<ship index>` (`IDS_PJSD207` → "Shiratsuyu"), and
    /// most other UI text.
    pub fn catalog(&self, lang: &str) -> Result<HashMap<String, String>> {
        parse_mo(&self.read(&format!("texts/{lang}/LC_MESSAGES/global.mo"))?)
    }
}

/// A gettext `.mo` file (little-endian, as the client ships it). Plural
/// entries keep their singular form.
pub fn parse_mo(d: &[u8]) -> Result<HashMap<String, String>> {
    if u32_at(d, 0)? != 0x9504_12de {
        return Err(bad("not a little-endian .mo catalogue".into()));
    }
    let n = u32_at(d, 8)? as usize;
    let (originals, translations) = (u32_at(d, 12)? as usize, u32_at(d, 16)? as usize);
    let text = |table: usize, i: usize| -> Result<String> {
        let (len, at) = (u32_at(d, table + 8 * i)? as usize, u32_at(d, table + 8 * i + 4)? as usize);
        let s = str_at(d, at, len)?;
        Ok(s.split('\0').next().unwrap_or_default().to_owned())
    };
    (0..n).map(|i| Ok((text(originals, i)?, text(translations, i)?))).collect()
}

/// Entity definitions read straight from the install: `scripts/` resolves
/// to the packaged `scripts/` tree.
pub struct InstallScripts<'r>(pub &'r Resources);

impl ScriptSource for InstallScripts<'_> {
    fn read(&self, path: &str) -> Result<Vec<u8>> {
        self.0.read(&format!("scripts/{path}"))
    }
}

fn crc32(data: &[u8]) -> u32 {
    let mut h = flate2::Crc::new();
    h.update(data);
    h.sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A one-file index: `dir/f.txt` in `pkg_0001.pkg`, laid out like the
    /// client's (nodes, then names, then file records, then the footer).
    fn index(name_offset_pad: usize) -> Vec<u8> {
        let mut d = Vec::new();
        d.extend(b"ISFP");
        d.extend(0x200_0000u32.to_le_bytes());
        d.extend(7u32.to_le_bytes());
        d.extend(0x40u32.to_le_bytes());
        d.extend(2u32.to_le_bytes()); // nodes
        d.extend(1u32.to_le_bytes()); // files
        d.extend(1u64.to_le_bytes());
        let nodes_at = 0x38usize;
        let names_at = nodes_at + 2 * 32;
        let files_at = names_at + 4 + 6;
        let footer_at = files_at + 48;
        for off in [nodes_at, files_at, footer_at] {
            d.extend(((off - 0x10) as u64).to_le_bytes());
        }
        // node 0: "dir" (id 10, root parent 0); node 1: "f.txt" (id 11, parent 10)
        for (i, (len, name_at, id, parent)) in
            [(4u64, names_at, 10u64, 0u64), (6, names_at + 4, 11, 10)].into_iter().enumerate()
        {
            let at = nodes_at + i * 32;
            d.extend(len.to_le_bytes());
            d.extend(((name_at - at) as u64).to_le_bytes());
            d.extend(id.to_le_bytes());
            d.extend(parent.to_le_bytes());
        }
        d.extend(b"dir\0f.txt\0");
        d.extend(11u64.to_le_bytes());
        d.extend(99u64.to_le_bytes());
        d.extend(512u64.to_le_bytes()); // offset in the package
        d.extend(5u32.to_le_bytes());
        d.extend(1u32.to_le_bytes());
        d.extend(20u32.to_le_bytes());
        d.extend(0xdead_beefu32.to_le_bytes());
        d.extend(33u64.to_le_bytes());
        d.extend(13u64.to_le_bytes());
        d.extend(((24 + name_offset_pad) as u64).to_le_bytes());
        d.extend(99u64.to_le_bytes());
        d.extend(vec![0; name_offset_pad]);
        d.extend(b"pkg_0001.pkg\0");
        d
    }

    #[test]
    fn parses_an_index() {
        for pad in [0, 24] {
            let (package, files) = parse_index(&index(pad)).unwrap();
            assert_eq!(package, "pkg_0001.pkg");
            let [(path, e)] = files.as_slice() else { panic!("{files:?}") };
            assert_eq!(path, "dir/f.txt");
            assert_eq!((e.offset, e.compression, e.stored, e.crc, e.size), (512, 5, 20, 0xdead_beef, 33));
        }
        let mut short = index(0);
        short.truncate(0x60);
        assert!(parse_index(&short).is_err());
        assert!(parse_index(b"NOPE").is_err());
    }

    #[test]
    fn parses_a_catalogue() {
        // Two entries, the second a plural ("ship\0ships").
        let (k1, v1, k2, v2) = (&b"IDS_A"[..], &b"Alpha"[..], &b"one\0many"[..], &b"ship\0ships"[..]);
        let mut d = Vec::new();
        for v in [0x9504_12de, 0, 2, 28, 44, 0, 0] {
            d.extend(u32::to_le_bytes(v));
        }
        let mut at = 60u32;
        let mut strings = Vec::new();
        let mut table = |s: &[u8], d: &mut Vec<u8>| {
            d.extend((s.len() as u32).to_le_bytes());
            d.extend(at.to_le_bytes());
            strings.extend(s);
            strings.push(0);
            at += s.len() as u32 + 1;
        };
        table(k1, &mut d);
        table(k2, &mut d);
        table(v1, &mut d);
        table(v2, &mut d);
        d.extend(strings);
        let cat = parse_mo(&d).unwrap();
        assert_eq!(cat["IDS_A"], "Alpha");
        assert_eq!(cat["one"], "ship");
        assert!(parse_mo(&[0; 28]).is_err());
    }
}
