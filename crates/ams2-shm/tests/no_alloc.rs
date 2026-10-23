//! `snapshot()` must not allocate after setup (counting allocator, per thread).

use ams2_shm::testing::{TestMapping, unique_name};
use ams2_shm::{FrameSource, Mapping, SnapshotOutcome, new_frame};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

struct Counting;

thread_local! {
    static ALLOCS: Cell<u64> = const { Cell::new(0) };
}

// SAFETY: forwards to the system allocator, only counting.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let _ = ALLOCS.try_with(|c| c.set(c.get() + 1));
        unsafe { System.alloc(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        unsafe { System.dealloc(p, l) }
    }
}

#[global_allocator]
static A: Counting = Counting;

#[test]
fn snapshot_does_not_allocate() {
    let name = unique_name("alloc");
    let mut w = TestMapping::create(&name).unwrap();
    let mut m = Mapping::open(&name).unwrap();
    let mut out = new_frame();
    w.write_frame(&new_frame());
    let before = ALLOCS.with(Cell::get);
    for _ in 0..1000 {
        assert!(matches!(
            m.snapshot(&mut out).unwrap(),
            SnapshotOutcome::Frame(_)
        ));
    }
    assert_eq!(ALLOCS.with(Cell::get), before);
}
