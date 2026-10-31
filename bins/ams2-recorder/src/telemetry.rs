//! `connector.telemetry` v1 (contract 5.5): the sampling thread.
//!
//! Produced only while Pitwall subscribes. The thread reads the 4-byte sequence number of its
//! own read-only view of the mapping once per expected game frame (phase-locked: it sleeps
//! until the next frame is due, polls every 0.25 ms once it is due and not there, every 5 ms with
//! no frame for two periods) and reads the dozen fields it needs straight from the view. Rules:
//!
//! * `t_ns` is taken right after reading a changed sequence number, never at send time;
//! * a sample goes out only for a new game frame, at most `rate_hz` per second; with no new
//!   frame (menus, pause, no game) one repeat with `fresh` = 0 every 250 ms keeps `game_state`
//!   current;
//! * the subscription option `o: {rate_hz}` (10-200) lowers the cap, else `bad_request`.
//!
//! The thread never touches the loop's buffers: it has its own mapping, reopened when the
//! loop reopened its own, and reads the lap state from [`Hints`].

use crate::live::Hints;
use ams2_core::telemetry::{
    CHANNEL, Fields, RATE_HZ, RATE_MAX, RATE_MIN, SAMPLE_SIZE, idle_values, sample_values,
};
use ams2_platform::timer::WaitableTimer;
use ams2_shm::{Mapping, ParticipantInfo, SharedMemory};
use std::mem::{offset_of, size_of};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::Duration;
use stintmate_ipc::blocking::Client;
use stintmate_ipc::clock::now_ns;
use stintmate_ipc::codec::{self, Value};
use stintmate_ipc::error::RemoteError;
use stintmate_ipc::layout::{FieldType, Layout};
use stintmate_ipc::{Error, StreamChannel};

/// The sequence number while no frame is expected (menus, pause).
const POLL: f64 = 0.005;
/// Once the next frame is due and not there yet.
const DUE_POLL: f64 = 0.000_25;
/// Wake this long after the frame is expected.
const LATE: f64 = 0.0003;
/// The expected phase moves this much earlier per frame (re-locks every few frames).
const NUDGE: f64 = 0.0003;
/// The first guess of the game's frame period.
const PERIOD: f64 = 1.0 / 90.0;
const PERIOD_MIN: f64 = 0.004;
const PERIOD_MAX: f64 = 0.05;
/// A repeat (`fresh` = 0) while no new frame comes.
pub const KEEPALIVE: f64 = 0.25;
/// No mapping: how often to look again.
const REOPEN_WAIT: f64 = 0.5;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

/// The layout of a sample (32 bytes).
pub fn layout() -> Layout {
    use FieldType::{F32, U8, U16, U32};
    Layout::new(
        [
            ("lap_distance_m", F32, 1),
            ("speed_ms", F32, 1),
            ("accel_long_ms2", F32, 1),
            ("track_length_m", F32, 1),
            ("lap_time_s", F32, 1),
            ("frame", U32, 1),
            ("sim_lap", U16, 1),
            ("session_type", U8, 1),
            ("game_state", U8, 1),
            ("flags", U16, 1),
            ("reserved", U16, 1),
        ],
        Some("telemetry"),
    )
    .expect("the telemetry layout is valid")
}

/// What the sampling thread reads from: the sequence number and the sample's fields.
pub trait View: Send {
    /// The sequence number (odd while the game writes).
    fn sequence(&self) -> u32;
    /// The fields of a sample, read straight from the mapping.
    fn fields(&self) -> Fields;
}

impl View for Mapping {
    fn sequence(&self) -> u32 {
        Mapping::sequence(self)
    }

    fn fields(&self) -> Fields {
        let viewed: i32 = self.read_field(offset_of!(SharedMemory, viewed_participant_index));
        let num: i32 = self.read_field(offset_of!(SharedMemory, num_participants));
        let accel_z: f32 = self.read_field(offset_of!(SharedMemory, local_acceleration) + 8);
        let valid =
            usize::try_from(viewed).is_ok_and(|i| i < usize::try_from(num).unwrap_or(0).min(64));
        let (lap_distance, current_lap) = if valid {
            let base = offset_of!(SharedMemory, participant_info)
                + viewed as usize * size_of::<ParticipantInfo>();
            (
                self.read_field(base + offset_of!(ParticipantInfo, current_lap_distance)),
                self.read_field(base + offset_of!(ParticipantInfo, current_lap)),
            )
        } else {
            (0.0, 0)
        };
        Fields {
            viewed,
            num_participants: num,
            track_length: self.read_field(offset_of!(SharedMemory, track_length)),
            current_time: self.read_field(offset_of!(SharedMemory, current_time)),
            game_state: self.read_field(offset_of!(SharedMemory, game_state)),
            session_state: self.read_field(offset_of!(SharedMemory, session_state)),
            pit_mode: self.read_field(offset_of!(SharedMemory, pit_mode)),
            lap_invalidated: self.read_field::<u8>(offset_of!(SharedMemory, lap_invalidated)) != 0,
            speed: self.read_field(offset_of!(SharedMemory, speed)),
            accel_z,
            lap_distance,
            current_lap,
        }
    }
}

/// How the thread opens its view.
pub type Opener = Box<dyn Fn() -> Option<Box<dyn View>> + Send + Sync>;

/// The opener of a real mapping: the game's, or a test mapping by name.
pub fn mapping_opener(name: Option<String>) -> Opener {
    Box::new(move || {
        let m = match &name {
            Some(n) => Mapping::open(n),
            None => Mapping::open_game(),
        };
        m.ok().map(|m| Box::new(m) as Box<dyn View>)
    })
}

/// Counters of the stream.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct TelemetryStats {
    /// Samples sent.
    pub sent: u64,
    /// Reads discarded because the frame changed under them.
    pub torn: u64,
    /// Wake-ups of the thread.
    pub wakes: u64,
}

struct Inner {
    channel: StreamChannel,
    hints: Arc<Hints>,
    open: Opener,
    active: AtomicBool,
    stop: AtomicBool,
    rate_bits: AtomicU64,
    frame: AtomicU32,
    sent: AtomicU64,
    torn: AtomicU64,
    wakes: AtomicU64,
    thread: Mutex<(bool, Option<JoinHandle<()>>)>,
}

/// The stream's producer: declares the channel and runs the thread while subscribed.
#[derive(Clone)]
pub struct Telemetry(Arc<Inner>);

impl Telemetry {
    /// Declares `connector.telemetry` (`rate_hz` 100) on `client` and hooks its subscription.
    /// Declare before the client starts.
    pub fn install(client: &Client, hints: Arc<Hints>, open: Opener) -> Result<Self, Error> {
        let channel =
            client.stream_channel_with(CHANNEL, layout(), &[1], Some(f64::from(RATE_HZ)), None)?;
        let inner = Arc::new(Inner {
            channel: channel.clone(),
            hints,
            open,
            active: AtomicBool::new(false),
            stop: AtomicBool::new(false),
            rate_bits: AtomicU64::new(f64::from(RATE_HZ).to_bits()),
            frame: AtomicU32::new(0),
            sent: AtomicU64::new(0),
            torn: AtomicU64::new(0),
            wakes: AtomicU64::new(0),
            thread: Mutex::new((false, None)),
        });
        let i = inner.clone();
        client.on_subscribe(&channel, move |req| {
            let rate = match codec::get(&req.options, "rate_hz") {
                None | Some(Value::Nil) => f64::from(RATE_HZ),
                Some(Value::Boolean(_)) => return Err(bad_rate()),
                Some(v) => match v.as_f64().or_else(|| v.as_i64().map(|n| n as f64)) {
                    Some(r) if (f64::from(RATE_MIN)..=f64::from(RATE_MAX)).contains(&r) => r,
                    _ => return Err(bad_rate()),
                },
            };
            i.rate_bits.store(rate.to_bits(), Ordering::Relaxed);
            Ok(())
        });
        let i = inner.clone();
        client.on_active(&channel, move |active| i.set_active(active));
        Ok(Self(inner))
    }

    /// The counters.
    pub fn stats(&self) -> TelemetryStats {
        TelemetryStats {
            sent: self.0.sent.load(Ordering::Relaxed),
            torn: self.0.torn.load(Ordering::Relaxed),
            wakes: self.0.wakes.load(Ordering::Relaxed),
        }
    }

    /// The thread is sampling.
    pub fn active(&self) -> bool {
        self.0.active.load(Ordering::Relaxed)
    }

    /// Stops the thread (shutdown).
    pub fn close(&self) {
        self.0.stop.store(true, Ordering::Relaxed);
        self.0.active.store(false, Ordering::Relaxed);
        let h = lock(&self.0.thread).1.take();
        if let Some(h) = h
            && h.thread().id() != std::thread::current().id()
        {
            let _ = h.join();
        }
    }
}

fn bad_rate() -> RemoteError {
    RemoteError::new(
        "bad_request",
        format!("{CHANNEL}: o.rate_hz must be {RATE_MIN}..{RATE_MAX}"),
    )
}

impl Inner {
    fn rate(&self) -> f64 {
        f64::from_bits(self.rate_bits.load(Ordering::Relaxed))
    }

    fn set_active(self: &Arc<Self>, active: bool) {
        if !active {
            tracing::info!(
                "Pitwall unsubscribed from {CHANNEL}: sampling stopped ({} samples sent)",
                self.sent.load(Ordering::Relaxed)
            );
            self.active.store(false, Ordering::Relaxed);
            return;
        }
        tracing::info!(
            "Pitwall subscribed to {CHANNEL} ({:.0} Hz cap): sampling",
            self.rate()
        );
        let mut g = lock(&self.thread);
        self.active.store(true, Ordering::Relaxed);
        if g.0 || self.stop.load(Ordering::Relaxed) {
            return; // a thread still winding down sees the flag again and carries on
        }
        if let Some(old) = g.1.take() {
            let _ = old.join();
        }
        g.0 = true;
        let me = self.clone();
        g.1 = std::thread::Builder::new()
            .name("telemetry".into())
            .spawn(move || me.thread_main())
            .ok();
    }

    fn thread_main(&self) {
        loop {
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.run()));
            if r.is_err() {
                tracing::error!("Telemetry sampling failed; stopped until the next subscription");
                self.active.store(false, Ordering::Relaxed);
            }
            let mut g = lock(&self.thread);
            if self.active.load(Ordering::Relaxed) && !self.stop.load(Ordering::Relaxed) {
                continue; // subscribed again while winding down
            }
            g.0 = false;
            return;
        }
    }

    fn send(&self, bytes: &[u8; SAMPLE_SIZE], t_ns: i64) {
        if self.channel.send(bytes, Some(t_ns)).is_ok() {
            self.sent.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn live(&self) -> bool {
        self.active.load(Ordering::Relaxed) && !self.stop.load(Ordering::Relaxed)
    }

    /// The sampling loop (see the module docs).
    fn run(&self) {
        ams2_platform::proc::raise_thread_priority();
        let timer = WaitableTimer::new().ok();
        let sleep = |secs: f64| {
            let d = Duration::from_secs_f64(secs.max(0.0));
            match &timer {
                Some(t) => {
                    let _ = t.sleep(d);
                }
                None => std::thread::sleep(d),
            }
        };
        let secs = |ns: i64| ns as f64 / 1e9;
        let ns = |s: f64| (s * 1e9) as i64;
        let mut view: Option<Box<dyn View>> = None;
        let mut gen_seen = u64::MAX;
        let mut last_seq: Option<u32> = None;
        let (mut last_new, mut last_sent, mut last_any) = (0i64, 0i64, 0i64);
        let mut period = PERIOD;
        let mut expected: Option<i64> = None;
        let mut empty = false;
        let (mut ref_ns, mut ref_seq) = (0i64, 0u32);
        while self.live() {
            let (running, generation) = self.hints.game();
            if !running {
                view = None;
                last_seq = None;
                let now = now_ns();
                if now - last_any >= ns(KEEPALIVE) {
                    let frame = self.frame.load(Ordering::Relaxed);
                    self.send(&idle_values(frame).to_bytes(), now);
                    last_any = now;
                }
                sleep(REOPEN_WAIT.min(KEEPALIVE));
                continue;
            }
            if view.is_none() || generation != gen_seen {
                view = (self.open)();
                if view.is_none() {
                    sleep(REOPEN_WAIT);
                    continue;
                }
                gen_seen = generation;
                last_seq = None;
            }
            let v = view.as_ref().expect("opened above");
            let seq = v.sequence();
            let now = now_ns(); // t_ns: when the connector first sees the frame
            if Some(seq) != last_seq && seq & 1 == 0 {
                let (garage, flying) = self.hints.lap_state();
                let fields = v.fields();
                if v.sequence() != seq {
                    self.torn.fetch_add(1, Ordering::Relaxed);
                    continue; // written meanwhile: read it again at once
                }
                let frames = last_seq.map_or(0, |l| i64::from(seq.wrapping_sub(l) / 2));
                if ref_ns == 0 || frames <= 0 || frames > 3 {
                    (ref_ns, ref_seq) = (now, seq);
                } else if now - ref_ns > 1_000_000_000 {
                    let n = i64::from(seq.wrapping_sub(ref_seq) / 2).max(1);
                    let dt = secs(now - ref_ns) / n as f64;
                    if dt > PERIOD_MIN && dt < PERIOD_MAX {
                        period = dt;
                    }
                    (ref_ns, ref_seq) = (now, seq);
                }
                if empty {
                    expected = Some(now - ns(DUE_POLL * 0.5));
                } else if let (Some(e), true) = (expected, (1..=3).contains(&frames)) {
                    expected = Some(now.min(e + ns(frames as f64 * period - NUDGE)));
                } else {
                    expected = Some(now);
                }
                empty = false;
                last_seq = Some(seq);
                last_new = now;
                let frame = self.frame.fetch_add(1, Ordering::Relaxed).wrapping_add(1);
                let rate = self.rate();
                if secs(now - last_sent) >= 1.0 / rate * 0.95 {
                    let s = sample_values(&fields, frame, true, garage, flying);
                    self.send(&s.to_bytes(), now);
                    last_sent = now;
                    last_any = now;
                }
                self.wakes.fetch_add(1, Ordering::Relaxed);
                // A rate cap below the frame rate: sleep over the frames not sent.
                let skip = ((0.95 / rate / period - 1e-9).ceil()).max(1.0);
                let due = secs(expected.unwrap_or(now) - now_ns()) + skip * period + LATE;
                sleep(due);
                continue;
            }
            if last_seq.is_some() && secs(now - last_any) >= KEEPALIVE {
                let (garage, flying) = self.hints.lap_state();
                let frame = self.frame.load(Ordering::Relaxed);
                let s = sample_values(&v.fields(), frame, false, garage, flying);
                self.send(&s.to_bytes(), now);
                last_any = now;
            }
            self.wakes.fetch_add(1, Ordering::Relaxed);
            empty = true;
            let due = last_new != 0 && secs(now - last_new) < 2.0 * period;
            sleep(if due { DUE_POLL } else { POLL });
        }
    }
}
