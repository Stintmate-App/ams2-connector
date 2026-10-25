//! The recorder's writes: tracker records, then write API messages, then the outbox, then
//! Pitwall.
//!
//! The tracker hands its records to [`PitwallSink`] (the core's `Sink`). It queues them and, on
//! its own writer thread, builds the messages (`ams2-core`), turns each into the params of its
//! IPC method (`pitwall.write.<type>`, a lap's channels as `bin`) and stores it in the
//! [`Outbox`]. The 50 Hz loop never waits for any of it.
//!
//! Sessions are named by the connector's own **session key** (16 random hex digits chosen when
//! the tracker opens the session). A recorder stopped while the game runs (BUG-128) does not end
//! its session: it keeps the key and the write sequences in `session-resume.json` (next to the
//! outbox folder), and a recorder started later in the same game process, whose first session has
//! the same car, track, layout and session type and whose game clock continued (`ams2_shm::clock`:
//! remaining time not higher, laps completed not lower), continues it under the same key (Pitwall keeps
//! the session open across restarts and replays the first answer to `session_start`, so nothing
//! is announced twice). Anything else gets a new key. Each write has an idempotency key
//! `ams2/<session_key>/start|lap/<seq>|setup/<seq>|pit_exit/<seq>|participants/<seq>|end`,
//! `ams2/catalog/<sha256-12>` for a catalogue, assigned here and stored with the write.

use crate::link::{Transport, to_value};
use crate::outbox::{OnChange, Outbox, Summary};
use ams2_core::CONNECTOR_ID;
use ams2_core::announce::catalog::{canonical, sha12};
use ams2_core::channels::Row;
use ams2_core::messages::{
    self, Catalog, LapRecord, ParticipantLap, ParticipantLaps, PitExit, SessionEnd, SessionInfo,
};
use ams2_core::names::{self, Names};
use ams2_core::setup::{SetupSnapshot, setup_message};
use ams2_core::tracker::Sink;
use ams2_shm::SessionClock;
use std::collections::{BTreeSet, HashMap};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;
use stintmate_ipc::codec::{self, Value};

/// What the outbox may still deliver at shutdown (under the 10 s forced stop).
pub const CLOSE_DRAIN: Duration = Duration::from_secs(5);

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

/// 16 random hex digits (contract 4.2).
pub fn new_session_key() -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u128(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos()),
    );
    h.write_u32(std::process::id());
    format!("{:016x}", h.finish())
}

/// `ams2/catalog/<sha256-12 of the params>`: the same catalogue is one write, forever.
pub fn catalog_key(params: &serde_json::Value) -> String {
    format!("{CONNECTOR_ID}/catalog/{}", sha12(&canonical(params)))
}

#[derive(Default)]
struct Session {
    info: Option<SessionInfo>,
    key: String,
    seq_lap: u64,
    seq_setup: u64,
    seq_pit_exit: u64,
    seq_participants: u64,
    remote_id: Option<i64>,
    stint: Option<i64>,
}

#[derive(Default)]
struct Shared {
    next_local: u64,
    sessions: HashMap<u64, Session>,
    by_key: HashMap<String, u64>,
    cars: BTreeSet<String>,
}

/// Counters for `service.status`.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Stats {
    /// Laps stored in the outbox.
    pub laps_queued: u64,
    /// Laps Pitwall accepted.
    pub laps_sent: u64,
    /// Laps Pitwall refused.
    pub laps_rejected: u64,
}

enum Job {
    Start(u64),
    /// `(session, ended_at, clock)`: a clock stores the resume file, not a `session_end`.
    End(u64, String, Option<SessionClock>),
    Lap(LapRecord, Vec<Row>),
    Setup(u64, String, String, SetupSnapshot),
    PitExit(u64, String, Option<f64>),
    Participants(u64, Vec<ParticipantLap>),
    Split(String, Vec<Value>),
    Mark(Sender<()>),
    Quit,
}

struct Core {
    dir: std::path::PathBuf,
    shared: Mutex<Shared>,
    stats: Mutex<Stats>,
    outbox: Mutex<Option<Outbox>>,
    queue: Mutex<Sender<Job>>,
    /// Where an open session is kept for the next recorder of the same game process.
    resume_path: std::path::PathBuf,
    /// Identity of the running game process (its pid), when known.
    game_token: Mutex<Option<u64>>,
    /// The next `end_session` keeps the session open instead of closing it.
    /// Set when the recorder stops while the game runs: the clock of its last frame.
    stop_clock: Mutex<Option<SessionClock>>,
    /// The clock of the frame that opens a session (set by the recorder while none is open).
    start_clock: Mutex<Option<SessionClock>>,
}

/// What a stopped recorder leaves for the next one.
fn resume_json(token: u64, s: &Session, clock: &SessionClock) -> Option<serde_json::Value> {
    let i = s.info.as_ref()?;
    Some(serde_json::json!({
        "game": token, "clock": clock.to_json(), "key": s.key, "car": i.car, "track": i.track,
        "variation": i.variation, "session_state": i.session_state,
        "seq": [s.seq_lap, s.seq_setup, s.seq_pit_exit, s.seq_participants],
    }))
}

/// The key and sequences stored by [`resume_json`] when they describe `info` in game process
/// `token` and the stored session clock can continue as `now` (`SessionClock::continues_as`: a
/// session that began again, with the clock back at the total or fewer laps, is another one).
/// The file is always consumed.
fn take_resume(
    path: &std::path::Path,
    token: Option<u64>,
    info: &SessionInfo,
    now: Option<&SessionClock>,
) -> Option<(String, [u64; 4])> {
    let text = std::fs::read_to_string(path).ok()?;
    let _ = std::fs::remove_file(path);
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    let text_of = |k: &str| v.get(k).and_then(serde_json::Value::as_str);
    let clock_ok = match (v.get("clock").and_then(SessionClock::from_json), now) {
        (Some(stored), Some(now)) => stored.continues_as(now),
        _ => false,
    };
    let same = token.is_some()
        && clock_ok
        && v.get("game").and_then(serde_json::Value::as_u64) == token
        && text_of("car") == Some(info.car.as_str())
        && text_of("track") == Some(info.track.as_str())
        && text_of("variation") == Some(info.variation.as_str())
        && text_of("session_state") == Some(info.session_state.as_str());
    let key = text_of("key").filter(|k| same && !k.is_empty())?;
    let seq = v.get("seq")?.as_array()?;
    let mut n = [0u64; 4];
    for (slot, x) in n.iter_mut().zip(seq) {
        *slot = x.as_u64()?;
    }
    Some((key.to_owned(), n))
}

/// The tracker's sink in the recorder (see the module docs).
pub struct PitwallSink {
    core: Arc<Core>,
    writer: Option<std::thread::JoinHandle<()>>,
    sender: Option<std::thread::JoinHandle<()>>,
}

impl PitwallSink {
    /// Starts the writer thread and the outbox in `outbox_dir`.
    pub fn new(
        outbox_dir: &std::path::Path,
        link: Arc<dyn Transport>,
        on_change: Option<OnChange>,
    ) -> std::io::Result<Self> {
        let (tx, rx) = channel();
        let core = Arc::new(Core {
            dir: outbox_dir.to_owned(),
            shared: Mutex::default(),
            stats: Mutex::default(),
            outbox: Mutex::new(None),
            queue: Mutex::new(tx),
            resume_path: outbox_dir.with_file_name("session-resume.json"),
            game_token: Mutex::new(None),
            stop_clock: Mutex::new(None),
            start_clock: Mutex::new(None),
        });
        let (c1, c2) = (core.clone(), core.clone());
        let on_result: crate::outbox::OnResult = Box::new(move |m, p, r| c1.delivered(m, p, r));
        let on_rejected: crate::outbox::OnRejected =
            Box::new(move |m, p, code| c2.rejected(m, p, code));
        let (outbox, sender) = Outbox::start(
            outbox_dir,
            link,
            Some(on_result),
            Some(on_rejected),
            on_change,
        )?;
        *lock(&core.outbox) = Some(outbox);
        let c3 = core.clone();
        let writer = std::thread::Builder::new()
            .name("pitwall-writer".into())
            .spawn(move || c3.run(rx))?;
        Ok(Self {
            core,
            writer: Some(writer),
            sender: Some(sender),
        })
    }

    /// The outbox (status, tests).
    pub fn outbox(&self) -> Outbox {
        lock(&self.core.outbox).clone().expect("started")
    }

    /// The outbox folder.
    pub fn outbox_dir(&self) -> std::path::PathBuf {
        self.core.dir.clone()
    }

    /// `connector.status.outbox`.
    pub fn outbox_summary(&self) -> Summary {
        self.outbox().summary()
    }

    /// Counters.
    pub fn stats(&self) -> Stats {
        *lock(&self.core.stats)
    }

    /// Names the running game process (its pid); a resumed session needs it to match.
    pub fn set_game_token(&self, token: Option<u64>) {
        *lock(&self.core.game_token) = token;
    }

    /// The next `end_session` (the recorder is stopping while the game runs) keeps the session
    /// open for the next recorder instead of sending `session_end`; `clock` is the game's
    /// session clock at the last frame.
    pub fn keep_session_open(&self, clock: SessionClock) {
        *lock(&self.core.stop_clock) = Some(clock);
    }

    /// The game's session clock at the frame being processed; the recorder calls it only while
    /// no session is open, so a session opened by that frame knows where the clock stood.
    pub fn note_clock(&self, clock: SessionClock) {
        *lock(&self.core.start_clock) = Some(clock);
    }

    /// The session key of a tracker session.
    pub fn session_key(&self, local: Option<u64>) -> Option<String> {
        let sh = lock(&self.core.shared);
        sh.sessions.get(&local?).map(|s| s.key.clone())
    }

    /// Pitwall's session id of a tracker session, once Pitwall answered a write of it.
    pub fn remote_id(&self, local: Option<u64>) -> Option<i64> {
        lock(&self.core.shared).sessions.get(&local?)?.remote_id
    }

    /// Pitwall's stint of a tracker session as last answered.
    pub fn stint(&self, local: Option<u64>) -> Option<i64> {
        lock(&self.core.shared).sessions.get(&local?)?.stint
    }

    /// Waits until every queued record is stored in the outbox (not yet delivered).
    pub fn queued(&self, timeout: Duration) -> bool {
        let (tx, rx) = channel();
        self.core.send(Job::Mark(tx));
        rx.recv_timeout(timeout).is_ok()
    }

    /// Waits until everything queued is stored and delivered (tests).
    pub fn flush(&self, timeout: Duration) -> bool {
        let t0 = std::time::Instant::now();
        self.queued(timeout) && self.outbox().flush(timeout.saturating_sub(t0.elapsed()))
    }

    /// Stores what is queued, lets the outbox deliver what it can within `drain`, stops both
    /// threads. The rest stays on disk.
    pub fn close(&mut self, drain: Duration) {
        self.core.send(Job::Quit);
        if let Some(h) = self.writer.take() {
            let _ = h.join();
        }
        self.outbox().close(drain);
        if let Some(h) = self.sender.take() {
            let _ = h.join();
        }
    }
}

impl Drop for PitwallSink {
    fn drop(&mut self) {
        if self.writer.is_some() {
            self.close(Duration::ZERO);
        }
    }
}

impl Sink for PitwallSink {
    fn start_session(&mut self, info: &SessionInfo) -> u64 {
        let local = {
            let mut sh = lock(&self.core.shared);
            sh.next_local += 1;
            let local = sh.next_local;
            let token = *lock(&self.core.game_token);
            let now = *lock(&self.core.start_clock);
            let (key, seq) = take_resume(&self.core.resume_path, token, info, now.as_ref())
                .unwrap_or_else(|| (new_session_key(), [0; 4]));
            sh.by_key.insert(key.clone(), local);
            sh.sessions.insert(
                local,
                Session {
                    info: Some(info.clone()),
                    key,
                    seq_lap: seq[0],
                    seq_setup: seq[1],
                    seq_pit_exit: seq[2],
                    seq_participants: seq[3],
                    ..Session::default()
                },
            );
            local
        };
        self.core.send(Job::Start(local));
        local
    }

    fn end_session(&mut self, session_id: u64, ended_at: &str) {
        let keep = *lock(&self.core.stop_clock);
        self.core
            .send(Job::End(session_id, ended_at.to_owned(), keep));
    }

    fn insert_lap(&mut self, record: LapRecord, rows: Vec<Row>) {
        self.core.send(Job::Lap(record, rows));
    }

    fn insert_setup(
        &mut self,
        session_id: u64,
        taken_at: &str,
        reason: &str,
        snapshot: SetupSnapshot,
    ) {
        self.core.send(Job::Setup(
            session_id,
            taken_at.to_owned(),
            reason.to_owned(),
            snapshot,
        ));
    }

    fn insert_pit_exit(&mut self, session_id: u64, at: &str, fuel_l: Option<f64>) {
        self.core
            .send(Job::PitExit(session_id, at.to_owned(), fuel_l));
    }

    fn insert_participant_laps(&mut self, session_id: u64, rows: Vec<ParticipantLap>) {
        self.core.send(Job::Participants(session_id, rows));
    }
}

fn with(mut params: Value, extra: Vec<(&str, Value)>) -> Value {
    if let Value::Map(m) = &mut params {
        for (k, v) in extra {
            m.push((Value::from(k), v));
        }
    }
    params
}

impl Core {
    fn send(&self, job: Job) {
        let _ = lock(&self.queue).send(job);
    }

    fn outbox(&self) -> Outbox {
        lock(&self.outbox).clone().expect("started")
    }

    fn run(&self, rx: Receiver<Job>) {
        for job in rx {
            let quit = matches!(job, Job::Quit);
            if let Job::Mark(done) = job {
                let _ = done.send(());
                continue;
            }
            if !quit {
                self.handle(job);
            }
            if quit {
                return;
            }
        }
    }

    fn put(&self, kind: &str, ik: &str, params: Value) {
        if let Err(e) = self
            .outbox()
            .put(&format!("pitwall.write.{kind}"), ik, params)
        {
            tracing::error!("Cannot store {kind} {ik} in the outbox: {e} (dropped)");
        }
    }

    fn catalog(&self, cat: &Catalog) {
        let json = serde_json::to_value(cat).expect("a catalog serializes");
        self.put("catalog", &catalog_key(&json), to_value(cat));
    }

    /// Takes the next sequence number of `kind` for a session and returns it with the key.
    fn next(&self, local: u64, kind: &str) -> Option<(String, u64)> {
        let mut sh = lock(&self.shared);
        let s = sh.sessions.get_mut(&local)?;
        let seq = match kind {
            "lap" => &mut s.seq_lap,
            "setup" => &mut s.seq_setup,
            "pit_exit" => &mut s.seq_pit_exit,
            _ => &mut s.seq_participants,
        };
        *seq += 1;
        Some((s.key.clone(), *seq))
    }

    fn handle(&self, job: Job) {
        match job {
            Job::Start(local) => {
                let (info, key) = {
                    let sh = lock(&self.shared);
                    let Some(s) = sh.sessions.get(&local) else {
                        return;
                    };
                    (s.info.clone().expect("set at start"), s.key.clone())
                };
                let (cat, start) = messages::session_start_message(&info, Names::embedded());
                if let Some(cat) = cat {
                    lock(&self.shared)
                        .cars
                        .extend(cat.cars.iter().map(|c| c.car_id.clone()));
                    self.catalog(&cat);
                }
                let params = with(
                    to_value(&start),
                    vec![("session_key", Value::from(key.as_str()))],
                );
                self.put(
                    "session_start",
                    &format!("{CONNECTOR_ID}/{key}/start"),
                    params,
                );
                tracing::info!("Session {key} queued for Pitwall (recorder session {local})");
            }
            Job::End(local, ended_at, keep_clock) => {
                let (key, resume) = {
                    let sh = lock(&self.shared);
                    let Some(s) = sh.sessions.get(&local) else {
                        return;
                    };
                    let token = *lock(&self.game_token);
                    (
                        s.key.clone(),
                        token
                            .zip(keep_clock)
                            .and_then(|(t, c)| resume_json(t, s, &c)),
                    )
                };
                if let Some(r) = resume {
                    match std::fs::write(&self.resume_path, r.to_string()) {
                        Ok(()) => {
                            tracing::info!("Session {key} kept open for the next recorder");
                            return;
                        }
                        Err(e) => tracing::warn!("Cannot keep session {key} open: {e}"),
                    }
                }
                let params = with(
                    to_value(&SessionEnd { ended_at }),
                    vec![("session_key", Value::from(key.as_str()))],
                );
                self.put("session_end", &format!("{CONNECTOR_ID}/{key}/end"), params);
            }
            Job::Lap(record, rows) => {
                let Some((key, seq)) = self.next(record.session_id, "lap") else {
                    return;
                };
                let msg = messages::lap_message(&record, &rows);
                let params = with(
                    to_value(&msg),
                    vec![
                        ("session_key", Value::from(key.as_str())),
                        ("seq", Value::from(seq)),
                    ],
                );
                self.put("lap", &format!("{CONNECTOR_ID}/{key}/lap/{seq}"), params);
                lock(&self.stats).laps_queued += 1;
                tracing::info!(
                    "Lap {} ({}, {} samples) stored in the outbox ({key}/lap/{seq})",
                    record.lap_number,
                    messages::lap_type(&record),
                    rows.len()
                );
            }
            Job::Setup(local, taken_at, reason, snapshot) => {
                let Some((key, seq)) = self.next(local, "setup") else {
                    return;
                };
                let params = with(
                    to_value(&setup_message(&snapshot, &taken_at, &reason)),
                    vec![
                        ("session_key", Value::from(key.as_str())),
                        ("seq", Value::from(seq)),
                    ],
                );
                self.put(
                    "setup",
                    &format!("{CONNECTOR_ID}/{key}/setup/{seq}"),
                    params,
                );
            }
            Job::PitExit(local, at, fuel_l) => {
                let Some((key, seq)) = self.next(local, "pit_exit") else {
                    return;
                };
                let params = with(
                    to_value(&PitExit { at, fuel_l }),
                    vec![
                        ("session_key", Value::from(key.as_str())),
                        ("seq", Value::from(seq)),
                    ],
                );
                self.put(
                    "pit_exit",
                    &format!("{CONNECTOR_ID}/{key}/pit_exit/{seq}"),
                    params,
                );
            }
            Job::Participants(local, rows) => {
                if rows.is_empty() || !lock(&self.shared).sessions.contains_key(&local) {
                    return;
                }
                let new: BTreeSet<(String, Option<String>)> = {
                    let sh = lock(&self.shared);
                    rows.iter()
                        .filter_map(|r| {
                            let id = r.car_id.clone()?;
                            (!sh.cars.contains(&id)).then(|| (id, r.class_id.clone()))
                        })
                        .collect()
                };
                if !new.is_empty() {
                    let cars: Vec<(String, Option<String>)> = new.into_iter().collect();
                    lock(&self.shared)
                        .cars
                        .extend(cars.iter().map(|(c, _)| c.clone()));
                    self.catalog(&names::catalog(&[], &cars, Names::embedded()));
                }
                let batch = to_value(&ParticipantLaps { laps: rows });
                let laps = codec::get(&batch, "laps")
                    .and_then(Value::as_array)
                    .cloned();
                self.participants(local, laps.unwrap_or_default());
            }
            Job::Split(key, laps) => {
                let Some(local) = lock(&self.shared).by_key.get(&key).copied() else {
                    return;
                };
                for row in laps {
                    self.participants(local, vec![row]);
                }
            }
            Job::Mark(_) | Job::Quit => {}
        }
    }

    fn participants(&self, local: u64, laps: Vec<Value>) {
        let Some((key, seq)) = self.next(local, "participants") else {
            return;
        };
        let params = codec::map([
            ("laps", Value::Array(laps)),
            ("session_key", Value::from(key.as_str())),
            ("seq", Value::from(seq)),
        ]);
        self.put(
            "participant_laps",
            &format!("{CONNECTOR_ID}/{key}/participants/{seq}"),
            params,
        );
    }

    fn delivered(&self, method: &str, params: &Value, result: &Value) {
        let key = codec::get(params, "session_key").and_then(Value::as_str);
        let mut sh = lock(&self.shared);
        let local = key.and_then(|k| sh.by_key.get(k).copied());
        if let Some(s) = local.and_then(|l| sh.sessions.get_mut(&l)) {
            if let Some(id) = codec::get(result, "session_id").and_then(Value::as_i64) {
                s.remote_id = Some(id);
            }
            if let Some(st) = codec::get(result, "stint").and_then(Value::as_i64) {
                s.stint = Some(st);
            }
        }
        drop(sh);
        if method == "pitwall.write.lap" {
            lock(&self.stats).laps_sent += 1;
        }
    }

    fn rejected(&self, method: &str, params: &Value, code: &str) {
        if method == "pitwall.write.lap" {
            lock(&self.stats).laps_rejected += 1;
        }
        if method != "pitwall.write.participant_laps" || code != "bad_request" {
            return;
        }
        let laps = codec::get(params, "laps")
            .and_then(Value::as_array)
            .cloned();
        let key = codec::get(params, "session_key").and_then(Value::as_str);
        if let (Some(laps), Some(key)) = (laps, key)
            && laps.len() > 1
        {
            // One odd row (a name with control characters) must not lose the batch.
            tracing::warn!(
                "Rival batch refused; queueing its {} rows one by one",
                laps.len()
            );
            self.send(Job::Split(key.to_owned(), laps));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::link::CallError;

    fn contains(hay: &[u8], needle: &[u8]) -> bool {
        hay.windows(needle.len()).any(|w| w == needle)
    }

    /// A Pitwall that is never there: the writes stay in the outbox.
    struct Down;

    impl Transport for Down {
        fn connected(&self) -> bool {
            false
        }
        fn wait_connected(&self, _: Duration) -> bool {
            false
        }
        fn call(
            &self,
            _: &str,
            _: Value,
            _: Option<&str>,
            _: Duration,
        ) -> Result<Value, CallError> {
            Err(CallError::NotConnected)
        }
        fn call_async(
            &self,
            _: &str,
            _: Value,
            _: Duration,
            _: Box<dyn FnOnce(Result<Value, CallError>) + Send>,
        ) -> Result<(), CallError> {
            Err(CallError::NotConnected)
        }
    }

    fn info(state: &str) -> SessionInfo {
        SessionInfo {
            started_at: "2026-10-25T10:00:00.000+02:00".into(),
            car: "Formula Ultimate Gen2".into(),
            car_class: "F-Ultimate".into(),
            track: "Oulton Park".into(),
            variation: "Fosters".into(),
            session_state: state.into(),
            length_m: Some(4300.0),
            fuel_capacity_l: Some(100.0),
            ambient_c: None,
            track_c: None,
            rain: Some(0.5),
            wind_speed: None,
        }
    }

    /// A recorder of game process `token`; its first session is `state`.
    fn recorder(dir: &std::path::Path, token: u64) -> PitwallSink {
        let sink = PitwallSink::new(&dir.join("outbox"), Arc::new(Down), None).unwrap();
        sink.set_game_token(Some(token));
        sink
    }

    /// The game's clock in a 30 min practice: `left` s to go, `laps` completed.
    fn clock(left: f32, laps: u32) -> SessionClock {
        SessionClock {
            game_state: 2,
            session_state: 1,
            identity: 7,
            time_remaining_s: left,
            laps_in_event: 0,
            duration_min: 30.0,
            laps_completed: laps,
        }
    }

    fn open(sink: &mut PitwallSink, state: &str) -> String {
        open_at(sink, state, clock(1500.0, 3))
    }

    fn open_at(sink: &mut PitwallSink, state: &str, at: SessionClock) -> String {
        sink.note_clock(at);
        let id = sink.start_session(&info(state));
        sink.session_key(Some(id)).unwrap()
    }

    /// Stops a recorder; `game_runs`: the game goes on (the session is kept open).
    fn stop(mut sink: PitwallSink, id: u64, game_runs: bool) -> usize {
        if game_runs {
            sink.keep_session_open(clock(1500.0, 3));
        }
        sink.end_session(id, "2026-10-25T10:05:00.000+02:00");
        assert!(sink.queued(Duration::from_secs(5)));
        let pending = sink.outbox_summary().pending;
        sink.close(Duration::ZERO);
        pending
    }

    #[test]
    fn a_restarted_recorder_in_the_same_game_session_keeps_the_key_and_sends_no_end() {
        let dir = tempfile::tempdir().unwrap();
        let mut a = recorder(dir.path(), 4242);
        let key = open(&mut a, "PRACTICE");
        a.insert_pit_exit(1, "t", None);
        assert!(a.queued(Duration::from_secs(5)));
        let before = a.outbox_summary().pending;
        assert_eq!(
            stop(a, 1, true),
            before,
            "no session_end while the game runs"
        );

        let mut b = recorder(dir.path(), 4242);
        assert_eq!(open(&mut b, "PRACTICE"), key, "same key after the restart");
        b.insert_pit_exit(1, "t", None);
        assert!(b.queued(Duration::from_secs(5)));
        // the pit exit continues the sequence (a repeat of seq 1 would be replayed by Pitwall)
        let pending = b.outbox_summary().pending;
        assert_eq!(pending, before + 3); // catalogue and start again (Pitwall replays them), the exit
        let ik = std::fs::read_dir(dir.path().join("outbox"))
            .unwrap()
            .filter_map(|e| std::fs::read(e.ok()?.path()).ok())
            .filter(|t| contains(t, format!("{key}/pit_exit/2").as_bytes()))
            .count();
        assert_eq!(ik, 1, "the second pit exit is seq 2");
        b.close(Duration::ZERO);
        assert!(!dir.path().join("session-resume.json").exists());
    }

    #[test]
    fn a_new_game_session_gets_a_new_key() {
        for (token, state) in [(7, "PRACTICE"), (4242, "QUALIFY"), (4242, "RACE")] {
            let dir = tempfile::tempdir().unwrap();
            let mut a = recorder(dir.path(), 4242);
            let key = open(&mut a, "PRACTICE");
            stop(a, 1, true);
            let mut b = recorder(dir.path(), token);
            let other = open(&mut b, state);
            assert_ne!(other, key, "{token} {state}");
            b.close(Duration::ZERO);
        }
    }

    #[test]
    fn a_session_ended_for_good_is_not_resumed() {
        let dir = tempfile::tempdir().unwrap();
        let mut a = recorder(dir.path(), 4242);
        let key = open(&mut a, "PRACTICE");
        let before = a.outbox_summary().pending;
        assert!(
            stop(a, 1, false) > before,
            "the game left: session_end is queued"
        );
        assert!(!dir.path().join("session-resume.json").exists());
        let mut b = recorder(dir.path(), 4242);
        assert_ne!(open(&mut b, "PRACTICE"), key);
        b.close(Duration::ZERO);
    }

    #[test]
    fn an_unknown_game_process_never_resumes() {
        let dir = tempfile::tempdir().unwrap();
        let mut a = recorder(dir.path(), 4242);
        let key = open(&mut a, "PRACTICE");
        stop(a, 1, true);
        let mut b = PitwallSink::new(&dir.path().join("outbox"), Arc::new(Down), None).unwrap();
        assert_ne!(open(&mut b, "PRACTICE"), key);
        b.close(Duration::ZERO);
    }

    #[test]
    fn the_same_session_after_a_long_stop_keeps_the_key() {
        let dir = tempfile::tempdir().unwrap();
        let mut a = recorder(dir.path(), 4242);
        let key = open(&mut a, "PRACTICE");
        stop(a, 1, true);
        // however long the recorder was off, the game's clock went on
        let mut b = recorder(dir.path(), 4242);
        let again = open_at(&mut b, "PRACTICE", clock(700.0, 9));
        assert_eq!(again, key, "the clock continued: the same session");
        b.close(Duration::ZERO);
    }

    #[test]
    fn a_new_session_of_the_same_kind_gets_a_new_key() {
        // clock back at the total, laps back to 0, or the time unset: another session
        for (left, laps) in [(1800.0, 0), (1500.0, 0), (1800.0, 3), (-1.0, 3)] {
            let dir = tempfile::tempdir().unwrap();
            let mut a = recorder(dir.path(), 4242);
            let key = open(&mut a, "PRACTICE");
            stop(a, 1, true);
            let mut b = recorder(dir.path(), 4242);
            let other = open_at(&mut b, "PRACTICE", clock(left, laps));
            assert_ne!(other, key, "{left} s left, {laps} laps");
            b.close(Duration::ZERO);
        }
    }

    #[test]
    fn a_session_opened_without_a_clock_is_never_a_resume() {
        let dir = tempfile::tempdir().unwrap();
        let mut a = recorder(dir.path(), 4242);
        let key = open(&mut a, "PRACTICE");
        stop(a, 1, true);
        let mut b = recorder(dir.path(), 4242);
        let id = b.start_session(&info("PRACTICE"));
        assert_ne!(b.session_key(Some(id)).unwrap(), key);
        b.close(Duration::ZERO);
    }
}
