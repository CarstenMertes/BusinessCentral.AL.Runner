//! The AL standard library: builtin signatures (generated from the docs, see
//! ADR-008) and, later, the value model and native implementations.

pub mod library_kind;
pub mod signature;
pub mod stream;
pub mod text;
pub mod textfns;
pub mod value;
pub mod value_codec;
pub mod version;
pub mod wsac;
pub mod xml;
pub mod xmlport;

pub mod assumptions;
pub mod builtin_id;
pub mod catalog;
pub mod collation;
pub mod collections;
pub mod datatransfer;
pub mod datefns;
pub mod describe;
pub mod dotnet_sigs;
pub mod errorinfo;
pub mod evaluate;
pub mod file;
pub mod filter;
pub mod format;
pub mod formatstr;
pub mod http;
pub mod iso;
pub mod json;
pub mod mathfns;
pub mod media;
pub mod moduleinfo;
pub mod native;
pub mod ops;
pub mod page_op;
pub mod pagebuiltins;
pub mod pagefmt;
pub mod platform;
pub mod platform_native;
pub mod querybuiltins;
pub mod record_op;
pub mod refs;
pub mod sessionfns;
pub mod sessiontypes;
pub mod sysenum;

/// MEASUREMENT ONLY (false-result-risk): how many permission checks (ReadPermission, WritePermission) ran on this thread.
pub mod frr {
    use std::cell::RefCell;
    use std::collections::BTreeSet;
    thread_local! {
        pub static PERM_CHECKS: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
        /// Non-temporary tables this test read before it cleared them with DeleteAll (state inherited from earlier tests).
        pub static INHERITED: RefCell<BTreeSet<u32>> = const { RefCell::new(BTreeSet::new()) };
        pub static CLEARED: RefCell<BTreeSet<u32>> = const { RefCell::new(BTreeSet::new()) };
    }
    pub fn enabled() -> bool {
        static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        *ON.get_or_init(|| std::env::var_os("AL_FRR_LOG").is_some())
    }
    pub fn note(table: u32, read: bool, clears: bool) {
        if clears {
            CLEARED.with(|c| c.borrow_mut().insert(table));
        } else if read && !CLEARED.with(|c| c.borrow().contains(&table)) {
            INHERITED.with(|c| c.borrow_mut().insert(table));
        }
    }
    pub fn reset() {
        INHERITED.with(|c| c.borrow_mut().clear());
        CLEARED.with(|c| c.borrow_mut().clear());
        PERM_CHECKS.with(|c| c.set(0));
    }
    pub fn take_inherited() -> BTreeSet<u32> {
        INHERITED.with(|c| std::mem::take(&mut *c.borrow_mut()))
    }
}
