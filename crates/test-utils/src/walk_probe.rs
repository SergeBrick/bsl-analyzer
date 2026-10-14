//! How far a reference walk got, read from the span `ide` already emits.
//!
//! `ide::references` opens a `find_references_in_file` span per file, so a cancellation
//! gate can learn that a walk has started — and how many files it entered — without a
//! `sleep` and without a counter in production code. The price is that the subscriber
//! and the counter are process-global: every test in a binary that walks references
//! adds to the same number. A test that reads it must therefore serialise against every
//! other test in its binary that can move it — a gate mutex owned by that test crate —
//! else [`await_walk_start`] returns on somebody else's file and the gate measures an
//! interleaving instead of a cancellation.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

const WALK_SPAN: &str = "find_references_in_file";

/// Files the walk has entered since the last [`reset`].
static WALKED: AtomicUsize = AtomicUsize::new(0);

struct FirstSpanGate {
    state: Mutex<FirstSpanState>,
    changed: Condvar,
}

#[derive(Default)]
struct FirstSpanState {
    claimed: bool,
    entered: bool,
    released: bool,
}

static FIRST_SPAN_GATE: Mutex<Option<Arc<FirstSpanGate>>> = Mutex::new(None);

fn lock_recover<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

struct WalkCounter;

impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for WalkCounter {
    fn on_new_span(
        &self,
        attrs: &tracing::span::Attributes<'_>,
        _id: &tracing::span::Id,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        if attrs.metadata().name() == WALK_SPAN {
            WALKED.fetch_add(1, Ordering::SeqCst);
            let gate = lock_recover(&FIRST_SPAN_GATE).clone();
            if let Some(gate) = gate {
                let mut state = lock_recover(&gate.state);
                if !state.claimed {
                    state.claimed = true;
                    state.entered = true;
                    gate.changed.notify_all();
                    while !state.released {
                        state = gate
                            .changed
                            .wait(state)
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                    }
                }
            }
        }
    }
}

/// Install the counting subscriber once for the whole test binary.
///
/// A global subscriber installed earlier by something else wins silently, and the
/// counter then never moves: a positive control that expects the full walk count is
/// what exposes that, so every gate on this counter must keep one.
pub fn install() {
    use tracing_subscriber::prelude::*;
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let _ = tracing_subscriber::registry()
            .with(WalkCounter)
            .with(tracing_subscriber::filter::LevelFilter::DEBUG)
            .try_init();
    });
}

pub fn reset() {
    WALKED.store(0, Ordering::SeqCst);
}

pub fn entered() -> usize {
    WALKED.load(Ordering::SeqCst)
}

/// Hold the first subsequent reference-walk span until the returned guard is released.
///
/// The guard releases the span and removes the process-global gate on drop, including when
/// a test assertion unwinds.
pub fn hold_first_span() -> (FirstSpanGuard, FirstSpanWaiter) {
    let gate = Arc::new(FirstSpanGate {
        state: Mutex::new(FirstSpanState::default()),
        changed: Condvar::new(),
    });
    let mut active = lock_recover(&FIRST_SPAN_GATE);
    assert!(active.is_none(), "a first-span gate is already active");
    *active = Some(Arc::clone(&gate));
    (FirstSpanGuard { gate: Arc::clone(&gate) }, FirstSpanWaiter { gate })
}

/// RAII handle that releases a held reference-walk span on drop.
pub struct FirstSpanGuard {
    gate: Arc<FirstSpanGate>,
}

impl FirstSpanGuard {
    pub fn release(&self) {
        let mut state = lock_recover(&self.gate.state);
        state.released = true;
        self.gate.changed.notify_all();
    }
}

impl Drop for FirstSpanGuard {
    fn drop(&mut self) {
        self.release();
        let mut active = lock_recover(&FIRST_SPAN_GATE);
        if active.as_ref().is_some_and(|gate| Arc::ptr_eq(gate, &self.gate)) {
            *active = None;
        }
    }
}

/// Bounded observer for the first span held by [`hold_first_span`].
#[derive(Clone)]
pub struct FirstSpanWaiter {
    gate: Arc<FirstSpanGate>,
}

impl FirstSpanWaiter {
    /// Wait for the tracing callback to reach the held span, bounded by `timeout`.
    pub fn wait_entered(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let mut state = lock_recover(&self.gate.state);
        while !state.entered {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return false;
            }
            let (next, result) = self
                .gate
                .changed
                .wait_timeout(state, remaining)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state = next;
            if result.timed_out() && !state.entered {
                return false;
            }
        }
        true
    }
}

/// Block until the walk has entered its first file, so a cancel lands mid-flight. A
/// fixed sleep would race the build of whatever the walk reads first. The bound counts
/// polls, so a slow machine waits longer for the walk to start instead of failing on it.
pub fn await_walk_start() {
    for _ in 0..60_000 {
        if entered() > 0 {
            return;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    panic!("the walk never started");
}
