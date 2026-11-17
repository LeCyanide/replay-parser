//! What rendering needs besides the replay, from a data directory:
//!
//! ```text
//! <data>/defs/<key>/            entity definitions (the client's scripts/: entities.xml, entity_defs/)
//! <data>/gameparams/<key>.json  optional: extracted GameParams + ship names (`replay-dump datapack`)
//! <data>/maps/maps.json         map images and sizes, in the suite's seed format
//! <data>/maps/<file>            the images it names
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

/// `<dir>/<file>` or `<dir>/assets/<file>` (the suite keeps images there).
fn find_file(dir: &Path, file: &str) -> Option<PathBuf> {
    [dir.join(file), dir.join("assets").join(file)].into_iter().find(|p| p.is_file())
}
