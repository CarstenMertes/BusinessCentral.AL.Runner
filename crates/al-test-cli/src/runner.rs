//! The seam between the harness and the runtime.

use crate::oracle::CaseId;
use crate::report::Outcome;

pub trait Runner {
    fn outcome(&self, case: &CaseId) -> Outcome;
}

/// Milestone 0: no runtime exists, so every case is reported as not run.
pub struct NoRuntime;

impl Runner for NoRuntime {
    fn outcome(&self, _case: &CaseId) -> Outcome {
        Outcome::NotRun("runtime not implemented".into())
    }
}
