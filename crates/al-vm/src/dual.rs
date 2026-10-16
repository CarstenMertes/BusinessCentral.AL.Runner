//! What the dual run needs of a session (ADR-034 decision 4): the executor a session uses, and a digest of its data.
//!
//! `ExecMode::Bytecode` is the executable form lane A builds. Until it runs, a session in that mode runs the tree walker
//! too, so the harness that compares the two (`al-testrun`'s dual run) can be tested: it must find no difference
//! between two runs of the same interpreter.

use crate::Session;

/// Which executor runs the AL code of a session (docs/reviews/2026-10-bytecode-design.md; the names are the note's).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ExecMode {
    /// The tree walker over `Program` (the oracle of the dual run).
    #[default]
    Tree,
    /// The executable form (ADR-034). Falls back to the tree walker for what it does not compile yet, and for
    /// everything until it exists.
    Bytecode,
    /// What `AL_EXEC=dual` asks of a run: the harness runs each test codeunit twice, once per form. A session
    /// itself never runs in this mode; one that is given it runs as `Tree`.
    Dual,
}

impl Session {
    /// Chooses the executor for the calls that follow.
    pub fn set_exec_mode(&mut self, mode: ExecMode) {
        self.exec_mode = mode;
    }

    /// The executor this session uses.
    pub fn exec_mode(&self) -> ExecMode {
        self.exec_mode
    }

    /// A hash of the rows of each table the session holds (see `MemStore::digest`).
    pub fn state_digest(&self) -> Vec<(u32, u64)> {
        self.store.digest()
    }
}
