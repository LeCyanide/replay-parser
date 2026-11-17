//! Game constants the replay carries as numbers.
//!
//! Death reasons: `receiveVehicleDeath`'s third argument indexes the
//! `DEATH_REASON_NAME` enum of the client's `gui/data/constants/battle.xml`
//! (by position). [`death_reasons_from_xml`] reads a build's own table;
//! [`DEFAULT_DEATH_REASONS`] is the table as wows-toolkit documents it, used
//! without game files. On the fixtures the default agrees with what the
//! replays show: every torpedo death (3, 5) follows a torpedo hit by the
//! killer, and shell deaths (2, 17, 18, 19) follow shell hits (see
//! `tests/world.rs`).

/// Index → `DEATH_REASON_NAME` constant.
pub const DEFAULT_DEATH_REASONS: &[&str] = &[
    "NONE",
    "ARTILLERY",
    "ATBA",
    "TORPEDO",
    "BOMB",
    "TBOMB",
    "BURNING",
    "RAM",
    "TERRAIN",
    "FLOOD",
    "MIRROR",
    "SEA_MINE",
    "SPECIAL",
    "DBOMB",
    "ROCKET",
    "DETONATE",
    "HEALTH",
    "AP_SHELL",
    "HE_SHELL",
    "CS_SHELL",
    "FEL",
    "PORTAL",
    "SKIP_BOMB",
    "SECTOR_WAVE",
    "ACID",
    "LASER",
    "MATCH",
    "TIMER",
    "ADBOMB",
    "EVENT_1",
    "EVENT_2",
    "EVENT_3",
    "EVENT_4",
    "EVENT_5",
    "EVENT_6",
    "MISSILE",
];

/// The `DEATH_REASON_NAME` constants of a `battle.xml`, in order.
pub fn death_reasons_from_xml(xml: &str) -> Option<Vec<String>> {
    let doc = roxmltree::Document::parse(xml).ok()?;
    let e = doc.descendants().find(|n| n.has_tag_name("enum") && n.attribute("name") == Some("DEATH_REASON_NAME"))?;
    let names: Vec<String> = e
        .children()
        .filter(|c| c.has_tag_name("const"))
        .map(|c| c.attribute("name").unwrap_or_default().to_owned())
        .collect();
    (!names.is_empty()).then_some(names)
}

/// A death reason's constant name: from `table` (a build's own) if given,
/// else the default.
pub fn death_reason<'a>(code: i64, table: Option<&'a [String]>) -> Option<&'a str> {
    let i = usize::try_from(code).ok()?;
    match table {
        Some(t) => t.get(i).map(String::as_str).filter(|s| !s.is_empty()),
        None => DEFAULT_DEATH_REASONS.get(i).copied(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_positional_enum() {
        let xml = r#"<root><enum name="OTHER"><const name="X"/></enum>
            <enum name="DEATH_REASON_NAME"><const name="NONE"/><const name="ARTILLERY"/><const name="ATBA"/></enum></root>"#;
        let t = death_reasons_from_xml(xml).unwrap();
        assert_eq!(t, ["NONE", "ARTILLERY", "ATBA"]);
        assert_eq!(death_reason(2, Some(&t)), Some("ATBA"));
        assert_eq!(death_reason(3, Some(&t)), None);
        assert_eq!(death_reason(17, None), Some("AP_SHELL"));
        assert_eq!(death_reason(-1, None), None);
    }
}
