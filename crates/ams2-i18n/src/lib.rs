//! The AMS2 connector's texts: the team's text catalogs contract v1 (`docs/catalogs.md`).
//!
//! The chain of catalogs is the embedded English catalog `ams2-connector` ([`embedded`]) with
//! the workspace's language packs over it ([`load`]), per key, every pack entry validated with
//! the contract's reason codes ([`Reason`]). Plural rules are the CLDR subset integer counts
//! need ([`plural`]); source hashes mark stale translations ([`source_hash`]). The test
//! pseudo-locale (`qps`) is [`pseudo_pack`].
//!
//! Not localised: logs, JSON files, IPC error texts and ids, the connector descriptor. The
//! recorder never loads this crate's catalog on a hot path; the control app loads it once at
//! start.

mod catalog;
mod load;
mod message;
pub mod plural;
mod reason;
mod tag;

use std::sync::OnceLock;

use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

pub use catalog::{Arg, Catalog};
pub use load::{CATALOG, LANGUAGE_FILE, Loaded, load, read_json, read_language};
pub use message::{
    KINDS, NUMBERS_SCHEMA, SCHEMA, Segment, check_file, kind_of, names, parse, source_hash,
    validate, validate_source,
};
pub use plural::{PluralRules, compile_rules, plural_category};
pub use reason::Reason;
pub use tag::{chain, normalize_tag};

/// The embedded English catalog file, byte for byte (`locales/en.json`).
pub const EMBEDDED_JSON: &str = include_str!("../locales/en.json");

/// The embedded English catalog file, parsed once.
pub fn embedded() -> &'static Value {
    static DOC: OnceLock<Value> = OnceLock::new();
    DOC.get_or_init(|| serde_json::from_str(EMBEDDED_JSON).unwrap_or(Value::Null))
}

/// The embedded English source messages.
pub fn embedded_messages() -> &'static Map<String, Value> {
    static EMPTY: OnceLock<Map<String, Value>> = OnceLock::new();
    embedded()
        .get("messages")
        .and_then(Value::as_object)
        .unwrap_or_else(|| EMPTY.get_or_init(Map::new))
}

/// `control.catalog.get`: the embedded catalog file and its `rev`, the first 12 hex digits of
/// the SHA-256 of its canonical JSON (sorted keys, no spaces, UTF-8 as it is).
pub fn embedded_document() -> (&'static Value, String) {
    let doc = embedded();
    let canon = serde_json::to_string(doc).unwrap_or_default();
    (
        doc,
        hex::encode(Sha256::digest(canon.as_bytes()))[..12].to_string(),
    )
}

/// The pseudo-locale pack of `messages` for tests: every text wrapped in `⟦…⟧` with accented
/// letters (placeholders untouched), so a test can find a visible text that did not go
/// through the catalog. Load it as the pack of the tag [`PSEUDO_TAG`].
pub fn pseudo_pack(messages: &Map<String, Value>) -> Map<String, Value> {
    messages
        .iter()
        .map(|(key, msg)| {
            let text = match msg.get("text") {
                Some(Value::Object(forms)) => Value::Object(
                    forms
                        .iter()
                        .map(|(c, v)| (c.clone(), Value::String(pseudo(v.as_str().unwrap_or("")))))
                        .collect(),
                ),
                other => Value::String(pseudo(other.and_then(Value::as_str).unwrap_or(""))),
            };
            (key.clone(), serde_json::json!({ "text": text }))
        })
        .collect()
}

/// The tag of the pseudo-locale.
pub const PSEUDO_TAG: &str = "qps";

fn accent(c: char) -> char {
    match c {
        'a' => 'á',
        'e' => 'é',
        'i' => 'í',
        'o' => 'ó',
        'u' => 'ú',
        'A' => 'Á',
        'E' => 'É',
        'I' => 'Í',
        'O' => 'Ó',
        'U' => 'Ú',
        'c' => 'ç',
        'n' => 'ñ',
        'y' => 'ý',
        'C' => 'Ç',
        'N' => 'Ñ',
        'Y' => 'Ý',
        other => other,
    }
}

fn pseudo(text: &str) -> String {
    let mut out = String::from("⟦");
    for seg in parse(text).unwrap_or_default() {
        match seg {
            Segment::Name(n) => {
                out.push('{');
                out.push_str(&n);
                out.push('}');
            }
            Segment::Lit(s) => {
                for c in s.chars() {
                    match c {
                        '{' => out.push_str("{{"),
                        '}' => out.push_str("}}"),
                        c => out.push(accent(c)),
                    }
                }
            }
        }
    }
    out.push('⟧');
    out
}
