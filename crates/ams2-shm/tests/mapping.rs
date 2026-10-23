//! Snapshot tests against a test mapping (never the game's `$pcars2$`).

use ams2_shm::testing::{TestMapping, unique_name};
use ams2_shm::{
    FRAME_SIZE, FrameSource, Mapping, SEQUENCE_OFFSET, ShmError, SnapshotOutcome, new_frame,
};
use proptest::prelude::*;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use zerocopy::IntoBytes;

#[test]
fn round_trip_through_a_test_mapping() {
    let name = unique_name("rt");
    let mut w = TestMapping::create(&name).unwrap();
    let mut m = Mapping::open(&name).unwrap();
    assert!(m.region_size() >= FRAME_SIZE);

    let mut f = new_frame();
    f.version = 14;
    f.speed = 55.5;
    f.car_name[..4].copy_from_slice(b"Test");
    f.participant_info[3].race_position = 7;
    f.launch_stage = -2;
    w.write_frame(&f);

    let mut out = new_frame();
    assert_eq!(m.snapshot(&mut out).unwrap(), SnapshotOutcome::Frame(2));
    assert_eq!((out.version, out.speed, out.launch_stage), (14, 55.5, -2));
    assert_eq!(&out.car_name[..4], b"Test");
    assert_eq!(out.participant_info[3].race_position, 7);
    assert_eq!(out.sequence_number, 2);
}

#[test]
fn odd_sequence_is_never_returned() {
    let name = unique_name("odd");
    let mut w = TestMapping::create(&name).unwrap();
    let mut m = Mapping::open(&name).unwrap();
    w.begin_write_and_hold();
    let mut out = new_frame();
    assert_eq!(m.snapshot(&mut out).unwrap(), SnapshotOutcome::Torn);
}

#[test]
fn missing_mapping_and_refused_names() {
    let e = Mapping::open(&unique_name("absent")).err().unwrap();
    assert!(matches!(e, ShmError::NotFound(_)));
    assert!(matches!(
        TestMapping::create("$pcars2$").err().unwrap(),
        ShmError::RefusedName(_)
    ));
}

/// A frame whose every byte (but the sequence word) is `k`: any torn copy mixes values.
fn uniform_frame(k: u8) -> Box<ams2_shm::SharedMemory> {
    let mut f = new_frame();
    f.as_mut_bytes().fill(k);
    f
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(6))]
    #[test]
    fn seqlock_never_returns_a_torn_snapshot(seed in 1u8..=250, reads in 2000usize..4000) {
        let name = unique_name("seq");
        let mut w = TestMapping::create(&name).unwrap();
        let mut m = Mapping::open(&name).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let stop_w = stop.clone();
        let writer = std::thread::spawn(move || {
            let mut k = seed;
            while !stop_w.load(Ordering::Relaxed) {
                w.write_frame(&uniform_frame(k));
                k = k.wrapping_add(1).max(1);
            }
            w.sequence()
        });
        let mut out = new_frame();
        let (mut frames, mut last_seq) = (0u32, 0u32);
        for _ in 0..reads {
            if let SnapshotOutcome::Frame(seq) = m.snapshot(&mut out).unwrap() {
                let bytes = out.as_bytes();
                let k = bytes[0];
                prop_assert_eq!(seq % 2, 0);
                prop_assert!(seq >= last_seq);
                last_seq = seq;
                if seq != 0 {
                    prop_assert!(bytes[..SEQUENCE_OFFSET].iter().all(|b| *b == k));
                    prop_assert!(bytes[SEQUENCE_OFFSET + 4..].iter().all(|b| *b == k));
                }
                frames += 1;
            }
        }
        stop.store(true, Ordering::Relaxed);
        writer.join().unwrap();
        prop_assert!(frames > 0);
    }
}
