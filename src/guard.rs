//! Process-wide limits, so a hostile or broken repository can't hang the prompt or exhaust memory.
//!
//! radar runs in every directory the shell enters. gix reads many files from `.git` (HEAD,
//! packed-refs, reflogs, excludes, attributes) and some of them are read to EOF. A symlink to
//! `/dev/zero` would then allocate without bound, and a FIFO would block forever. Guarding each
//! read is whack-a-mole, so the whole process is bounded instead:
//!
//! * a global allocator that gives up once live heap allocations exceed [`MEMORY_LIMIT`];
//! * a watchdog thread that gives up after [`TIME_LIMIT`].
//!
//! Giving up means `_exit(0)`: no output, nothing on stderr, exit code 0, the same as outside a
//! repository. Neither guard fires once the prompt has started being written, so the output is
//! never cut in half (an unclosed `\x01` would break readline).
//!
//! The counter tracks heap bytes requested through the allocator. Memory-mapped files (packs,
//! the index) are page cache and don't count.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicU8, AtomicU64, AtomicUsize, Ordering::Relaxed, Ordering::SeqCst};
use std::time::Duration;

pub const MEMORY_LIMIT: usize = 1 << 30;
pub const TIME_LIMIT: Duration = Duration::from_secs(5);

const RUNNING: u8 = 0;
const WRITING: u8 = 1;
const GAVE_UP: u8 = 2;

static STATE: AtomicU8 = AtomicU8::new(RUNNING);
static IN_USE: AtomicUsize = AtomicUsize::new(0);
static LIMIT: AtomicUsize = AtomicUsize::new(MEMORY_LIMIT);
static TIME_LIMIT_MS: AtomicU64 = AtomicU64::new(TIME_LIMIT.as_millis() as u64);

unsafe extern "C" {
    /// Terminate without running atexit handlers or destructors, and without allocating.
    fn _exit(status: i32) -> !;
}

/// Exit silently, unless the prompt is already being written.
fn give_up() {
    if STATE
        .compare_exchange(RUNNING, GAVE_UP, SeqCst, SeqCst)
        .is_ok()
        || STATE.load(SeqCst) == GAVE_UP
    {
        // SAFETY: `_exit` is async-signal-safe and never returns.
        unsafe { _exit(0) }
    }
}

/// Claim the right to write the prompt. `false` if a guard already gave up.
pub fn begin_output() -> bool {
    STATE
        .compare_exchange(RUNNING, WRITING, SeqCst, SeqCst)
        .is_ok()
}

/// Start the watchdog. Call once, before any repository work.
pub fn start() {
    #[cfg(debug_assertions)]
    {
        let var = |k| std::env::var(k).ok().and_then(|v| v.parse().ok());
        if let Some(bytes) = var("RADAR_TEST_MEMORY_LIMIT") {
            LIMIT.store(bytes as usize, SeqCst);
        }
        if let Some(ms) = var("RADAR_TEST_TIME_LIMIT_MS") {
            TIME_LIMIT_MS.store(ms, SeqCst);
        }
    }
    let limit = Duration::from_millis(TIME_LIMIT_MS.load(SeqCst));
    // Without a watchdog thread radar still works, just unguarded against hangs.
    let _ = std::thread::Builder::new().spawn(move || {
        std::thread::sleep(limit);
        give_up();
    });
}

// The counter only needs to be eventually accurate, so it uses relaxed atomics: this runs on
// every allocation.
fn grow(bytes: usize) {
    if IN_USE.fetch_add(bytes, Relaxed).saturating_add(bytes) > LIMIT.load(Relaxed) {
        give_up();
    }
}

fn shrink(bytes: usize) {
    IN_USE.fetch_sub(bytes, Relaxed);
}

/// The system allocator with a ceiling on live heap bytes.
pub struct Capped;

// SAFETY: every call forwards to `System` with the same arguments; only bookkeeping is added.
unsafe impl GlobalAlloc for Capped {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        grow(layout.size());
        let ptr = unsafe { System.alloc(layout) };
        if ptr.is_null() {
            shrink(layout.size());
        }
        ptr
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        grow(layout.size());
        let ptr = unsafe { System.alloc_zeroed(layout) };
        if ptr.is_null() {
            shrink(layout.size());
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) };
        shrink(layout.size());
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let old_size = layout.size();
        if new_size > old_size {
            grow(new_size - old_size);
        }
        let new = unsafe { System.realloc(ptr, layout, new_size) };
        match (new.is_null(), new_size > old_size) {
            (true, true) => shrink(new_size - old_size), // failed: the old block is still live
            (false, false) => shrink(old_size - new_size),
            _ => {}
        }
        new
    }
}
