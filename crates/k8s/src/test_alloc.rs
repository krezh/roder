//! Allocation-counting global allocator + helper, shared by the crate's memory
//! regression tests (`printer_columns`, `metrics`). A test binary can only
//! define one global allocator, so it lives here and the tests reuse it.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

thread_local! {
    // Per-thread, not a process-global atomic: `cargo test` runs tests in
    // parallel, and a global counter would fold other tests' allocations into
    // our measurement window. Each test parses on its own thread, so a
    // thread-local isolates exactly the allocations we care about.
    static ALLOCATED: Cell<usize> = const { Cell::new(0) };
}

struct Counting;

// Only `alloc` is counted; the default `GlobalAlloc::realloc` routes growth back
// through `alloc`, so Vec/String growth is reflected. A `Cell<usize>` update
// doesn't allocate, so there's no reentrancy; `try_with` tolerates the TLS not
// being live yet (very early allocations) or being torn down.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let _ = ALLOCATED.try_with(|c| c.set(c.get() + layout.size()));
        System.alloc(layout)
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        System.dealloc(ptr, layout)
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

/// Bytes allocated on the current thread while running `f`.
fn alloc_delta(f: impl FnOnce()) -> usize {
    let before = ALLOCATED.with(Cell::get);
    f();
    ALLOCATED.with(Cell::get).saturating_sub(before)
}

/// Allocation cost of `parse`, taken as the minimum over several runs so
/// allocator/measurement jitter — which only ever inflates a sample — doesn't
/// make it flaky.
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
