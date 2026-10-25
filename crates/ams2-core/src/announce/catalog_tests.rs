//! The `connector.ams2` announcement catalog (PRY-052): valid in the team's catalog format,
//! params checked as Pitwall checks them, every code with a validation status. A port of the
//! Python connector's `test_announce_catalog.py`.

use super::catalog::*;
use serde_json::{Value, json};

fn msgs() -> &'static serde_json::Map<String, Value> {
    messages()
}

#[test]
fn the_catalog_is_valid() {
    assert_eq!(validate_catalog(catalog()), Vec::<String>::new());
    assert_eq!(catalog()["schema"], "stintmate-catalog/1");
    assert_eq!(catalog()["catalog"], "connector.ams2");
    assert_eq!(CATALOG_ID, "connector.ams2");
    assert_eq!(catalog()["language"], "en");
}

#[test]
fn every_code_has_a_validation_status() {
    for code in msgs().keys() {
        assert!(validation(code).is_some(), "{code}");
    }
    assert_eq!(validation("flag_yellow"), Some(NEEDS_RACE));
    assert_eq!(validation("radio_check"), Some(DESK_TESTED));
    assert_eq!(validation("engineer_test"), Some(DESK_TESTED));
    assert_eq!(validation("nope"), None);
}

#[test]
fn the_first_set_of_the_research_is_there() {
    // Note: the catalog has 28 codes (the task brief said 39; the Python catalog has 28).
    let expected = [
        "flag_yellow",
        "flag_double_yellow",
        "flag_yellow_cause",
        "flag_clear",
        "flag_blue",
        "flag_blue_unknown",
        "flag_slow_car",
        "final_lap",
        "flag_red",
        "flag_black_white",
        "flag_black_orange",
        "disqualified",
        "retired",
        "chequered",
        "chequered_no_position",
        "fcy_start",
        "fcy_pits_open",
        "fcy_pits_lead_lap",
        "fcy_last_lap",
        "fcy_restart",
        "fcy_halt",
        "penalty_drive_through",
        "penalty_stop_go",
        "penalty_served",
        "rival_disqualified",
        "rival_out",
        "radio_check",
        "engineer_test",
    ];
    for code in expected {
        assert!(msgs().contains_key(code), "{code}");
    }
    assert_eq!(msgs().len(), expected.len());
    assert_eq!(msgs()["engineer_test"]["audience"], "agent");
    for (k, m) in msgs() {
        if k != "engineer_test" {
            assert_eq!(m["audience"], "driver", "{k}");
        }
    }
}

#[test]
fn source_hashes_are_12_hex_digits_and_follow_the_text() {
    let hashes: Vec<(String, String)> = msgs()
        .iter()
        .map(|(c, m)| (c.clone(), source_hash(m)))
        .collect();
    for (_, h) in &hashes {
        assert!(
            h.len() == 12
                && h.bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        );
    }
    let base = &msgs()["flag_yellow"];
    let h0 = source_hash(base);
    let mut changed = base.clone();
    changed["text"] = json!("Yellow, sector {sector}.");
    assert_ne!(source_hash(&changed), h0);
    // The PRY-052 fields and the note are not part of the hash.
    let mut other = base.clone();
    other["ttl_s"] = json!(99);
    other["note"] = json!("x");
    assert_eq!(source_hash(&other), h0);
}

#[test]
fn source_hash_matches_the_contract_example() {
    // docs/catalogs.md section 8: canonical JSON of {"params", "text"}, sorted, no spaces.
    let msg = json!({"text": "Combustible para {laps}.", "params": {"laps": "laps"}});
    let raw = r#"{"params":{"laps":"laps"},"text":"Combustible para {laps}."}"#;
    assert_eq!(
        canonical(&json!({"params": {"laps": "laps"}, "text": "Combustible para {laps}."})),
        raw
    );
    assert_eq!(source_hash(&msg), sha12(raw));
}

#[test]
fn params_that_fit() {
    for (code, params) in [
        ("flag_yellow", json!({"sector": 2})),
        ("flag_yellow_cause", json!({"cause": "vehicle_crash"})),
        ("flag_blue", json!({"driver": "Paris"})),
        ("chequered", json!({"position": 3})),
        ("radio_check", json!({})),
        ("engineer_test", json!({"text": "hello"})),
    ] {
        assert_eq!(check_params(code, &params, msgs()), None, "{code}");
    }
}

#[test]
fn params_refused_name_the_param() {
    for (code, params, word) in [
        ("nope", json!({}), "unknown code"),
        ("flag_yellow", json!({}), "missing"),
        ("flag_yellow", json!({"sector": 2, "x": 1}), "not declared"),
        ("flag_yellow", json!({"sector": "2"}), "integer"),
        ("flag_yellow", json!({"sector": true}), "integer"),
        ("flag_yellow", json!({"sector": 2.0}), "integer"),
        ("flag_blue", json!({"driver": 3}), "text"),
        ("chequered", json!({"position": 0}), ">= 1"),
        (
            "flag_yellow_cause",
            json!({"cause": "meteor"}),
            "not one of",
        ),
        ("radio_check", json!([]), "map"),
    ] {
        let reason = check_params(code, &params, msgs());
        assert!(
            reason.as_deref().is_some_and(|r| r.contains(word)),
            "{code}: {reason:?}"
        );
    }
}

#[test]
fn a_refused_enum_value_is_not_echoed() {
    // BUG-093: the refusal reaches the log; the value may hold what the driver said.
    let reason = check_params(
        "flag_yellow_cause",
        &json!({"cause": "the driver's own words"}),
        msgs(),
    )
    .unwrap();
    assert!(reason.contains("not one of"), "{reason}");
    assert!(!reason.contains("driver's own words"), "{reason}");
}

#[test]
fn validate_catalog_finds_problems() {
    let mut bad = catalog().clone();
    let m = bad["messages"].as_object_mut().unwrap();
    let radio = m["radio_check"].clone();
    m.insert("Bad-Code".into(), radio);
    m["flag_yellow"]["text"] = json!("Yellow {sector} {lap}.");
    m["flag_clear"]["text"] = json!("Clear\u{7}");
    m["flag_red"]["priority"] = json!("urgent");
    m["flag_slow_car"]["audience"] = json!("everyone");
    m["fcy_start"]["ttl_s"] = json!(0);
    m["flag_yellow_cause"]["values"] = json!({"cause": {"solo_crash": "x"}});
    m["final_lap"]["text"] = json!("Final {lap");
    m["retired"]["text"] = json!("x".repeat(201));
    let problems = validate_catalog(&bad).join("\n");
    for word in [
        "Bad-Code",
        "flag_yellow: placeholders",
        "flag_clear: control",
        "flag_red: priority",
        "flag_slow_car: audience",
        "fcy_start: ttl_s",
        "flag_yellow_cause: enum",
        "final_lap: bad placeholder",
        "retired: longer",
    ] {
        assert!(problems.contains(word), "{word}\n{problems}");
    }
}

#[test]
fn placeholders_parser() {
    assert_eq!(
        placeholders("a {x} b {{literal}} {y}"),
        Some(vec!["x".to_string(), "y".to_string()])
    );
    assert_eq!(placeholders("a } b"), None);
    assert_eq!(placeholders("{Bad}"), None);
}

#[test]
fn every_code_has_its_group() {
    for (code, m) in msgs() {
        let group = m["group"].as_str().unwrap();
        assert!(GROUPS.contains(&group), "{code}");
        if code.starts_with("flag_") {
            assert_eq!(group, "flags", "{code}");
        } else if code.starts_with("fcy_") {
            assert_eq!(group, "safety_car", "{code}");
        } else if code.starts_with("penalty_") {
            assert_eq!(group, "penalties", "{code}");
        } else if code.starts_with("rival_") {
            assert_eq!(group, "rivals", "{code}");
        }
    }
    for code in [
        "final_lap",
        "chequered",
        "chequered_no_position",
        "disqualified",
        "retired",
    ] {
        assert_eq!(msgs()[code]["group"], "race", "{code}");
    }
    assert_eq!(msgs()["radio_check"]["group"], "test");
    assert_eq!(msgs()["engineer_test"]["group"], "test");
    let mut bad = catalog().clone();
    bad["messages"]["flag_red"]["group"] = json!("banderas");
    assert!(
        validate_catalog(&bad)
            .join("\n")
            .contains("flag_red: group")
    );
}
