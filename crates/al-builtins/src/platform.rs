//! The registry of platform tables that have a native row provider (ADR-013 decision 7, ADR-018).
//!
//! A platform table is a table of the platform app `System` (the system tables, ids from
//! 2000000000). Its schema is the real declaration, taken as AL. Its rows are not AL: the platform
//! computes them, or stores them under native triggers. A platform table with no entry here is
//! refused wherever code reaches it, because a declared table with no behaviour would answer
//! empty, and silently wrong.
//!
//! This is the second id namespace of the builtin registry (ADR-008). The providers themselves
//! live in `al-vm` (`platform/`); this table is data so that `al-sema` can refuse at compile time.

/// How the rows of a platform table are provided.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    /// Rows computed on read; the table cannot be written.
    Virtual(Virtual),
    /// An ordinary stored table, with its real declaration (ADR-013 decision 7).
    Stored,
}

/// The computed tables. One variant per table the runtime can answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Virtual {
    Integer,
    Date,
}

/// (table id, table name, provider). Every entry is a table of the platform `System` app.
pub const REGISTRY: &[(u32, &str, Provider)] = &[
    (2_000_000_026, "Integer", Provider::Virtual(Virtual::Integer)),
    (2_000_000_007, "Date", Provider::Virtual(Virtual::Date)),
];

/// The provider of platform table `id`, if it has one.
pub fn provider(id: u32) -> Option<Provider> {
    REGISTRY.iter().find(|(i, _, _)| *i == id).map(|(_, _, p)| *p)
}

/// The computed-table provider of platform table `id`, if it is one.
pub fn virtual_table(id: u32) -> Option<Virtual> {
    match provider(id)? {
        Provider::Virtual(v) => Some(v),
        Provider::Stored => None,
    }
}

/// The first id of the platform's system objects.
pub const SYSTEM_OBJECT_BASE: u32 = 2_000_000_000;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique_system_ids() {
        for (i, (id, name, _)) in REGISTRY.iter().enumerate() {
            assert!(*id >= SYSTEM_OBJECT_BASE, "{name} is not a system object");
            assert!(
                REGISTRY.iter().skip(i + 1).all(|(j, n, _)| j != id && !n.eq_ignore_ascii_case(name)),
                "{name} is registered twice"
            );
        }
    }
}
