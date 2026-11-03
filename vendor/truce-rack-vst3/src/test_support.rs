//! Per-thread allocation accounting for native process-boundary tests.

use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
};

thread_local! {
    static MEASURE: Cell<bool> = const { Cell::new(false) };
    static DEALLOCATIONS: Cell<usize> = const { Cell::new(0) };
    static ALLOCATIONS: Cell<usize> = const { Cell::new(0) };
}

struct CountingAllocator;
// SAFETY: each operation forwards the original pointer/layout to the system
// allocator; thread-local counters observe calls without changing ownership.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if MEASURE.try_with(Cell::get).unwrap_or(false) {
            ALLOCATIONS.with(|count| count.set(count.get() + 1));
        }
        // SAFETY: the caller supplies the GlobalAlloc layout contract unchanged.
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        if MEASURE.try_with(Cell::get).unwrap_or(false) {
            DEALLOCATIONS.with(|count| count.set(count.get() + 1));
        }
        // SAFETY: the pointer/layout retain the caller's GlobalAlloc ownership contract.
        unsafe { System.dealloc(pointer, layout) };
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        if MEASURE.try_with(Cell::get).unwrap_or(false) {
            ALLOCATIONS.with(|count| count.set(count.get() + 1));
        }
        // SAFETY: the caller supplies the original allocation and valid replacement size.
        unsafe { System.realloc(pointer, layout, size) }
    }
}
#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

pub(crate) fn count_allocations(operation: impl FnOnce()) -> usize {
    count_heap_operations(operation).0
}

pub(crate) fn count_heap_operations(operation: impl FnOnce()) -> (usize, usize) {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            MEASURE.with(|value| value.set(false));
        }
    }
    ALLOCATIONS.with(|count| count.set(0));
    DEALLOCATIONS.with(|count| count.set(0));
    MEASURE.with(|value| value.set(true));
    let reset = Reset;
    operation();
    drop(reset);
    (ALLOCATIONS.with(Cell::get), DEALLOCATIONS.with(Cell::get))
}
