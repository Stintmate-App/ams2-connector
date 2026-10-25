//! Parity with the Python connector: the catalog's source hashes and idempotency key, the
//! actions' and the guidance's declarations and keys equal what the Python modules produced
//! (`fixtures/announce_parity.json`, written by `scripts/gen_announce_fixtures.py`), and the
//! rules of `test_actions.py` / `test_guidance.py` that need no link.

use crate::actions::{actions, actions_ik};
use crate::announce::catalog::{canonical, catalog, catalog_ik, codes, messages, source_hash};
use crate::guidance::{items, items_ik};
use crate::schema_check;
use serde_json::Value;

fn parity() -> Value {
    let raw = std::fs::read_to_string(format!(
        "{}/fixtures/announce_parity.json",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap();
    serde_json::from_str(&raw).unwrap()
}

/// The codes added after the Python connector (PRY-090): its fixture knows none of them, so the
/// catalog's key and the actions' enum of codes differ from it by exactly these.
const AFTER_PYTHON: [&str; 6] = [
    "flag_yellow_ahead",
    "flag_double_yellow_ahead",
    "flag_yellow_driver",
    "flag_yellow_self",
    "flag_slow_car_driver",
    "flag_track_limits",
];

#[test]
fn source_hashes_equal_the_pythons() {
    let want = parity()["source_hashes"].as_object().unwrap().clone();
    assert_eq!(want.len() + AFTER_PYTHON.len(), messages().len());
    for (code, msg) in messages() {
        if AFTER_PYTHON.contains(&code.as_str()) {
            assert!(!want.contains_key(code), "{code}");
            continue;
        }
        assert_eq!(Value::from(source_hash(msg)), want[code], "{code}");
    }
}

#[test]
fn catalog_idempotency_key_is_the_one_of_this_catalog() {
    // Pinned: the Python key changes with the six codes of PRY-090.
    assert_eq!(catalog_ik(catalog()), "ams2/textcatalog/c7c250c61d18");
}

#[test]
fn actions_equal_the_pythons_but_for_the_new_codes() {
    let p = parity();
    // Compared as canonical JSON: a build that turns on `preserve_order` (the conformance
    // check) does not change what is declared.
    let mut ours = actions().clone();
    let list = ours[1]["params"]["properties"]["code"]["enum"]
        .as_array_mut()
        .unwrap();
    list.retain(|c| !AFTER_PYTHON.contains(&c.as_str().unwrap()));
    assert_eq!(canonical(&ours), canonical(&p["actions"]));
    assert_eq!(actions_ik(actions()), "ams2/actions/7a79b8e9ee86");
}

#[test]
fn guidance_equals_the_pythons_and_so_does_its_key() {
    let p = parity();
    assert_eq!(items(), &p["guidance"]);
    assert_eq!(items_ik(items()), p["guidance_ik"].as_str().unwrap());
}

// ---- test_actions.py

#[test]
fn the_actions_declaration_obeys_the_rules() {
    let acts = actions().as_array().unwrap();
    assert!((1..=32).contains(&acts.len()));
    let names: Vec<_> = acts.iter().map(|a| a["code"].as_str().unwrap()).collect();
    assert_eq!(names, ["test.ping", "test.announce"]);
    let root = serde_json::json!({});
    for a in acts {
        let code = a["code"].as_str().unwrap();
        assert!(
            code.len() <= 64
                && code.bytes().all(|b| b.is_ascii_lowercase()
                    || b == b'.'
                    || b == b'_'
                    || b.is_ascii_digit())
        );
        let desc = a["description"].as_str().unwrap();
        assert!((1..=600).contains(&desc.len()) && desc.is_ascii());
        assert!((1..=60).contains(&a["timeout_s"].as_i64().unwrap()));
        assert_ne!(a.get("requires_confirmation"), Some(&Value::Bool(true)));
        for key in ["params", "result"] {
            assert_eq!(a[key]["type"], "object");
        }
        // the example fits the params' schema
        assert_eq!(
            schema_check::check_schema(&[], &root, &a["params"], &a["example"]),
            Ok(()),
            "{code}"
        );
        assert!(serde_json::to_string(a).unwrap().len() <= 16 * 1024);
    }
    assert!(serde_json::to_string(acts).unwrap().len() <= 128 * 1024);
}

#[test]
fn test_announce_offers_the_catalog_codes() {
    let acts = actions().as_array().unwrap();
    let schema = &acts.iter().find(|a| a["code"] == "test.announce").unwrap()["params"];
    let offered: Vec<_> = schema["properties"]["code"]["enum"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(offered, codes());
}

#[test]
fn the_actions_idempotency_key_follows_the_actions() {
    let a = actions_ik(actions());
    assert!(a.starts_with("ams2/actions/") && a.rsplit('/').next().unwrap().len() == 12);
    let first = Value::Array(actions().as_array().unwrap()[..1].to_vec());
    assert_ne!(actions_ik(&first), a);
}

// ---- test_guidance.py

#[test]
fn the_guidance_items_follow_the_declare_rules() {
    const TOPICS: [&str; 12] = [
        "setup",
        "capture",
        "tyres",
        "fuel",
        "pit",
        "flags",
        "weather",
        "session",
        "car",
        "track",
        "telemetry",
        "other",
    ];
    let its = items().as_array().unwrap();
    assert!((1..=16).contains(&its.len()));
    let mut ids: Vec<_> = its.iter().map(|i| i["id"].as_str().unwrap()).collect();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), its.len());
    let mut total = 0;
    for i in its {
        let mut keys: Vec<_> = i.as_object().unwrap().keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(keys, ["id", "text", "topic", "when"]);
        let id = i["id"].as_str().unwrap();
        assert!(id.len() <= 40 && id.as_bytes()[0].is_ascii_lowercase());
        assert!(
            id.bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
        );
        assert!(TOPICS.contains(&i["topic"].as_str().unwrap()));
        let (when, text) = (i["when"].as_str().unwrap(), i["text"].as_str().unwrap());
        assert!((1..=120).contains(&when.len()) && !when.contains('\n'));
        assert!((1..=1500).contains(&text.len()));
        assert!(text.is_ascii() && when.is_ascii());
        total += text.len();
    }
    assert!(total <= 8000);
    let topics: Vec<_> = its.iter().map(|i| i["topic"].as_str().unwrap()).collect();
    assert!(topics.contains(&"setup") && topics.contains(&"capture"));
}

#[test]
fn setup_guidance_names_the_tools_the_engineer_has() {
    let text = items()
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["topic"] == "setup")
        .unwrap()["text"]
        .as_str()
        .unwrap();
    for word in ["*.vts", "capture button", "list_captures", "get_capture"] {
        assert!(text.contains(word), "{word}");
    }
}

#[test]
fn the_guidance_idempotency_key_follows_the_items() {
    let a = items_ik(items());
    assert!(a.starts_with("ams2/guidance/") && a.rsplit('/').next().unwrap().len() == 12);
    let first = Value::Array(items().as_array().unwrap()[..1].to_vec());
    assert_ne!(items_ik(&first), a);
}
