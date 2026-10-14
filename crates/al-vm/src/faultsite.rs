//! Where a refusal or a failure was raised, for `al-test-cli --explain`.
//!
//! With tracing on, a routine that an error or a refusal leaves records the call stack it left
//! from. The innermost routine runs first, so the first snapshot is the deepest; a shallower one
//! that is a prefix of it (the same statements on the way out) does not replace it. A snapshot of
//! another branch does, so an error that was caught earlier does not outlive a later one.
//! Tracing is off in a normal run: nothing is recorded and the cost is one flag test on the
//! error path.

use crate::Session;

/// One routine on the stack: its object, its routine index and the statement it was running.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FaultFrame {
    pub object: usize,
    pub routine: usize,
    /// `Program::positions` id of the statement; `al_ir::NO_POS` before the first one.
    pub pos: u32,
}

impl Session {
    /// Starts or stops recording fault sites. Starting forgets the last one.
    pub fn trace_faults(&mut self, on: bool) {
        self.fault_trace = on;
        self.fault_site.clear();
    }

    /// The call stack (outermost first) the last error or refusal was raised from; empty when
    /// nothing was recorded.
    pub fn fault_site(&self) -> &[FaultFrame] {
        &self.fault_site
    }

    /// Forgets the recorded site (before the next test).
    pub fn clear_fault_site(&mut self) {
        self.fault_site.clear();
    }

    /// Called by a routine an error or a refusal is leaving, before its frame is popped.
    pub(crate) fn note_fault(&mut self) {
        let now: Vec<FaultFrame> = self
            .frames
            .iter()
            .zip(&self.call_stack)
            .map(|(&object, f)| FaultFrame {
                object,
                routine: f.routine,
                pos: f.pos,
            })
            .collect();
        let deeper_branch_already_seen = now.len() <= self.fault_site.len() && self.fault_site[..now.len()] == now[..];
        if !deeper_branch_already_seen {
            self.fault_site = now;
        }
    }
}
