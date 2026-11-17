//! The arena-info block: battle metadata the client writes in plain JSON at
//! the head of every replay.
//!
//! Only fields the suite uses are modelled, and only the ones present in every
//! observed replay are required; the full JSON stays available as
//! [`crate::Replay::raw_meta`].

use serde::Deserialize;
use serde::Serialize;

use crate::error::Error;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReplayMeta {
    pub client_version_from_exe: String,
    pub client_version_from_xml: String,
    pub map_name: String,
    pub map_display_name: String,
    pub map_id: u32,
    pub game_mode: u32,
    #[serde(default)]
    pub game_type: Option<String>,
    #[serde(default)]
    pub match_group: Option<String>,
    pub scenario: String,
    pub scenario_config_id: u32,
    /// Battle time limit in seconds, not how long the battle actually ran.
    pub duration: u32,
    #[serde(default)]
    pub battle_duration: Option<u32>,
    pub players_per_team: u32,
    pub teams_count: u32,
    pub date_time: String,
    pub name: String,
    pub player_name: String,
    #[serde(rename = "playerID")]
    pub player_id: i64,
    pub player_vehicle: String,
    pub vehicles: Vec<VehicleMeta>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VehicleMeta {
    /// GameParams id of the ship.
    pub ship_id: i64,
    /// 0 = the recording player, 1 = ally, 2 = enemy.
    pub relation: u32,
    /// Not the ship entity id. The Avatar id in some builds (14.11), another
    /// player id in others (13.11); the `onArenaStateReceived` roster entry
    /// holding this id also holds the Avatar id, which `Vehicle.owner` names.
    pub id: i64,
    pub name: String,
}

/// A client version, from `clientVersionFromExe` ("15,10,0,12345678").
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct Version {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
    /// Regional clients share a semantic version but not a build number, so
    /// comparisons that select wire layouts must use the first three fields.
    pub build: u32,
}

impl Version {
    pub fn parse(s: &str) -> Result<Self, Error> {
        let bad = || Error::BadVersion(s.to_owned());
        let parts: Vec<u32> =
            s.split([',', '.']).map(|p| p.trim().parse::<u32>()).collect::<Result<_, _>>().map_err(|_| bad())?;
        match parts.as_slice() {
            [major, minor, patch, build] => Ok(Version { major: *major, minor: *minor, patch: *patch, build: *build }),
            [major, minor, patch] => Ok(Version { major: *major, minor: *minor, patch: *patch, build: 0 }),
            _ => Err(bad()),
        }
    }

    pub fn semver(&self) -> (u32, u32, u32) {
        (self.major, self.minor, self.patch)
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}.{}", self.major, self.minor, self.patch, self.build)
    }
}

impl ReplayMeta {
    pub fn version(&self) -> Result<Version, Error> {
        Version::parse(&self.client_version_from_exe)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_parse_and_order_by_semver() {
        let a = Version::parse("12,5,0,7000000").unwrap();
        let b = Version::parse("12,6,0,100").unwrap();
        assert!(a.semver() < b.semver());
        assert_eq!(b.to_string(), "12.6.0.100");
        assert!(Version::parse("12,x,0,1").is_err());
    }
}
