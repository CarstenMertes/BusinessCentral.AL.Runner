//! Builtins that act on session state. The VM implements them (`Ctx::session_op`); this table
//! only registers the overload ids so lowering accepts them.

use crate::native::NativeFn;

macro_rules! via_session {
    ($($id:literal),* $(,)?) => {
        &[$(($id, |c, _, a| c.session_op($id, a))),*]
    };
}

pub const REGISTRY: &[(&str, NativeFn)] = via_session![
    "NumberSequence.Insert#1",
    "NumberSequence.Delete#1",
    "NumberSequence.Exists#1",
    "NumberSequence.Next#1",
    "NumberSequence.Current#1",
    "NumberSequence.Range#1",
    "NumberSequence.Range#2",
    "NumberSequence.Restart#1",
    "Database.IsInWriteTransaction#1",
    "Database.CompanyName#1",
];
