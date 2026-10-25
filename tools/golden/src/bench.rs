//! `bench`: the recorder's hot path (snapshot copy, tracker, detector, live publish) over a
//! capture, at full speed for the tick times and paced at 50 Hz for the CPU.

use crate::sampler::Sampler;
use ams2_core::announce::detector::EventDetector;
use ams2_core::testing::{FakeClock, RecordingSink};
use ams2_core::tracker::SessionTracker;
use ams2_platform::timer::WaitableTimer;
use ams2_recorder::live::{LiveBuffer, Meta};
use ams2_shm::{SharedMemory, new_frame};
use std::io::BufReader;
use std::path::Path;
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::FILETIME;
use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};
use zerocopy::IntoBytes;

/// The hot path's state.
struct Hot {
    clock: FakeClock,
    tracker: SessionTracker,
    detector: EventDetector,
    sink: RecordingSink,
    live: LiveBuffer,
    work: Box<SharedMemory>,
}

impl Hot {
    fn new() -> Self {
        let clock = FakeClock::new();
        Self {
            tracker: SessionTracker::new(Box::new(clock.clone())),
            detector: EventDetector::new(clock.clone(), 2.0),
            clock,
            sink: RecordingSink::new(),
            live: LiveBuffer::default(),
            work: new_frame(),
        }
    }

    /// One tick as the recorder's loop does it, from `src` (the game's mapping).
    fn tick(&mut self, src: &SharedMemory, t_s: f64) {
        self.clock.set(t_s);
        self.work.as_mut_bytes().copy_from_slice(src.as_bytes());
        self.tracker.process(&self.work, &mut self.sink);
        self.sink.calls.clear();
        self.live.publish(
            &self.work,
            Meta {
                in_garage: self.tracker.in_garage(),
                lap: self.tracker.live_lap(),
                last_change: Instant::now(),
            },
        );
        std::hint::black_box(self.detector.update(&self.work));
    }
}

fn pct(sorted: &[f64], p: f64) -> f64 {
    sorted[((sorted.len() as f64 * p) as usize).min(sorted.len() - 1)]
}

fn report(name: &str, mut ns: Vec<f64>) {
    ns.sort_by(f64::total_cmp);
    let mean = ns.iter().sum::<f64>() / ns.len() as f64;
    println!(
        "{name}: {} ticks, median {:.2} us, mean {:.2} us, p99 {:.2} us, p99.9 {:.2} us, max {:.1} us",
        ns.len(),
        pct(&ns, 0.5) / 1e3,
        mean / 1e3,
        pct(&ns, 0.99) / 1e3,
        pct(&ns, 0.999) / 1e3,
        ns[ns.len() - 1] / 1e3
    );
}

fn ft(t: &FILETIME) -> f64 {
    ((u64::from(t.dwHighDateTime) << 32) | u64::from(t.dwLowDateTime)) as f64 * 1e-7
}

/// Kernel plus user CPU seconds of this process.
fn cpu_seconds() -> f64 {
    let z = FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    let (mut c, mut e, mut k, mut u) = (z, z, z, z);
    // SAFETY: the pseudo handle of this process and four out pointers.
    unsafe { GetProcessTimes(GetCurrentProcess(), &mut c, &mut e, &mut k, &mut u) };
    ft(&k) + ft(&u)
}

/// Runs both measurements on `capture`; the paced run replays `paced_s` seconds from `from_s`.
pub fn run(capture: &Path, from_s: f64, paced_s: f64) -> Result<(), String> {
    let open = || -> Result<Sampler<_>, String> {
        let f = std::fs::File::open(capture).map_err(|e| format!("{}: {e}", capture.display()))?;
        Sampler::new(BufReader::with_capacity(1 << 20, f))
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "the capture holds no record".to_owned())
    };
    // Full speed.
    let mut sampler = open()?;
    let mut hot = Hot::new();
    let t0 = sampler.t0_us();
    let mut ns = Vec::new();
    let mut busy = 0.0;
    while let Some(rel) = sampler.advance().map_err(|e| e.to_string())? {
        let t_s = (t0 + rel) as f64 / 1e6;
        let start = Instant::now();
        hot.tick(sampler.frame(), t_s);
        let d = start.elapsed().as_secs_f64();
        busy += d;
        ns.push(d * 1e9);
    }
    println!(
        "full speed: {} records, {:.1} s busy in the hot path, {:.0} ticks/s",
        sampler.records,
        busy,
        ns.len() as f64 / busy
    );
    report("full speed", ns);

    // Paced: a segment into memory, then 50 Hz.
    let mut sampler = open()?;
    let want = (paced_s * 50.0) as usize;
    let skip = (from_s * 50.0) as u64;
    let mut seg: Vec<(f64, Box<SharedMemory>)> = Vec::with_capacity(want);
    while let Some(rel) = sampler.advance().map_err(|e| e.to_string())? {
        if sampler.ticks > skip {
            let mut f = new_frame();
            f.as_mut_bytes().copy_from_slice(sampler.frame().as_bytes());
            seg.push(((t0 + rel) as f64 / 1e6, f));
            if seg.len() == want {
                break;
            }
        }
    }
    if seg.len() < want {
        return Err(format!(
            "only {} of {want} ticks after {from_s} s: use a smaller --from",
            seg.len()
        ));
    }
    let mut hot = Hot::new();
    let timer = WaitableTimer::new().map_err(|e| e.to_string())?;
    timer
        .start_periodic(Duration::from_millis(20))
        .map_err(|e| e.to_string())?;
    let mut ns = Vec::with_capacity(want);
    let (c0, w0) = (cpu_seconds(), Instant::now());
    for (t_s, f) in &seg {
        let start = Instant::now();
        hot.tick(f, *t_s);
        ns.push(start.elapsed().as_secs_f64() * 1e9);
        timer.wait_timeout(Duration::from_millis(80));
    }
    let (cpu, wall) = (cpu_seconds() - c0, w0.elapsed().as_secs_f64());
    report("paced 50 Hz", ns);
    println!(
        "paced 50 Hz: {:.3} % of a core ({:.3} s CPU in {:.1} s), segment from {from_s} s",
        cpu / wall * 100.0,
        cpu,
        wall
    );
    println!(
        "targets (docs/ARCHITECTURE.md Performance): tick median <= 5 us; recorder CPU at 50 Hz <= 0.2 % (the whole recorder, this is its hot path only)"
    );
    Ok(())
}
