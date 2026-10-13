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
    "IsolatedStorage.Contains#1",
    "IsolatedStorage.Contains#2",
    "IsolatedStorage.Delete#1",
    "IsolatedStorage.Get#1",
    "IsolatedStorage.Get#2",
    "IsolatedStorage.Get#3",
    "IsolatedStorage.Get#4",
    "IsolatedStorage.Set#1",
    "IsolatedStorage.Set#2",
    "IsolatedStorage.SetEncrypted#1",
    "IsolatedStorage.SetEncrypted#2",
    "System.CodeCoverageLog#1",
    "Database.RegisterTableConnection#1",
    "Database.UnregisterTableConnection#1",
    "Database.HasTableConnection#1",
    "Database.SetDefaultTableConnection#1",
    "Database.GetDefaultTableConnection#1",
];
