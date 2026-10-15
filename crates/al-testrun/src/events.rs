//! What a run tells while it runs: one event per test as it finishes, and a flag that stops the run after the current
//! test (`alrunner serve`, ADR-028). Installed per thread by the code that runs a test codeunit, so the signatures of
//! the run functions did not change.

use crate::Outcome;
use std::cell::RefCell;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// A test that finished.
#[derive(Debug, Clone)]
pub struct TestEvent {
    pub codeunit: u32,
    pub codeunit_name: String,
    pub test: String,
    pub outcome: Outcome,
    pub call_stack: String,
    pub time: std::time::Duration,
    /// With `RunOptions::coverage`: the statements the test ran (position id, runs).
    pub coverage: Vec<(u32, u32)>,
}

type OnTest = Arc<dyn Fn(TestEvent) + Send + Sync>;

/// The callbacks of a run.
#[derive(Clone, Default)]
pub struct Hooks {
    /// Called on the thread that ran the test, as soon as its outcome is known.
    pub on_test: Option<OnTest>,
    /// Set from outside: no test starts after it is set; the running one finishes.
    pub cancel: Option<Arc<AtomicBool>>,
}

impl std::fmt::Debug for Hooks {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Hooks")
            .field("on_test", &self.on_test.is_some())
            .field("cancel", &self.cancel.is_some())
            .finish()
    }
}

thread_local! {
    static CURRENT: RefCell<(Hooks, Vec<String>)> = RefCell::new((Hooks::default(), Vec::new()));
}

pub(crate) fn install(h: Hooks) {
    CURRENT.with(|c| *c.borrow_mut() = (h, Vec::new()));
}

/// Removes the hooks of this thread and returns the names of the tests already reported.
pub(crate) fn uninstall() -> Vec<String> {
    CURRENT.with(|c| std::mem::take(&mut *c.borrow_mut()).1)
}

pub(crate) fn reported() -> Vec<String> {
    CURRENT.with(|c| c.borrow().1.clone())
}

pub(crate) fn cancelled() -> bool {
    CURRENT.with(|c| c.borrow().0.cancel.as_ref().is_some_and(|f| f.load(Ordering::Relaxed)))
}

/// The flag that stops the run (for a scratch run, whose one routine is stopped while it runs).
pub(crate) fn cancel_flag() -> Option<Arc<AtomicBool>> {
    CURRENT.with(|c| c.borrow().0.cancel.clone())
}

pub(crate) fn wants_events() -> bool {
    CURRENT.with(|c| c.borrow().0.on_test.is_some())
}

pub(crate) fn report(e: TestEvent) {
    let hook = CURRENT.with(|c| {
        let mut c = c.borrow_mut();
        c.1.push(e.test.clone());
        c.0.on_test.clone()
    });
    if let Some(h) = hook {
        h(e);
    }
}
