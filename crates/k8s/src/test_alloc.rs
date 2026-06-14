//! Allocation-counting global allocator + helper, shared by the crate's memory
//! regression tests (`printer_columns`, `metrics`). A test binary can only
//! define one global allocator, so it lives here and the tests reuse it.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

static ALLOCATED: AtomicUsize = AtomicUsize::new(0);

struct Counting;

// Only `alloc` is counted; the default `GlobalAlloc::realloc` routes growth back
// through `alloc`, so Vec/String growth is reflected. Atomic adds don't allocate,
// so there's no reentrancy.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATED.fetch_add(layout.size(), Ordering::Relaxed);
        System.alloc(layout)
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        System.dealloc(ptr, layout)
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

/// Bytes allocated while running `f`.
fn alloc_delta(f: impl FnOnce()) -> usize {
    let before = ALLOCATED.load(Ordering::Relaxed);
    f();
    ALLOCATED.load(Ordering::Relaxed).saturating_sub(before)
}

/// Allocation cost of `parse`, taken as the minimum over several runs so other
/// (parallel) tests' allocations — which only ever inflate a sample — don't make
/// it flaky.
pub(crate) fn min_delta<T>(parse: impl Fn() -> T) -> usize {
    (0..6)
        .map(|_| {
            alloc_delta(|| {
                std::hint::black_box(parse());
            })
        })
        .min()
        .unwrap()
}
