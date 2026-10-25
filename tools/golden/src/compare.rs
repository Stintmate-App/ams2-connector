//! `compare`: two exports, aligned per kind, with the golden tolerance.

use ams2_core::golden::same;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::io::{BufRead, BufReader};
use std::path::Path;

const KINDS: [&str; 3] = ["call", "announce", "live"];
const SHOWN: usize = 20;

#[derive(Default)]
struct Side {
    kinds: [Vec<Value>; 3],
}

fn load(path: &Path) -> Result<Side, String> {
    let f = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut side = Side::default();
    for (n, line) in BufReader::new(f).lines().enumerate() {
        let line = line.map_err(|e| format!("{}: {e}", path.display()))?;
        if line.trim().is_empty() {
            continue;
        }
        let v: Value = serde_json::from_str(&line)
            .map_err(|e| format!("{}:{}: {e}", path.display(), n + 1))?;
        let kind = v["kind"].as_str().unwrap_or("");
        let i = KINDS
            .iter()
            .position(|k| *k == kind)
            .ok_or_else(|| format!("{}:{}: unknown kind {kind:?}", path.display(), n + 1))?;
        side.kinds[i].push(v);
    }
    Ok(side)
}

fn t_ms(v: &Value) -> i64 {
    v["t_ms"].as_i64().unwrap_or(-1)
}

fn label(kind: &str, v: &Value) -> String {
    match kind {
        "call" => match v["call"].as_str() {
            Some("event") => format!("event {}", v["type"].as_str().unwrap_or("?")),
            Some(c) => c.to_owned(),
            None => String::new(),
        },
        "announce" => v["code"].as_str().unwrap_or("?").to_owned(),
        _ => "frame".to_owned(),
    }
}

fn without_time(v: &Value) -> Value {
    let mut v = v.clone();
    if let Some(o) = v.as_object_mut() {
        o.remove("t_ms");
    }
    v
}

fn compare_live(xa: &[Value], xb: &[Value]) -> Vec<String> {
    fn index(x: &[Value]) -> BTreeMap<i64, &Value> {
        x.iter().map(|v| (t_ms(v), v)).collect()
    }
    let (ia, ib) = (index(xa), index(xb));
    let mut found = Vec::new();
    for t in ia.keys().chain(ib.keys()).collect::<BTreeSet<_>>() {
        match (ia.get(t), ib.get(t)) {
            (Some(p), Some(q)) => {
                if let Err(e) = same(&p["frame"], &q["frame"], "frame") {
                    found.push(format!("live t_ms={t}: {e}"));
                }
            }
            (Some(_), None) => found.push(format!("live t_ms={t}: only in A")),
            _ => found.push(format!("live t_ms={t}: only in B")),
        }
    }
    found
}

fn compare_ordered(kind: &str, xa: &[Value], xb: &[Value]) -> Vec<String> {
    let mut found = Vec::new();
    for (n, (p, q)) in xa.iter().zip(xb).enumerate() {
        let (la, lb) = (label(kind, p), label(kind, q));
        if la != lb {
            found.push(format!(
                "{kind} #{n} t_ms={}/{}: A is {la:?}, B is {lb:?}",
                t_ms(p),
                t_ms(q)
            ));
        } else if let Err(e) = same(&without_time(p), &without_time(q), &format!("#{n} {la}")) {
            found.push(format!("{kind} t_ms={}/{}: {e}", t_ms(p), t_ms(q)));
        }
    }
    for (side, x, other) in [("A", xa, xb.len()), ("B", xb, xa.len())] {
        for (n, v) in x.iter().enumerate().skip(other) {
            found.push(format!(
                "{kind} #{n} t_ms={}: {} only in {side}",
                t_ms(v),
                label(kind, v)
            ));
        }
    }
    found
}

/// Compares the two files, prints the report, and says whether they match.
pub fn run(a: &Path, b: &Path) -> Result<bool, String> {
    let (sa, sb) = (load(a)?, load(b)?);
    println!("A: {}\nB: {}", a.display(), b.display());
    let mut diffs: Vec<String> = Vec::new();
    for (i, kind) in KINDS.iter().enumerate() {
        let (xa, xb) = (&sa.kinds[i], &sb.kinds[i]);
        let found = if *kind == "live" {
            compare_live(xa, xb)
        } else {
            compare_ordered(kind, xa, xb)
        };
        println!(
            "{kind}: A {} / B {}, {} difference(s)",
            xa.len(),
            xb.len(),
            found.len()
        );
        diffs.extend(found);
    }
    for d in diffs.iter().take(SHOWN) {
        println!("  - {d}");
    }
    if diffs.len() > SHOWN {
        println!("  ... {} more", diffs.len() - SHOWN);
    }
    println!("{}", if diffs.is_empty() { "PASS" } else { "FAIL" });
    Ok(diffs.is_empty())
}
