//! The recorder: the 50 Hz loop, the game watch, the tracker, the detector, the sink and the
//! status (plan 1.3, `recorder/main.py`).
//!
//! The game is "running" when its process exists (never trusted to the mapping, which other
//! programs keep alive). The mapping is opened only then, and again when frames stop for 5 s
//! and the process is still there. Per tick: one snapshot, the tracker, the detector once per
//! new game frame, one clock read for the status. The status is built every 0.5 s and sent
//! only when it changed.

use crate::announcer::{Announcer, declare_connection};
use crate::link::{
    ControlLink, PITWALL_ENDPOINT, PitwallLink, SERVICE, StatusPublisher, Transport, key_source,
    peer_info,
};
use crate::live::{Hints, LiveBuffer, LiveServer, Meta};
use crate::serve::{ActionServer, capture_target};
use crate::sink::{CLOSE_DRAIN, PitwallSink};
use crate::telemetry::{Telemetry, mapping_opener};
use ams2_core::announce::EventDetector;
use ams2_core::garage;
use ams2_core::tracker::{Clock, SessionTracker};
use ams2_core::vocab;
use ams2_platform::game::GameWatch;
use ams2_platform::logging::local_iso;
use ams2_shm::{FrameSource, Mapping, SHARED_MEMORY_VERSION, SharedMemory, SnapshotOutcome};
use serde_json::{Value as Json, json};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use stintmate_ipc::{MethodOpts, naming};

/// Status compared this often.
pub const STATUS_CHECK: Duration = Duration::from_millis(500);
/// Frames stopped this long: ask whether the game is still there.
pub const STALE: Duration = Duration::from_secs(5);
/// The wait between attempts to map the game.
pub const WAIT_INTERVAL: Duration = Duration::from_secs(2);
/// The wait after an unsupported shared memory version.
pub const VERSION_MISMATCH_WAIT: Duration = Duration::from_secs(30);
/// `connector.status.error_code` of a version other than 14.
pub const ERROR_UNSUPPORTED: &str = "unsupported_sim_version";

/// Settings of one run.
pub struct Config {
    /// Sampling rate.
    pub hz: f64,
    /// The workspace (the key lives in `<workspace>\ipc\key`).
    pub workspace: PathBuf,
    /// `<workspace>\connectors\ams2`.
    pub data_dir: PathBuf,
    /// Pitwall's pipe (default: the current user's).
    pub pitwall_pipe: Option<String>,
    /// The control app's pipe (default: the current user's); `None` and `no_control`: none.
    pub control_pipe: Option<String>,
    /// No control link at all.
    pub no_control: bool,
    /// A mapping other than the game's (tests; the game's name is refused by the test writer).
    pub shm_name: Option<String>,
}

/// Injection points for tests.
#[derive(Default)]
pub struct Overrides {
    /// Replaces the process check.
    pub game: Option<GameWatch>,
    /// Replaces the tracker and detector clock: seconds, and a fixed ISO timestamp.
    pub clock: Option<(Arc<dyn Fn() -> f64 + Send + Sync>, String)>,
    /// Replaces the wall clock of the loop (staleness, status cadence).
    pub now: Option<Arc<dyn Fn() -> Instant + Send + Sync>>,
    /// Replaces "does the game's process exist?" for `connector.live_frame`.
    pub game_check: Option<Arc<dyn Fn() -> bool + Send + Sync>>,
}

/// Per-tick timings of the loop, for `tools/measure` (off unless enabled).
#[derive(Debug, Default, Clone)]
pub struct LoopStats {
    /// Seconds between the starts of consecutive ticks.
    pub periods: Vec<f64>,
    /// Seconds one tick took.
    pub ticks: Vec<f64>,
}

struct FnClock(Arc<dyn Fn() -> f64 + Send + Sync>, Option<String>);

impl Clock for FnClock {
    fn now(&self) -> f64 {
        (self.0)()
    }
    fn iso(&self) -> String {
        self.1.clone().unwrap_or_else(|| local_iso(false))
    }
}

/// `connector.status.live.session_type` from AMS2's session state name.
pub fn live_session_type(name: &str) -> &'static str {
    match name {
        "PRACTICE" | "TIME_ATTACK" => "practice",
        "TEST" => "test",
        "QUALIFY" => "qualifying",
        "FORMATION_LAP" | "RACE" => "race",
        _ => "other",
    }
}

/// The recorder (see the module docs).
pub struct Recorder {
    cfg: Config,
    link: Arc<PitwallLink>,
    control: Option<Arc<ControlLink>>,
    publisher: StatusPublisher,
    sink: PitwallSink,
    announcer: Arc<Announcer>,
    tracker: SessionTracker,
    detector: EventDetector,
    game: GameWatch,
    now: Arc<dyn Fn() -> Instant + Send + Sync>,
    source: Option<Box<dyn FrameSource + Send>>,
    frame: Box<SharedMemory>,
    have_frame: bool,
    game_running: bool,
    error: Option<&'static str>,
    pause_until: Option<Instant>,
    last_seq: Option<u32>,
    announced_seq: Option<u32>,
    last_change: Instant,
    last_check: Option<Instant>,
    dirty: Arc<AtomicBool>,
    key_cell: Arc<Mutex<Option<String>>>,
    last_sid: Option<u64>,
    started: Instant,
    /// +1 at every (re)mapping.
    pub map_generation: u64,
    live: Arc<LiveBuffer>,
    hints: Arc<Hints>,
    telemetry: Telemetry,
    stats: Option<LoopStats>,
}

impl Recorder {
    /// Builds everything and starts the writer and the outbox sender; the links are started by
    /// [`run`](Self::run) / [`start_links`](Self::start_links).
    pub fn build(cfg: Config, ov: Overrides) -> Result<Self, String> {
        let err = |e: &dyn std::fmt::Display| e.to_string();
        let key = key_source(&cfg.workspace).map_err(|e| err(&e))?;
        let pipe = match &cfg.pitwall_pipe {
            Some(p) => p.clone(),
            None => naming::pipe_path(PITWALL_ENDPOINT, None).map_err(|e| err(&e))?,
        };
        let link = Arc::new(PitwallLink::new(&pipe, peer_info(SERVICE), key).map_err(|e| err(&e))?);
        let control = if cfg.no_control {
            None
        } else {
            let pipe = match &cfg.control_pipe {
                Some(p) => p.clone(),
                None => naming::pipe_path(crate::link::APP_ID, None).map_err(|e| err(&e))?,
            };
            let key = key_source(&cfg.workspace).map_err(|e| err(&e))?;
            Some(Arc::new(
                ControlLink::new(&pipe, peer_info(SERVICE), key).map_err(|e| err(&e))?,
            ))
        };
        let publisher =
            StatusPublisher::new(&link, control.clone(), local_iso(true)).map_err(|e| err(&e))?;
        let dirty = Arc::new(AtomicBool::new(true));
        let d = dirty.clone();
        let transport: Arc<dyn Transport> = link.clone();
        let sink = PitwallSink::new(
            &cfg.data_dir.join("outbox"),
            transport.clone(),
            Some(Box::new(move || d.store(true, Ordering::Relaxed))),
        )
        .map_err(|e| err(&e))?;
        let (clock_fn, iso): (Arc<dyn Fn() -> f64 + Send + Sync>, Option<String>) = match ov.clock {
            Some((f, iso)) => (f, Some(iso)),
            None => {
                let t0 = Instant::now();
                (Arc::new(move || t0.elapsed().as_secs_f64()), None)
            }
        };
        let key_cell: Arc<Mutex<Option<String>>> = Arc::default();
        let kc = key_cell.clone();
        let announcer = Arc::new(Announcer::new(
            transport.clone(),
            move || kc.lock().ok().and_then(|g| g.clone()),
            {
                let cf = clock_fn.clone();
                move || cf()
            },
        ));
        let (l2, a2) = (link.clone(), announcer.clone());
        link.on_connected(move || declare_connection(l2.as_ref(), &a2));
        let a3 = announcer.clone();
        link.on_disconnected(move || a3.lost());
        let d = dirty.clone();
        link.on_state(move || d.store(true, Ordering::Relaxed));
        let now = ov
            .now
            .unwrap_or_else(|| Arc::new(Instant::now) as Arc<dyn Fn() -> Instant + Send + Sync>);
        let t = now();
        let live = Arc::new(LiveBuffer::default());
        let hints = Arc::new(Hints::default());
        let game_check = ov
            .game_check
            .unwrap_or_else(|| Arc::new(ams2_platform::game::game_running));
        let server = Arc::new(LiveServer {
            buffer: live.clone(),
            game_running: game_check,
            now: now.clone(),
        });
        let io = |e: stintmate_ipc::Error| e.to_string();
        link.client
            .method(
                "connector.live_frame",
                MethodOpts::default().inline(true),
                move |_ctx, _params| server.answer(),
            )
            .map_err(io)?;
        link.client
            .method(
                "connector.capture_target",
                MethodOpts::default(),
                |_ctx, _params| capture_target(),
            )
            .map_err(io)?;
        let actions = ActionServer::new(announcer.clone());
        link.client
            .method("connector.action", MethodOpts::default(), move |_ctx, p| {
                actions.call(&p)
            })
            .map_err(io)?;
        let telemetry = Telemetry::install(
            &link.client,
            hints.clone(),
            mapping_opener(cfg.shm_name.clone()),
        )
        .map_err(io)?;
        Ok(Self {
            live,
            hints,
            telemetry,
            stats: None,
            link,
            control,
            publisher,
            sink,
            announcer,
            tracker: SessionTracker::new(Box::new(FnClock(clock_fn.clone(), iso.clone()))),
            detector: EventDetector::new(FnClock(clock_fn, iso), 2.0),
            game: ov.game.unwrap_or_default(),
            now,
            source: None,
            frame: ams2_shm::new_frame(),
            have_frame: false,
            game_running: false,
            error: None,
            pause_until: None,
            last_seq: None,
            announced_seq: None,
            last_change: t,
            last_check: None,
            dirty,
            key_cell,
            last_sid: None,
            started: t,
            map_generation: 0,
            cfg,
        })
    }

    /// Starts the Pitwall link and the control link (they reconnect by themselves).
    pub fn start_links(&self) {
        self.link.start();
        if let Some(c) = &self.control {
            c.start();
        }
    }

    /// The Pitwall link.
    pub fn link(&self) -> &Arc<PitwallLink> {
        &self.link
    }

    /// The sink (outbox, counters, flush).
    pub fn sink(&self) -> &PitwallSink {
        &self.sink
    }

    /// The announcer.
    pub fn announcer(&self) -> &Arc<Announcer> {
        &self.announcer
    }

    /// The mapping is open.
    pub fn mapped(&self) -> bool {
        self.source.is_some()
    }

    /// Replaces the frame source (tests with a `MemorySource`).
    pub fn set_source(&mut self, source: Box<dyn FrameSource + Send>) {
        self.source = Some(source);
        self.map_generation += 1;
        self.last_change = (self.now)();
        self.hints
            .generation
            .store(self.map_generation, Ordering::Relaxed);
        self.hints.game_running.store(true, Ordering::Relaxed);
    }

    /// The mapping is closed: nothing to show, nothing to sample.
    fn drop_source(&mut self) {
        self.source = None;
        self.live.clear();
        self.hints.game_running.store(false, Ordering::Relaxed);
    }

    /// Per-tick timings (`tools/measure`): record the next `capacity` ticks.
    pub fn enable_stats(&mut self, capacity: usize) {
        self.stats = Some(LoopStats {
            periods: Vec::with_capacity(capacity),
            ticks: Vec::with_capacity(capacity),
        });
    }

    /// The timings recorded so far.
    pub fn loop_stats(&self) -> Option<&LoopStats> {
        self.stats.as_ref()
    }

    /// The telemetry stream (counters).
    pub fn telemetry(&self) -> &Telemetry {
        &self.telemetry
    }

    fn open_source(&self) -> Result<Box<dyn FrameSource + Send>, ams2_shm::ShmError> {
        Ok(match &self.cfg.shm_name {
            Some(n) => Box::new(Mapping::open(n)?),
            None => Box::new(Mapping::open_game()?),
        })
    }

    /// Maps the shared memory, but only if the game's process runs.
    pub fn connect(&mut self) {
        let now = (self.now)();
        if self.pause_until.is_some_and(|t| now < t) {
            return;
        }
        self.pause_until = None;
        self.error = None; // re-detected on the first snapshot if the version is still wrong
        self.game_running = self.game.running(true);
        if self.game_running
            && let Ok(src) = self.open_source()
        {
            tracing::info!("Automobilista 2 is running: shared memory mapped");
            self.set_source(src);
        }
        self.housekeeping(true);
    }

    fn game_gone(&mut self) {
        tracing::info!("Automobilista 2 is not running any more: session ended, mapping closed");
        self.tracker.end_session(&mut self.sink);
        self.drop_source();
        self.game_running = false;
        self.have_frame = false;
        self.announced_seq = None;
        self.last_seq = None;
        self.detector.reset();
        self.tracker.reset_garage();
        self.sync_key();
        self.housekeeping(true);
    }

    fn sync_key(&mut self) {
        let sid = self.tracker.session_id();
        if sid != self.last_sid {
            self.last_sid = sid;
            if let Ok(mut g) = self.key_cell.lock() {
                *g = self.sink.session_key(sid);
            }
        }
    }

    /// One iteration of the loop while mapped.
    pub fn tick(&mut self) {
        let now = (self.now)();
        if now.duration_since(self.last_change) >= STALE && self.game.due() {
            // No new frame for 5 s: normal in menus and while paused, or the game is gone.
            if !self.game.running(true) {
                self.game_gone();
                return;
            }
            match self.open_source() {
                Ok(src) => self.set_source(src),
                Err(_) => {
                    self.game_gone();
                    return;
                }
            }
        }
        let Some(src) = self.source.as_mut() else {
            return;
        };
        let mut new_frame = false;
        match src.snapshot(&mut self.frame) {
            Ok(SnapshotOutcome::Frame(seq)) => {
                self.have_frame = true;
                if self.last_seq != Some(seq) {
                    self.last_seq = Some(seq);
                    self.last_change = now;
                    new_frame = true;
                }
            }
            Ok(SnapshotOutcome::Torn) | Err(_) => return,
            Ok(SnapshotOutcome::Ended) => {
                self.drop_source();
                return;
            }
        }
        let (version, seq) = (self.frame.version, self.frame.sequence_number);
        if version != SHARED_MEMORY_VERSION {
            tracing::error!(
                "Unsupported shared memory version {version} (expected {SHARED_MEMORY_VERSION}); not recording"
            );
            self.tracker.end_session(&mut self.sink);
            self.drop_source();
            self.live.unsupported(version);
            self.error = Some(ERROR_UNSUPPORTED);
            self.pause_until = Some(now + VERSION_MISMATCH_WAIT);
            self.sync_key();
            self.housekeeping(true);
            return;
        }
        self.tracker.process(&self.frame, &mut self.sink);
        self.hints
            .in_garage
            .store(self.tracker.in_garage(), Ordering::Relaxed);
        self.hints
            .flying
            .store(self.tracker.lap_not_out(), Ordering::Relaxed);
        if new_frame {
            self.live.publish(
                &self.frame,
                Meta {
                    in_garage: self.tracker.in_garage(),
                    lap: self.tracker.live_lap(),
                    last_change: self.last_change,
                },
            );
        }
        self.sync_key();
        if self.announced_seq != Some(seq) {
            // the detector sees each game frame once
            self.announced_seq = Some(seq);
            for a in self.detector.update(&self.frame) {
                self.announcer.announce(&a);
            }
        }
        self.housekeeping(false);
    }

    /// The status, built every 0.5 s (or when forced or something changed) and sent only when it
    /// changed.
    pub fn housekeeping(&mut self, force: bool) {
        let now = (self.now)();
        let due = self
            .last_check
            .is_none_or(|t| now.duration_since(t) >= STATUS_CHECK);
        if force || due {
            self.last_check = Some(now);
            self.dirty.store(false, Ordering::Relaxed);
            let (status, extra) = (self.protocol_status(), self.service_extra());
            self.publisher.publish(&status, &extra);
        }
    }

    /// Runs until `stop()`: maps the game when it runs, ticks at `hz`, then shuts down.
    pub fn run(&mut self, stop: &dyn Fn() -> bool) {
        tracing::info!(
            "Recorder started (pid {}, {} Hz)",
            std::process::id(),
            self.cfg.hz
        );
        self.start_links();
        self.housekeeping(true);
        let period = Duration::from_secs_f64(1.0 / self.cfg.hz);
        let timer = ams2_platform::timer::WaitableTimer::new().ok();
        if let Some(t) = &timer {
            let _ = t.start_periodic(period);
        }
        let mut last_start: Option<Instant> = None;
        while !stop() {
            if self.source.is_none() {
                self.connect();
                if self.source.is_none() {
                    self.housekeeping(false);
                    let end = Instant::now() + WAIT_INTERVAL;
                    while Instant::now() < end && !stop() {
                        std::thread::sleep(Duration::from_millis(100));
                    }
                }
                continue;
            }
            if self.stats.is_some() {
                let t0 = Instant::now();
                self.tick();
                let dt = t0.elapsed().as_secs_f64();
                if let Some(s) = self.stats.as_mut() {
                    if let Some(prev) = last_start {
                        s.periods.push(t0.duration_since(prev).as_secs_f64());
                    }
                    s.ticks.push(dt);
                }
                last_start = Some(t0);
            } else {
                self.tick();
            }
            match &timer {
                Some(t) => {
                    t.wait_timeout(period * 4);
                }
                None => std::thread::sleep(period),
            }
        }
        tracing::info!("Stopping (stop requested)");
        self.shutdown();
    }

    /// Ends the session, stores what is queued, delivers what it can, closes the links.
    pub fn shutdown(&mut self) {
        self.telemetry.close();
        self.tracker.end_session(&mut self.sink);
        self.drop_source();
        self.sink.close(CLOSE_DRAIN);
        let left = self.sink.outbox_summary();
        if left.pending > 0 {
            tracing::info!(
                "{} write(s) left in the outbox for the next run",
                left.pending
            );
        }
        self.link.close();
        if let Some(c) = &self.control {
            c.close();
        }
    }

    // ---- status -------------------------------------------------------------------------

    fn phase(&self) -> Option<&'static str> {
        (self.source.is_some() && self.have_frame).then(|| {
            garage::phase(
                self.frame.game_state,
                self.frame.pit_mode,
                self.tracker.in_garage(),
            )
        })
    }

    fn message(&self) -> &'static str {
        if self.error == Some(ERROR_UNSUPPORTED) {
            return "unsupported shared memory version: not recording";
        }
        if !self.game_running {
            return "Automobilista 2 is not running";
        }
        if self.source.is_none() {
            return "waiting for the shared memory (is Shared Memory set to Project CARS 2?)";
        }
        if self.tracker.session_id().is_none() {
            return match self.phase() {
                Some("garage") => "in the garage (session not open yet)",
                Some("menus") => "in the menus",
                _ => "waiting for a session",
            };
        }
        if self.tracker.in_garage() {
            "in the garage (session open)"
        } else {
            "recording"
        }
    }

    fn live(&self) -> Json {
        let (Some(v), true) = (self.tracker.status_view(), self.source.is_some()) else {
            return Json::Null;
        };
        if v.car.is_empty() || v.track.is_empty() || v.layout.is_empty() {
            return Json::Null;
        }
        let len = f64::from(self.frame.track_length);
        json!({"session_type": live_session_type(&v.session_type), "car_id": v.car,
               "track_id": v.track, "layout_id": v.layout,
               "track_length_m": if len > 0.0 { json!(len) } else { Json::Null }})
    }

    /// `connector.status` v1 (contract 5.2).
    pub fn protocol_status(&self) -> Json {
        let view = self.tracker.status_view();
        let sid = self.tracker.session_id();
        json!({
            "game_running": self.game_running,
            "phase": self.phase(),
            "session_key": self.sink.session_key(sid),
            "stint": self.sink.stint(sid),
            "lap": view.as_ref().and_then(|v| v.lap_number),
            "live": self.live(),
            "outbox": self.sink.outbox_summary(),
            "error_code": self.error,
            "message": self.message(),
        })
    }

    /// What only the control app shows (`service.status`, contract 5.6).
    pub fn service_extra(&self) -> Json {
        let (state, message) = self.link.state();
        let stats = self.sink.stats();
        let sid = self.tracker.session_id();
        json!({
            "pitwall": state.as_str(), "pitwall_message": message,
            "mapped": self.source.is_some(), "in_garage": self.tracker.in_garage(),
            "session_open": sid.is_some(),
            "session_id": self.sink.remote_id(sid),
            "session_type": self.tracker.status_view()
                .map(|v| vocab::session_type_of_name(&v.session_type)),
            "laps_queued": stats.laps_queued, "laps_sent": stats.laps_sent,
            "laps_rejected": stats.laps_rejected,
            "telemetry": self.telemetry.active(),
            "uptime_s": (self.now)().duration_since(self.started).as_secs_f64().round(),
        })
    }
}
