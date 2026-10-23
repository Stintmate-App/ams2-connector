//! The recorder's subcommands: `announce` (the test hook of `connector_messages.py send`),
//! `codes`, `diagnose` (`scripts/diagnose.py`) and, in dev builds, `capture-raw`.
//! Each takes an `out` sink for its lines so tests can read them.

use crate::announcer::{ANNOUNCE_TIMEOUT, Announcer, MAX_DATA};
use crate::link::{CallError, PitwallLink, Transport, key_source, peer_info, to_value};
use ams2_core::announce::catalog::{self, check_params, parse_params};
use ams2_core::decode_str;
use ams2_shm::{FRAME_SIZE, Mapping, SHARED_MEMORY_VERSION, SnapshotOutcome, new_frame};
use serde_json::{Map, Value as Json, json};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use stintmate_ipc::codec::{self, Value};

/// `peer.process` of the test hook.
pub const MESSAGE_CLI: &str = "message-cli";
/// How long `announce` waits for Pitwall.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// The subcommands this executable has besides the recorder itself.
pub const SUBCOMMANDS: [&str; 4] = ["announce", "codes", "diagnose", "capture-raw"];

/// `announce CODE [--param NAME=VALUE]... [--data JSON] [--pitwall-pipe NAME]`, parsed.
#[derive(Debug, Default, PartialEq)]
pub struct AnnounceArgs {
    /// A code of the catalog.
    pub code: String,
    /// `NAME=VALUE` pairs.
    pub params: Vec<String>,
    /// Facts for the engineer.
    pub data: Option<Map<String, Json>>,
    /// Pitwall's endpoint (tests).
    pub pipe: Option<String>,
}

/// Parses the arguments after `announce`; the error is the line to print (exit 2).
pub fn parse_announce(args: &[String]) -> Result<AnnounceArgs, String> {
    let mut out = AnnounceArgs::default();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut value = |what: &str| {
            it.next()
                .cloned()
                .ok_or_else(|| format!("{what} needs a value"))
        };
        match a.as_str() {
            "--param" => out.params.push(value("--param")?),
            "--data" => {
                let text = value("--data")?;
                match serde_json::from_str::<Json>(&text) {
                    Ok(Json::Object(m)) => out.data = Some(m),
                    Ok(_) => return Err("--data: must be a JSON object".into()),
                    Err(e) => return Err(format!("--data: not JSON: {e}")),
                }
            }
            "--pitwall-pipe" => out.pipe = Some(value("--pitwall-pipe")?),
            s if s.starts_with("--") => return Err(format!("unknown argument {s}")),
            _ if out.code.is_empty() => out.code = a.clone(),
            other => return Err(format!("unexpected argument {other}")),
        }
    }
    if out.code.is_empty() {
        return Err("announce needs a CODE (see: codes)".into());
    }
    Ok(out)
}

/// Declares the catalog on `link` and sends one announcement; prints the answer. 0: accepted,
/// 1: refused or not delivered, 2: bad arguments.
pub fn send(link: Arc<dyn Transport>, args: &AnnounceArgs, out: &mut dyn FnMut(String)) -> i32 {
    let params = match parse_params(&args.code, &args.params) {
        Ok(p) => p,
        Err(e) => {
            out(format!("error: {e}"));
            return 2;
        }
    };
    if let Some(problem) = check_params(
        &args.code,
        &Json::Object(params.clone()),
        catalog::messages(),
    ) {
        out(format!("error: {problem}"));
        return 2;
    }
    if !link.wait_connected(CONNECT_TIMEOUT) {
        out("error: Pitwall is not running".into());
        return 1;
    }
    let announcer = Announcer::new(link.clone(), || None, || 0.0);
    if !announcer.declare() {
        out("error: Pitwall did not accept the catalog (see the log)".into());
        return 1;
    }
    let mut body = json!({"code": args.code, "params": params});
    if let Some(d) = args.data.as_ref().filter(|d| !d.is_empty()) {
        if Json::Object(d.clone()).to_string().len() > MAX_DATA {
            out(format!("error: --data is over {MAX_DATA} bytes"));
            return 2;
        }
        body["data"] = Json::Object(d.clone());
    }
    match link.call("pitwall.announce", to_value(&body), None, ANNOUNCE_TIMEOUT) {
        Ok(res) => {
            let text = |k: &str| match codec::get(&res, k) {
                Some(Value::String(s)) => s.as_str().unwrap_or_default().to_owned(),
                _ => "None".into(),
            };
            out(format!("sent: {} ({})", text("message_id"), text("at")));
            0
        }
        Err(CallError::Remote {
            code,
            message,
            retry_ms,
        }) => {
            out(format!("refused: {code}: {message}"));
            if let Some(ms) = retry_ms {
                out(format!("retry in {} s", ms as f64 / 1000.0));
            }
            1
        }
        Err(e) => {
            out(format!("error: {e}"));
            1
        }
    }
}

/// `announce`: connects as `message-cli` and sends (the exit code is the process's).
pub fn announce_main(args: &[String], workspace: &Path, out: &mut dyn FnMut(String)) -> i32 {
    let parsed = match parse_announce(args) {
        Ok(a) => a,
        Err(e) => {
            out(format!("error: {e}"));
            return 2;
        }
    };
    let setup = (|| {
        let pipe = match &parsed.pipe {
            Some(p) => p.clone(),
            None => ams2_platform::dev::pipe_path(crate::link::PITWALL_ENDPOINT)
                .map_err(|e| e.to_string())?,
        };
        PitwallLink::new(
            &pipe,
            peer_info(MESSAGE_CLI),
            key_source(workspace).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())
    })();
    let link = match setup {
        Ok(l) => Arc::new(l),
        Err(e) => {
            out(format!("error: {e}"));
            return 1;
        }
    };
    link.start();
    let code = send(link.clone(), &parsed, out);
    link.close();
    code
}

/// `codes`: the catalog's codes with their audience, priority and params.
pub fn codes(out: &mut dyn FnMut(String)) {
    for code in catalog::codes() {
        let m = &catalog::messages()[code];
        let params: Vec<String> = m
            .get("params")
            .and_then(Json::as_object)
            .into_iter()
            .flatten()
            .map(|(n, spec)| {
                let kind = spec
                    .as_str()
                    .or_else(|| spec.get("kind").and_then(Json::as_str))
                    .unwrap_or("?");
                format!("{n}:{kind}")
            })
            .collect();
        let field = |k: &str| m.get(k).and_then(Json::as_str).unwrap_or("?");
        out(format!(
            "{code}  [{}, {}]  {}",
            field("audience"),
            field("priority"),
            params.join(", ")
        )
        .trim_end()
        .to_owned());
    }
}

/// `diagnose`: one consistent snapshot's main fields and the mapping size. `shm_name`: a mapping
/// other than the game's (tests). 0 ok, 1 no mapping, 2 unsupported version.
pub fn diagnose(shm_name: Option<&str>, out: &mut dyn FnMut(String)) -> i32 {
    let m = match shm_name {
        Some(n) => Mapping::open(n),
        None => Mapping::open_game(),
    };
    let m = match m {
        Ok(m) => m,
        Err(e) => {
            out(format!("error: {e}"));
            return 1;
        }
    };
    let mut d = new_frame();
    let seq = match m.snapshot_with(&mut d, 50) {
        SnapshotOutcome::Frame(s) => s,
        _ => {
            out("error: no consistent snapshot (the game is writing); try again".into());
            return 1;
        }
    };
    let text = |raw: &[u8]| decode_str(raw).into_owned();
    out(format!(
        "mapping: {} bytes (the v{SHARED_MEMORY_VERSION} struct is {FRAME_SIZE})",
        m.region_size()
    ));
    let ok = d.version == SHARED_MEMORY_VERSION;
    out(format!(
        "version: {} ({})",
        d.version,
        if ok { "supported" } else { "NOT supported" }
    ));
    out(format!("sequence: {seq}"));
    out(format!(
        "game state {}, session state {}, race state {}, pit mode {}",
        d.game_state, d.session_state, d.race_state, d.pit_mode
    ));
    out(format!(
        "car: {:?}  track: {:?} / {:?}  length: {:.1} m",
        text(&d.car_name),
        text(&d.track_location),
        text(&d.track_variation),
        d.track_length
    ));
    out(format!(
        "participants: {} (viewed {})  speed: {:.1} m/s  fuel: {:.1} / {:.1} l  lap time: {:.3} s",
        d.num_participants,
        d.viewed_participant_index,
        d.speed,
        d.fuel_level,
        d.fuel_capacity,
        d.current_time
    ));
    if ok { 0 } else { 2 }
}

/// `capture-raw FILE [--seconds N] [--shm-name NAME]` (dev builds): a raw capture for test
/// fixtures. Off in release builds (decision D13); never started by the control app.
#[cfg(feature = "capture-raw")]
pub fn capture_raw(args: &[String], out: &mut dyn FnMut(String)) -> i32 {
    use ams2_shm::capture::{CaptureWriter, RecordOptions, record};
    use std::sync::atomic::AtomicBool;
    let (mut file, mut seconds, mut shm) = (None, 600.0f64, None);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--seconds" => seconds = it.next().and_then(|v| v.parse().ok()).unwrap_or(seconds),
            "--shm-name" => shm = it.next().cloned(),
            s if !s.starts_with("--") && file.is_none() => file = Some(s.to_owned()),
            other => {
                out(format!("error: unexpected argument {other}"));
                return 2;
            }
        }
    }
    let Some(file) = file else {
        out("error: capture-raw needs a FILE".into());
        return 2;
    };
    let source = match &shm {
        Some(n) => Mapping::open(n),
        None => Mapping::open_game(),
    };
    let mut source = match source {
        Ok(s) => s,
        Err(e) => {
            out(format!("error: {e}"));
            return 1;
        }
    };
    let writer = std::fs::File::create(&file)
        .map(std::io::BufWriter::new)
        .map_err(ams2_shm::ShmError::from)
        .and_then(CaptureWriter::new);
    let mut writer = match writer {
        Ok(w) => w,
        Err(e) => {
            out(format!("error: {e}"));
            return 1;
        }
    };
    let stop = Arc::new(AtomicBool::new(false));
    let s = stop.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs_f64(seconds));
        s.store(true, std::sync::atomic::Ordering::Relaxed);
    });
    let opts = RecordOptions {
        interval: Duration::from_millis(2),
        max_frames: None,
    };
    match record(&mut source, &mut writer, &mut new_frame(), &opts, &stop)
        .and_then(|n| writer.finish().map(|_| n))
    {
        Ok(n) => {
            out(format!("{n} frames written to {file}"));
            0
        }
        Err(e) => {
            out(format!("error: {e}"));
            1
        }
    }
}
