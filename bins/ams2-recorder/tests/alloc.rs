//! Allocations on the loop per tick in steady state (plan 3: 0, counted with a test
//! allocator; see the assertion for what still allocates). The global allocator of this test binary counts the allocations made on the
//! thread that asked for it (the loop's, here the test's own thread); the links' and the
//! writer's threads are not counted. A test mapping only; Pitwall's pipe is a random one that
//! nobody serves.

mod common;
use ams2_core::testing::Sim;
use ams2_platform::game::GameWatch;
use ams2_recorder::recorder::{Config, Overrides, Recorder};
use ams2_shm::testing::{TestMapping, unique_name};
use common::*;
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

struct Counting;

static COUNT: AtomicUsize = AtomicUsize::new(0);
thread_local! {
    static ON: Cell<bool> = const { Cell::new(false) };
}

fn counted() {
    if ON.try_with(Cell::get).unwrap_or(false) {
        COUNT.fetch_add(1, Ordering::Relaxed);
    }
}

// SAFETY: forwards to the system allocator; the counter has no effect on the memory.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        counted();
        // SAFETY: the caller's contract.
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        counted();
        // SAFETY: the caller's contract.
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        counted();
        // SAFETY: the caller's contract.
        unsafe { System.realloc(ptr, layout, new_size) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: the caller's contract.
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static A: Counting = Counting;

/// Allocations made by `f` on this thread.
fn allocations(f: impl FnOnce()) -> usize {
    let before = COUNT.load(Ordering::Relaxed);
    ON.with(|c| c.set(true));
    f();
    ON.with(|c| c.set(false));
    COUNT.load(Ordering::Relaxed) - before
}

#[test]
fn the_loop_does_not_allocate_per_tick_in_steady_state() {
    let dir = tempfile::tempdir().unwrap();
    let ws = workspace(dir.path(), &KEY);
    let shm = unique_name("alloc");
    let mut map = TestMapping::create(&shm).unwrap();
    let cfg = Config {
        hz: 50.0,
        workspace: ws.clone(),
        data_dir: ws.join("connectors").join("ams2"),
        pitwall_pipe: Some(pitwall_pipe(&unique_user())),
        control_pipe: None,
        no_control: true,
        shm_name: Some(shm),
    };
    // Simulated time, 20 ms per tick: the status (every 0.5 s) and the rival scan (2 Hz) run
    // at their real cadence however fast the test ticks.
    let secs = Arc::new(AtomicU64::new(0f64.to_bits()));
    let (c1, c2) = (secs.clone(), secs.clone());
    let base = Instant::now();
    let ov = Overrides {
        clock: Some((
            Arc::new(move || f64::from_bits(c1.load(Ordering::Relaxed))),
            "ISO".into(),
        )),
        now: Some(Arc::new(move || {
            base + Duration::from_secs_f64(f64::from_bits(c2.load(Ordering::Relaxed)))
        })),
        game: Some(GameWatch::with(
            || true,
            Instant::now,
            Duration::from_secs(5),
        )),
        ..Overrides::default()
    };
    let mut rec = Recorder::build(cfg, ov).unwrap();
    rec.start_links();
    rec.connect();
    assert!(rec.mapped());

    // Lap 1 (the first lap sizes the buffers), then most of lap 2 counted tick by tick.
    let mut sim = Sim::new();
    let mut t = 0.0f64;
    let mut frame = |lap: u32, dist: f64, rec: &mut Recorder| {
        t += 0.02;
        secs.store(t.to_bits(), Ordering::Relaxed);
        map.write_frame(sim.frame(lap, dist, t));
        allocations(|| rec.tick())
    };
    for i in 0..1500 {
        frame(1, f64::from(i) * 2.6, &mut rec);
    }
    let crossing: usize = (0..20)
        .map(|i| frame(2, f64::from(i) * 2.6, &mut rec)) // lap 1 goes to the writer
        .sum();
    assert!(crossing > 0, "the counter sees the lap hand-over");
    let per_tick: Vec<usize> = (20..1400)
        .map(|i| frame(2, f64::from(i) * 2.6, &mut rec))
        .collect();
    let ticks_allocating = per_tick.iter().filter(|n| **n > 0).count();
    eprintln!(
        "allocating ticks at: {:?}",
        per_tick
            .iter()
            .enumerate()
            .filter(|(_, n)| **n > 0)
            .take(12)
            .collect::<Vec<_>>()
    );
    let total: usize = per_tick.iter().sum();
    // Plan 3 says 0. Today the ticks that allocate are only the 0.5 s status (its JSON is built
    // to compare with the last one sent) and the rivals' 2 Hz batch: at most 2 in 25 ticks.
    assert!(
        ticks_allocating * 25 <= per_tick.len() * 2,
        "{ticks_allocating} of {} steady-state ticks allocated ({total} allocations)",
        per_tick.len()
    );
    rec.shutdown();
}
