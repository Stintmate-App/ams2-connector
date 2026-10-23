//! Cost of `EventDetector::update` on a steady on-track race snapshot with 20 cars (the
//! recorder calls it at 50 Hz), and of the rival scan (2 Hz in production, forced every tick
//! here). Run: `cargo bench -p ams2-core`.

use ams2_core::announce::EventDetector;
use ams2_core::tracker::Clock;
use ams2_shm::{SharedMemory, new_frame};
use criterion::{Criterion, criterion_group, criterion_main};
use std::hint::black_box;

/// A clock the bench advances by hand (a `Clock` must be `Send`, so an atomic).
#[derive(Clone, Default)]
struct Manual(std::sync::Arc<std::sync::atomic::AtomicU64>);

impl Manual {
    fn advance(&self, dt: f64) {
        let t = f64::from_bits(self.0.load(std::sync::atomic::Ordering::Relaxed)) + dt;
        self.0
            .store(t.to_bits(), std::sync::atomic::Ordering::Relaxed);
    }
}

impl Clock for Manual {
    fn now(&self) -> f64 {
        f64::from_bits(self.0.load(std::sync::atomic::Ordering::Relaxed))
    }
    fn iso(&self) -> String {
        String::new()
    }
}

fn race(n: usize) -> Box<SharedMemory> {
    let mut d = new_frame();
    d.version = 14;
    d.game_state = 2;
    d.session_state = 5;
    d.race_state = 2;
    d.track_length = 4000.0;
    d.laps_in_event = 10;
    d.num_participants = n as i32;
    d.highest_flag_colour = 1;
    d.speed = 50.0;
    for i in 0..n {
        let p = &mut d.participant_info[i];
        p.is_active = 1;
        let name = if i == 0 {
            "Me".to_string()
        } else {
            format!("Rival {i}")
        };
        p.name[..name.len()].copy_from_slice(name.as_bytes());
        p.race_position = i as u32 + 1;
        p.current_lap = 3;
        p.current_lap_distance = 1000.0 + 100.0 * i as f32;
        p.current_sector = 1;
        d.car_class_names[i][..3].copy_from_slice(b"GT3");
        d.race_states[i] = 2;
    }
    d
}

fn bench(c: &mut Criterion) {
    let d = race(20);
    let mut group = c.benchmark_group("detector");

    let clock = Manual::default();
    let mut det = EventDetector::new(clock.clone(), 2.0);
    for _ in 0..600 {
        clock.advance(0.02); // past the 10 s quiet period
        det.update(&d);
    }
    group.bench_function("update_20_cars_steady_race", |b| {
        b.iter(|| {
            clock.advance(0.02);
            black_box(det.update(black_box(&d)))
        })
    });

    let clock = Manual::default();
    let mut scan = EventDetector::new(clock.clone(), 1e9);
    for _ in 0..600 {
        clock.advance(0.02);
        scan.update(&d);
    }
    group.bench_function("update_20_cars_with_rival_scan_every_tick", |b| {
        b.iter(|| {
            clock.advance(0.02);
            black_box(scan.update(black_box(&d)))
        })
    });
    group.finish();
}

criterion_group!(benches, bench);
criterion_main!(benches);
