//! The `.wowsreplay` file container.
//!
//! ```text
//! magic             u32      0x11343212 on every replay observed
//! block_count       u32      number of length-prefixed JSON blocks
//! block[0]          u32 len + UTF-8 JSON   arena info (the "meta" block)
//! block[1..count]   u32 len + bytes        extra blocks (battle results JSON on
//!                                          finished battles; absent otherwise)
//! decompressed_size u32
//! compressed_size   u32
//! stream            Blowfish-ECB, each plaintext block XORed with the previous
//!                   plaintext block (all-zero IV), wrapping a zlib stream of
//!                   the packet data.
//! ```
//!
//! All integers are little-endian. The packet stream itself is described in
//! [`crate::packet`].

use std::io::Read;

use blowfish::Blowfish;
use blowfish::cipher::BlockDecrypt;
use blowfish::cipher::KeyInit;
use blowfish::cipher::generic_array::GenericArray;

use crate::error::Error;
use crate::meta::ReplayMeta;

/// The fixed Blowfish key every WoWS client uses for the replay stream.
const REPLAY_KEY: [u8; 16] =
    [0x29, 0xB7, 0xC9, 0x09, 0x38, 0x3F, 0x84, 0x88, 0xFA, 0x98, 0xEC, 0x4E, 0x13, 0x19, 0x79, 0xFB];

const BLOCK: usize = 8;

/// A sanity cap on the up-front allocation, so a corrupt size field cannot
/// request gigabytes before inflation even starts.
const MAX_PREALLOC: usize = 512 * 1024 * 1024;

/// A parsed replay: its arena-info JSON and the decrypted, inflated packet
/// stream.
#[derive(Debug, Clone)]
pub struct Replay {
    pub meta: ReplayMeta,
    /// The arena-info block verbatim, for fields [`ReplayMeta`] does not model.
    pub raw_meta: String,
    /// Blocks after the arena info, verbatim. A finished battle usually
    /// carries its results JSON here.
    pub extra_blocks: Vec<Vec<u8>>,
    /// Decrypted, inflated packet stream; walk it with [`crate::packet::Packets`].
    pub packets: Vec<u8>,
    /// Whether the zlib stream ended cleanly. A replay the client was still
    /// writing (or that it crashed out of) inflates to a usable prefix but
    /// reports `false` here.
    pub stream_complete: bool,
}

struct Header<'a> {
    raw_meta: &'a [u8],
    extra_blocks: Vec<&'a [u8]>,
    decompressed_size: u32,
    stream: &'a [u8],
}

fn take<'a>(input: &mut &'a [u8], n: usize, what: &'static str) -> Result<&'a [u8], Error> {
    if input.len() < n {
        return Err(Error::Truncated { what, needed: n, available: input.len() });
    }
    let (head, tail) = input.split_at(n);
    *input = tail;
    Ok(head)
}

fn u32_le(input: &mut &[u8], what: &'static str) -> Result<u32, Error> {
    let b = take(input, 4, what)?;
    Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

fn parse_header(bytes: &[u8]) -> Result<Header<'_>, Error> {
    let mut i = bytes;
    let _magic = u32_le(&mut i, "magic")?;
    let block_count = u32_le(&mut i, "block count")?;
    if block_count == 0 {
        return Err(Error::NoMetaBlock);
    }
    let meta_len = u32_le(&mut i, "meta length")? as usize;
    let raw_meta = take(&mut i, meta_len, "meta block")?;
    let mut extra_blocks = Vec::new();
    for _ in 1..block_count {
        let len = u32_le(&mut i, "block length")? as usize;
        extra_blocks.push(take(&mut i, len, "extra block")?);
    }
    let decompressed_size = u32_le(&mut i, "decompressed size")?;
    let _compressed_size = u32_le(&mut i, "compressed size")?;
    Ok(Header { raw_meta, extra_blocks, decompressed_size, stream: i })
}

/// Reads only the arena-info block. Never touches the encrypted stream, so it
/// is cheap and works on files whose stream is truncated or still being
/// written.
pub fn read_meta(bytes: &[u8]) -> Result<(ReplayMeta, String), Error> {
    let mut i = bytes;
    let _magic = u32_le(&mut i, "magic")?;
    let block_count = u32_le(&mut i, "block count")?;
    if block_count == 0 {
        return Err(Error::NoMetaBlock);
    }
    let meta_len = u32_le(&mut i, "meta length")? as usize;
    let raw = take(&mut i, meta_len, "meta block")?;
    decode_meta(raw)
}

fn decode_meta(raw: &[u8]) -> Result<(ReplayMeta, String), Error> {
    let text = std::str::from_utf8(raw).map_err(|_| Error::MetaNotUtf8)?;
    let meta = serde_json::from_str(text).map_err(Error::MetaJson)?;
    Ok((meta, text.to_owned()))
}

/// Streaming decryptor: Blowfish-ECB with a previous-plaintext XOR chain.
/// A trailing partial block is dropped, which is what lets a file the client
/// is still writing decrypt up to its last whole block.
struct Decryptor<'a> {
    cipher: Blowfish,
    encrypted: &'a [u8],
    previous: [u8; BLOCK],
}

impl<'a> Decryptor<'a> {
    fn new(encrypted: &'a [u8]) -> Self {
        let cipher = Blowfish::new_from_slice(&REPLAY_KEY).expect("16-byte key is within Blowfish limits");
        Self { cipher, encrypted, previous: [0; BLOCK] }
    }
}

impl Read for Decryptor<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let blocks = (self.encrypted.len() / BLOCK).min(buf.len() / BLOCK);
        if blocks == 0 {
            // A sub-block destination would read as EOF; inflate's buffers
            // are always far larger than one block, so this is end of stream.
            return Ok(0);
        }
        let n = blocks * BLOCK;
        for (src, dst) in self.encrypted[..n].chunks_exact(BLOCK).zip(buf[..n].chunks_exact_mut(BLOCK)) {
            let block = GenericArray::from_mut_slice(dst);
            block.copy_from_slice(src);
            self.cipher.decrypt_block(block);
            for (d, p) in dst.iter_mut().zip(self.previous.iter()) {
                *d ^= *p;
            }
            self.previous.copy_from_slice(dst);
        }
        self.encrypted = &self.encrypted[n..];
        Ok(n)
    }
}

impl Replay {
    pub fn from_file(path: &std::path::Path) -> Result<Self, Error> {
        let bytes = std::fs::read(path).map_err(Error::Io)?;
        Self::from_bytes(&bytes)
    }

    /// Parses a complete replay. A packet stream that does not inflate to its
    /// end is still returned (with `stream_complete == false`) as long as
    /// something inflated: the client truncates replays when it exits
    /// mid-battle, and everything before the cut is good data. A stream that
    /// yields nothing at all is an error.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, Error> {
        let header = parse_header(bytes)?;
        let (meta, raw_meta) = decode_meta(header.raw_meta)?;

        let mut decoder = flate2::read::ZlibDecoder::new(Decryptor::new(header.stream));
        let mut packets = Vec::with_capacity((header.decompressed_size as usize).min(MAX_PREALLOC));
        let mut chunk = vec![0u8; 64 * 1024];
        let stream_complete = loop {
            match decoder.read(&mut chunk) {
                Ok(0) => break true,
                Ok(n) => packets.extend_from_slice(&chunk[..n]),
                Err(_) => break false,
            }
        };
        if packets.is_empty() {
            return Err(Error::EmptyStream);
        }

        Ok(Replay {
            meta,
            raw_meta,
            extra_blocks: header.extra_blocks.into_iter().map(<[u8]>::to_vec).collect(),
            packets,
            stream_complete,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use blowfish::cipher::BlockEncrypt;
    use std::io::Write;

    /// Inverse of [`Decryptor`], to build synthetic replays.
    fn encrypt(plain: &[u8]) -> Vec<u8> {
        let cipher: Blowfish = Blowfish::new_from_slice(&REPLAY_KEY).unwrap();
        let mut previous = [0u8; BLOCK];
        let mut out = Vec::new();
        for chunk in plain.chunks(BLOCK) {
            let mut block = [0u8; BLOCK];
            block[..chunk.len()].copy_from_slice(chunk);
            let plain_block = block;
            for (b, p) in block.iter_mut().zip(previous.iter()) {
                *b ^= *p;
            }
            cipher.encrypt_block(GenericArray::from_mut_slice(&mut block));
            out.extend_from_slice(&block);
            previous = plain_block;
        }
        out
    }

    fn synthetic(meta: &str, payload: &[u8]) -> Vec<u8> {
        let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        z.write_all(payload).unwrap();
        let compressed = z.finish().unwrap();
        let mut out = Vec::new();
        out.extend_from_slice(&0x11343212u32.to_le_bytes());
        out.extend_from_slice(&1u32.to_le_bytes());
        out.extend_from_slice(&(meta.len() as u32).to_le_bytes());
        out.extend_from_slice(meta.as_bytes());
        out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        out.extend_from_slice(&(compressed.len() as u32).to_le_bytes());
        out.extend_from_slice(&encrypt(&compressed));
        out
    }

    const META: &str = r#"{"clientVersionFromExe":"15,10,0,0","clientVersionFromXml":"15.10.0","mapName":"spaces/x","mapDisplayName":"x","mapId":1,"gameMode":7,"duration":1200,"playerName":"p","playerVehicle":"v","playerID":1,"dateTime":"01.01.2026 00:00:00","name":"12x12","scenario":"Domination","scenarioConfigId":1,"playersPerTeam":12,"teamsCount":2,"vehicles":[]}"#;

    #[test]
    fn round_trips_a_synthetic_replay() {
        let payload: Vec<u8> = (0..10_000u32).map(|i| (i * 7 % 256) as u8).collect();
        let replay = Replay::from_bytes(&synthetic(META, &payload)).unwrap();
        assert!(replay.stream_complete);
        assert_eq!(replay.packets, payload);
        assert_eq!(replay.meta.map_name, "spaces/x");
    }

    #[test]
    fn a_truncated_stream_yields_its_prefix() {
        let payload: Vec<u8> = (0..200_000u32).map(|i| (i.wrapping_mul(2654435761) >> 13) as u8).collect();
        let mut file = synthetic(META, &payload);
        file.truncate(file.len() - file.len() / 3);
        let replay = Replay::from_bytes(&file).unwrap();
        assert!(!replay.stream_complete);
        assert!(!replay.packets.is_empty());
        assert!(payload.starts_with(&replay.packets));
    }

    #[test]
    fn meta_reads_without_the_stream() {
        let mut file = synthetic(META, b"irrelevant");
        let meta_end = 12 + META.len();
        file.truncate(meta_end);
        let (meta, _) = read_meta(&file).unwrap();
        assert_eq!(meta.client_version_from_exe, "15,10,0,0");
    }
}
