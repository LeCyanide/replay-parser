//! What rendering needs besides the replay, from a data directory:
//!
//! ```text
//! <data>/defs/<key>/            entity definitions (the client's scripts/: entities.xml, entity_defs/)
//! <data>/gameparams/<key>.json  optional: extracted GameParams + ship names (`replay-dump datapack`)
//! <data>/maps/maps.json         map images and sizes, in the suite's seed format
//! <data>/maps/<file>            the images it names
//! <data>/fonts/*.ttf|otf|ttc    optional fallback fonts for names Inter cannot show (CJK, …)
//! <data>/battle_hud/            optional: the client's gui/battle_hud/ (markers_minimap/plane/,
//!                               icon_frag/) for plane markers and kill-type icons
//! ```
//!
//! `<key>` is the replay's build number (`13533378`) or, failing that, its
//! version (`15_10_0`): regional clients share a version but not a build.

use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;

use replay_parser::Version;
use replay_parser::defs::EntityDefs;
use replay_parser::defs::ScriptDir;
use replay_parser::gameparams::GameParams;
use tiny_skia::Pixmap;

#[derive(Debug, thiserror::Error)]
pub enum DataError {
    #[error("no entity definitions for game version {0} under {1}")]
    UnsupportedVersion(String, String),
    #[error("{0}")]
    Missing(String),
    #[error("{path}: {msg}")]
    Bad { path: String, msg: String },
}

/// `replay-dump datapack` output: what the renderer uses from GameParams and
/// the text catalogue, small enough to load in milliseconds.
#[derive(serde::Serialize, serde::Deserialize, Default)]
pub struct GameData {
    pub build: u32,
    pub gameparams: GameParams,
    /// Catalogue entries the labels use: `IDS_<ship index>` and bot names.
    pub names: HashMap<String, String>,
    /// The build's `DEATH_REASON_NAME` constants by index, if extracted.
    #[serde(default)]
    pub death_reasons: Option<Vec<String>>,
}

/// A map in the suite's `maps.json`.
#[derive(serde::Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct MapEntry {
    pub name: String,
    pub file: String,
    #[serde(default)]
    pub space_name: Option<String>,
    /// Side of the playable square in world units.
    #[serde(default)]
    pub space_size: Option<i32>,
    #[serde(default)]
    pub archived: bool,
}

pub struct MapImage {
    pub png: Vec<u8>,
    /// Side of the playable square in world units, if `maps.json` gives it.
    pub space_size: Option<i32>,
}

/// The game's own minimap plane markers and kill-type icons. Empty when the
/// data dir has none: planes are then drawn as shapes, kills named in words.
#[derive(Default)]
pub struct Icons {
    /// By the draw command's icon key, e.g. `controllable/fighter_he_enemy`.
    pub planes: HashMap<String, Pixmap>,
    /// By file stem, e.g. `icon_frag_torpedo`.
    pub frags: HashMap<String, Pixmap>,
}

pub struct DataDir {
    pub root: PathBuf,
}

impl DataDir {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    fn keys(v: &Version) -> [String; 2] {
        [v.build.to_string(), format!("{}_{}_{}", v.major, v.minor, v.patch)]
    }

    pub fn defs(&self, v: &Version) -> Result<EntityDefs, DataError> {
        for key in Self::keys(v) {
            let dir = self.root.join("defs").join(&key);
            if dir.join("entities.xml").is_file() {
                return EntityDefs::load(&ScriptDir(dir.clone()))
                    .map_err(|e| DataError::Bad { path: dir.display().to_string(), msg: e.to_string() });
            }
        }
        Err(DataError::UnsupportedVersion(
            format!("{}.{}.{} (build {})", v.major, v.minor, v.patch, v.build),
            self.root.join("defs").display().to_string(),
        ))
    }

    pub fn game_data(&self, v: &Version) -> Result<Option<GameData>, DataError> {
        for key in Self::keys(v) {
            let path = self.root.join("gameparams").join(format!("{key}.json"));
            match std::fs::read(&path) {
                Ok(b) => {
                    return serde_json::from_slice(&b)
                        .map(Some)
                        .map_err(|e| DataError::Bad { path: path.display().to_string(), msg: e.to_string() });
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(DataError::Bad { path: path.display().to_string(), msg: e.to_string() }),
            }
        }
        Ok(None)
    }

    /// Fallback fonts, by file name: `(path, bytes)`.
    pub fn fonts(&self) -> Vec<(PathBuf, Vec<u8>)> {
        let Ok(dir) = std::fs::read_dir(self.root.join("fonts")) else { return Vec::new() };
        let mut paths: Vec<PathBuf> = dir
            .filter_map(|e| Some(e.ok()?.path()))
            .filter(|p| {
                p.extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(|e| matches!(e.to_ascii_lowercase().as_str(), "ttf" | "otf" | "ttc"))
            })
            .collect();
        paths.sort();
        paths.into_iter().filter_map(|p| Some((p.clone(), std::fs::read(&p).ok()?))).collect()
    }

    /// Plane and kill icons from `<data>/battle_hud/`; unreadable files are skipped.
    pub fn icons(&self) -> Icons {
        let hud = self.root.join("battle_hud");
        let planes = hud.join("markers_minimap").join("plane");
        let mut icons = Icons { frags: pngs(&hud.join("icon_frag")).collect(), ..Icons::default() };
        for dir in ["airsupport", "consumables", "controllable"] {
            icons.planes.extend(pngs(&planes.join(dir)).map(|(stem, pm)| (format!("{dir}/{stem}"), pm)));
        }
        icons
    }

    /// The map image and its space size, by `spaces/<name>`.
    pub fn map(&self, space: &str) -> Result<Option<MapImage>, DataError> {
        let dir = self.root.join("maps");
        let index = dir.join("maps.json");
        let bytes = match std::fs::read(&index) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(DataError::Bad { path: index.display().to_string(), msg: e.to_string() }),
        };
        let maps: Vec<MapEntry> = serde_json::from_slice(&bytes)
            .map_err(|e| DataError::Bad { path: index.display().to_string(), msg: e.to_string() })?;
        let Some(m) = maps.iter().find(|m| !m.archived && m.space_name.as_deref() == Some(space)) else {
            return Ok(None);
        };
        let img = find_file(&dir, &m.file).ok_or_else(|| {
            DataError::Missing(format!("map image {} for {space} not under {}", m.file, dir.display()))
        })?;
        let png =
            std::fs::read(&img).map_err(|e| DataError::Bad { path: img.display().to_string(), msg: e.to_string() })?;
        Ok(Some(MapImage { png, space_size: m.space_size }))
    }
}

/// The PNGs in `dir`, by file stem.
fn pngs(dir: &Path) -> impl Iterator<Item = (String, Pixmap)> {
    std::fs::read_dir(dir).into_iter().flatten().filter_map(|e| {
        let path = e.ok()?.path();
        if path.extension()? != "png" {
            return None;
        }
        Some((path.file_stem()?.to_str()?.to_owned(), Pixmap::load_png(&path).ok()?))
    })
}

/// `<dir>/<file>` or `<dir>/assets/<file>` (the suite keeps images there).
fn find_file(dir: &Path, file: &str) -> Option<PathBuf> {
    [dir.join(file), dir.join("assets").join(file)].into_iter().find(|p| p.is_file())
}
