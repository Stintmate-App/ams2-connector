//! `ams2-golden`: the golden comparison of exports of a capture. Not shipped.
//!
//! ```text
//! ams2-golden export <capture.a2raw> [--out DIR]
//! ams2-golden compare A.jsonl B.jsonl
//! ams2-golden dump <capture.a2raw> [--from S] [--to S] [--step N]
//! ams2-golden bench <capture.a2raw> [--from SECONDS] [--seconds N]
//! ```
//!
//! `export` writes `<DIR>\<stem>.rust.jsonl` (default: the dev captures' `golden` folder in
//! `%LOCALAPPDATA%\Stintmate`): ticks every 20 ms of capture time, the newest record at or
//! before each tick, the capture time in seconds as the clock, `ISO` for every wall-clock
//! text, the live frame at every 500th tick (10 s) that is processed (`data_age_s` 0), and `data` null for an
//! announcement without facts.

mod bench;
mod compare;
mod dump;
mod export;
mod sampler;

use std::path::PathBuf;
use std::process::ExitCode;

fn arg(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

fn usage() -> ExitCode {
    eprintln!(
        "usage: ams2-golden export <capture> [--out DIR] | compare A.jsonl B.jsonl | bench <capture> [--from S] [--seconds N]"
    );
    ExitCode::from(2)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("export") if args.len() >= 2 => export(&args),
        Some("compare") if args.len() >= 3 => {
            compare::run(&PathBuf::from(&args[1]), &PathBuf::from(&args[2]))
        }
        Some("dump") if args.len() >= 2 => {
            let num = |f: &str, d: f64| arg(&args, f).and_then(|v| v.parse().ok()).unwrap_or(d);
            dump::run(
                &PathBuf::from(&args[1]),
                num("--from", 0.0),
                num("--to", 60.0),
                num("--step", 1.0) as u64,
            )
            .map(|()| true)
        }
        Some("bench") if args.len() >= 2 => {
            let num = |f: &str, d: f64| arg(&args, f).and_then(|v| v.parse().ok()).unwrap_or(d);
            bench::run(
                &PathBuf::from(&args[1]),
                num("--from", 600.0),
                num("--seconds", 60.0),
            )
            .map(|()| true)
        }
        _ => return usage(),
    };
    match result {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::from(2)
        }
    }
}

fn export(args: &[String]) -> Result<bool, String> {
    let capture = PathBuf::from(&args[1]);
    let path = export::default_out(&capture, arg(args, "--out").map(PathBuf::from));
    let mut out = export::create(&path)?;
    let s = export::run(&capture, &mut out)?;
    println!(
        "{}: {} records read, {} ticks processed, {} lines ({} call, {} announce, {} live), {:.1} s -> {}",
        capture.display(),
        s.records,
        s.ticks,
        s.lines.iter().sum::<u64>(),
        s.lines[0],
        s.lines[1],
        s.lines[2],
        s.seconds,
        path.display()
    );
    for (i, laps) in s.sessions.iter().enumerate() {
        let list: Vec<String> = laps
            .iter()
            .map(|l| {
                format!(
                    "{}:{}{}",
                    l.lap,
                    l.time_s.map_or("-".to_owned(), |t| format!("{t:.3}")),
                    if l.valid { "" } else { "!" }
                )
            })
            .collect();
        println!(
            "session {}: {} laps [{}] (! = invalid)",
            i + 1,
            laps.len(),
            list.join(" ")
        );
    }
    Ok(true)
}
