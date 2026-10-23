//! `ams2-replay --scripted | --capture FILE [--name NAME] [--hz 50] [--speed 1] [--hold SECS]`:
//! plays into a test mapping (never the game's) and keeps it for `--hold` seconds afterwards.

use ams2_replay::{capture, play, scripted};
use ams2_shm::testing::TestMapping;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let value = |flag: &str| {
        args.iter()
            .position(|a| a == flag)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    let number =
        |flag: &str, default: f64| value(flag).and_then(|v| v.parse().ok()).unwrap_or(default);
    let frames = if args.iter().any(|a| a == "--scripted") {
        scripted()
    } else if let Some(file) = value("--capture") {
        match capture(std::path::Path::new(&file)) {
            Ok(f) => f,
            Err(e) => {
                eprintln!("ams2-replay: {file}: {e}");
                std::process::exit(1);
            }
        }
    } else {
        eprintln!(
            "usage: ams2-replay --scripted | --capture FILE [--name NAME] [--hz 50] [--speed 1] [--hold SECS]"
        );
        std::process::exit(2);
    };
    let name = value("--name").unwrap_or_else(|| {
        format!(
            "Local{}ams2-replay-{}",
            std::path::MAIN_SEPARATOR,
            std::process::id()
        )
    });
    let mut map = match TestMapping::create(&name) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("ams2-replay: {e}");
            std::process::exit(1);
        }
    };
    println!("mapping {name}: {} frames", frames.len());
    let n = play(
        &mut map,
        &frames,
        number("--hz", 50.0),
        number("--speed", 1.0),
        &AtomicBool::new(false),
    );
    println!("played {n} frames");
    std::thread::sleep(Duration::from_secs_f64(number("--hold", 0.0)));
}
