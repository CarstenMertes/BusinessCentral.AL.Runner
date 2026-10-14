//! In-memory store on persistent maps: a snapshot is a cheap clone (ADR-004).
//! Scans are linear in the table size; fine for test-sized tables.

use crate::*;
use al_builtins::filter::{FieldFilter, key_cmp};
use std::cmp::Ordering;

/// A primary-key tuple ordered field by field.
#[derive(Clone, Debug)]
struct Key(Vec<Value>);

impl PartialEq for Key {
    fn eq(&self, o: &Self) -> bool {
        key_cmp(&self.0, &o.0) == Ordering::Equal
    }
}
impl Eq for Key {}
impl PartialOrd for Key {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}
impl Ord for Key {
    fn cmp(&self, o: &Self) -> Ordering {
        key_cmp(&self.0, &o.0)
    }
}

type Table = im::OrdMap<Key, Row>;
type Tables = im::OrdMap<(String, u32), Table>;

#[derive(Clone, Default)]
pub struct MemStore {
    tables: Tables,
    committed: Tables,
    /// The value of `counter` when each table last changed. It follows `tables` through commit and
    /// rollback, so a table has the same stamp exactly when it has the same content history.
    versions: im::OrdMap<(String, u32), u64>,
    committed_versions: im::OrdMap<(String, u32), u64>,
    /// Handed out by writes; never goes back, so a stamp is never reused.
    counter: u64,
    /// Tables that have no stored rows (ADR-019).
    virtuals: Option<std::rc::Rc<dyn VirtualTables>>,
}

fn slot(cx: &AccessContext, t: &TableDef) -> (String, u32) {
    (
        if t.per_company {
            cx.company.clone()
        } else {
            String::new()
        },
        t.object,
    )
}

fn passes(row: &Row, filters: &[FieldFilter]) -> bool {
    filters.iter().all(|f| f.expr.matches(&row[f.field]))
}

impl MemStore {
    /// A store whose virtual tables are answered by `v`.
    pub fn with_virtual_tables(v: std::rc::Rc<dyn VirtualTables>) -> MemStore {
        MemStore {
            virtuals: Some(v),
            ..Default::default()
        }
    }

    /// The store without its provider: the data only. A captured snapshot (ADR-004) holds no
    /// provider, so it cannot keep the session that made it alive.
    pub fn detached(mut self) -> MemStore {
        self.virtuals = None;
        self
    }

    /// The provider of the computed tables.
    pub fn virtual_tables(&self) -> Option<std::rc::Rc<dyn VirtualTables>> {
        self.virtuals.clone()
    }

    /// This store's data with `v` as its provider.
    pub fn attached(mut self, v: Option<std::rc::Rc<dyn VirtualTables>>) -> MemStore {
        self.virtuals = v;
        self
    }

    /// The provider of table `t`, if its rows are computed.
    fn virtual_of(&self, t: &TableDef) -> Option<&dyn VirtualTables> {
        self.virtuals.as_deref().filter(|v| v.serves(t))
    }

    fn read_only(t: &TableDef, what: &str) -> StoreError {
        StoreError::Refused(format!(
            "{what} on virtual table '{}': its rows are computed, not stored",
            t.name
        ))
    }

    /// Whether any write since the last commit or rollback is pending.
    pub fn has_uncommitted(&self) -> bool {
        !(self.tables.ptr_eq(&self.committed) || (self.tables.is_empty() && self.committed.is_empty()))
    }

    /// Whether table object `object` holds a row now (committed or not) in any company.
    pub fn has_rows(&self, object: u32) -> bool {
        self.tables.iter().any(|((_, o), t)| *o == object && !t.is_empty())
    }

    /// The table objects that hold a committed row in any company.
    pub fn nonempty_tables(&self) -> std::collections::BTreeSet<u32> {
        self.committed
            .iter()
            .filter(|(_, t)| !t.is_empty())
            .map(|((_, object), _)| *object)
            .collect()
    }

    /// Whether `company` holds a row in any per-company table, committed or not.
    pub fn has_company_rows(&self, company: &str) -> bool {
        self.tables
            .iter()
            .any(|((c, _), t)| !c.is_empty() && c.eq_ignore_ascii_case(company) && !t.is_empty())
    }

    /// The companies that hold a row in per-company table `object`, committed or not.
    pub fn companies_with_rows(&self, object: u32) -> Vec<String> {
        self.tables
            .iter()
            .filter(|((c, o), t)| *o == object && !c.is_empty() && !t.is_empty())
            .map(|((c, _), _)| c.clone())
            .collect()
    }

    /// Removes every row of `company`'s tables, as deleting the company does. Part of the open
    /// transaction: a rollback puts the rows back.
    pub fn drop_company(&mut self, company: &str) {
        let doomed: Vec<(String, u32)> = self
            .tables
            .keys()
            .filter(|(c, _)| !c.is_empty() && c.eq_ignore_ascii_case(company))
            .cloned()
            .collect();
        for slot in doomed {
            self.tables.remove(&slot);
            self.counter += 1;
            self.versions.insert(slot, self.counter);
        }
    }

    /// A number that changes whenever the rows of table `t` change (a rollback included). Queries
    /// compare it to tell whether their result set is stale (corpus query/TestQueryReadAfterWrite.al).
    pub fn table_stamp(&self, cx: &AccessContext, t: &TableDef) -> u64 {
        if self.virtual_of(t).is_some() {
            return 0;
        }
        self.versions.get(&slot(cx, t)).copied().unwrap_or(0)
    }

    fn touch(&mut self, cx: &AccessContext, t: &TableDef) {
        self.counter += 1;
        self.versions.insert(slot(cx, t), self.counter);
    }

    fn table(&self, cx: &AccessContext, t: &TableDef) -> Option<&Table> {
        self.tables.get(&slot(cx, t))
    }

    fn table_mut(&mut self, cx: &AccessContext, t: &TableDef) -> &mut Table {
        self.tables.entry(slot(cx, t)).or_default()
    }
}

impl Store for MemStore {
    fn insert(&mut self, cx: &AccessContext, t: &TableDef, row: Row) -> Result<InsertOutcome, StoreError> {
        if self.virtual_of(t).is_some() {
            return Err(Self::read_only(t, "Insert"));
        }
        let k = Key(t.primary_key(&row));
        let table = self.table_mut(cx, t);
        if table.contains_key(&k) {
            return Ok(InsertOutcome::AlreadyExists);
        }
        table.insert(k, row);
        self.touch(cx, t);
        Ok(InsertOutcome::Inserted)
    }

    fn get(&self, cx: &AccessContext, t: &TableDef, pk: &[Value]) -> Result<Option<Row>, StoreError> {
        if let Some(v) = self.virtual_of(t) {
            return v.get(self, cx, t, pk);
        }
        Ok(self.table(cx, t).and_then(|m| m.get(&Key(pk.to_vec())).cloned()))
    }

    fn modify(&mut self, cx: &AccessContext, t: &TableDef, row: Row) -> Result<WriteOutcome, StoreError> {
        if self.virtual_of(t).is_some() {
            return Err(Self::read_only(t, "Modify"));
        }
        let k = Key(t.primary_key(&row));
        let table = self.table_mut(cx, t);
        if !table.contains_key(&k) {
            return Ok(WriteOutcome::NotFound);
        }
        table.insert(k, row);
        self.touch(cx, t);
        Ok(WriteOutcome::Done)
    }

    fn delete(&mut self, cx: &AccessContext, t: &TableDef, pk: &[Value]) -> Result<WriteOutcome, StoreError> {
        if self.virtual_of(t).is_some() {
            return Err(Self::read_only(t, "Delete"));
        }
        Ok(match self.table_mut(cx, t).remove(&Key(pk.to_vec())) {
            Some(_) => {
                self.touch(cx, t);
                WriteOutcome::Done
            }
            None => WriteOutcome::NotFound,
        })
    }

    fn rename(
        &mut self,
        cx: &AccessContext,
        t: &TableDef,
        old: &[Value],
        row: Row,
    ) -> Result<RenameOutcome, StoreError> {
        if self.virtual_of(t).is_some() {
            return Err(Self::read_only(t, "Rename"));
        }
        let new_k = Key(t.primary_key(&row));
        let table = self.table_mut(cx, t);
        if !table.contains_key(&Key(old.to_vec())) {
            return Ok(RenameOutcome::NotFound);
        }
        if table.contains_key(&new_k) {
            return Ok(RenameOutcome::AlreadyExists);
        }
        table.remove(&Key(old.to_vec()));
        table.insert(new_k, row);
        self.touch(cx, t);
        Ok(RenameOutcome::Renamed)
    }

    fn next(&self, cx: &AccessContext, t: &TableDef, scan: &Scan) -> Result<Option<Row>, StoreError> {
        if let Some(v) = self.virtual_of(t) {
            return v.next(self, cx, t, scan);
        }
        let Some(table) = self.table(cx, t) else {
            return Ok(None);
        };
        let mut best: Option<(Vec<Value>, &Row)> = None;
        for row in table.values().filter(|r| passes(r, scan.filters)) {
            let pos = position(t, scan.key, row);
            let after_ok = scan.after.is_none_or(|a| {
                let o = key_cmp(&pos, a);
                if scan.ascending {
                    o == Ordering::Greater
                } else {
                    o == Ordering::Less
                }
            });
            if !after_ok {
                continue;
            }
            let better = best.as_ref().is_none_or(|(b, _)| {
                let o = key_cmp(&pos, b);
                if scan.ascending {
                    o == Ordering::Less
                } else {
                    o == Ordering::Greater
                }
            });
            if better {
                best = Some((pos, row));
            }
        }
        Ok(best.map(|(_, r)| r.clone()))
    }

    fn count(&self, cx: &AccessContext, t: &TableDef, f: &[FieldFilter]) -> Result<usize, StoreError> {
        if let Some(v) = self.virtual_of(t) {
            return v.count(self, cx, t, f);
        }
        Ok(self
            .table(cx, t)
            .map_or(0, |m| m.values().filter(|r| passes(r, f)).count()))
    }

    fn delete_all(&mut self, cx: &AccessContext, t: &TableDef, f: &[FieldFilter]) -> Result<usize, StoreError> {
        if self.virtual_of(t).is_some() {
            return Err(Self::read_only(t, "DeleteAll"));
        }
        let table = self.table_mut(cx, t);
        let doomed: Vec<Key> = table
            .iter()
            .filter(|(_, r)| passes(r, f))
            .map(|(k, _)| k.clone())
            .collect();
        for k in &doomed {
            table.remove(k);
        }
        if !doomed.is_empty() {
            self.touch(cx, t);
        }
        Ok(doomed.len())
    }

    fn commit(&mut self) {
        self.committed = self.tables.clone();
        self.committed_versions = self.versions.clone();
    }

    fn rollback(&mut self) {
        self.tables = self.committed.clone();
        self.versions = self.committed_versions.clone();
    }
}

impl SnapshotStore for MemStore {
    type Snapshot = MemStore;
    fn snapshot(&self) -> MemStore {
        self.clone()
    }
    fn restore(&mut self, s: MemStore) {
        let counter = self.counter.max(s.counter);
        *self = s;
        self.counter = counter;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passes_the_store_contract() {
        crate::contract::run_all(MemStore::default);
    }

    #[test]
    fn dropping_a_company_removes_its_rows_and_a_rollback_restores_them() {
        let cx = |c: &str| AccessContext {
            company: c.into(),
            user: "U".into(),
        };
        let t = TableDef {
            object: 7,
            name: "T".into(),
            fields: vec![FieldDef {
                id: 1,
                name: "K".into(),
                ty: al_builtins::value::RtType::Integer,
                stored: true,
            }],
            keys: vec![vec![0]],
            per_company: true,
        };
        let mut s = MemStore::default();
        s.insert(&cx("A"), &t, vec![Value::Integer(1)]).unwrap();
        s.insert(&cx("B"), &t, vec![Value::Integer(1)]).unwrap();
        s.commit();
        assert!(s.has_company_rows("a"));
        assert_eq!(s.companies_with_rows(7).len(), 2);
        let before = s.table_stamp(&cx("B"), &t);
        s.drop_company("B");
        assert!(!s.has_company_rows("B") && s.has_company_rows("A"));
        assert_eq!(s.count(&cx("B"), &t, &[]).unwrap(), 0);
        assert_eq!(s.count(&cx("A"), &t, &[]).unwrap(), 1);
        assert_ne!(s.table_stamp(&cx("B"), &t), before, "queries see the change");
        s.rollback();
        assert_eq!(s.count(&cx("B"), &t, &[]).unwrap(), 1);
    }

    #[test]
    fn reports_uncommitted_writes() {
        let cx = AccessContext {
            company: "C".into(),
            user: "U".into(),
        };
        let t = TableDef {
            object: 0,
            name: "T".into(),
            fields: vec![FieldDef {
                id: 1,
                name: "K".into(),
                ty: al_builtins::value::RtType::Integer,
                stored: true,
            }],
            keys: vec![vec![0]],
            per_company: true,
        };
        let mut s = MemStore::default();
        assert!(!s.has_uncommitted());
        s.insert(&cx, &t, vec![Value::Integer(1)]).unwrap();
        assert!(s.has_uncommitted());
        s.commit();
        assert!(!s.has_uncommitted());
        s.insert(&cx, &t, vec![Value::Integer(2)]).unwrap();
        s.rollback();
        assert!(!s.has_uncommitted());
    }

    #[test]
    fn table_stamp_changes_with_every_write_and_rollback() {
        let cx = AccessContext {
            company: "C".into(),
            user: "U".into(),
        };
        let t = TableDef {
            object: 0,
            name: "T".into(),
            fields: vec![FieldDef {
                id: 1,
                name: "K".into(),
                ty: al_builtins::value::RtType::Integer,
                stored: true,
            }],
            keys: vec![vec![0]],
            per_company: true,
        };
        let mut s = MemStore::default();
        let s0 = s.table_stamp(&cx, &t);
        s.insert(&cx, &t, vec![Value::Integer(1)]).unwrap();
        let s1 = s.table_stamp(&cx, &t);
        assert_ne!(s0, s1, "an insert changes the stamp");
        s.insert(&cx, &t, vec![Value::Integer(1)]).unwrap();
        assert_eq!(s1, s.table_stamp(&cx, &t), "a refused insert does not");
        s.commit();
        s.insert(&cx, &t, vec![Value::Integer(2)]).unwrap();
        let s2 = s.table_stamp(&cx, &t);
        s.rollback();
        assert_eq!(s1, s.table_stamp(&cx, &t), "a rollback restores the committed stamp");
        s.insert(&cx, &t, vec![Value::Integer(3)]).unwrap();
        assert_ne!(s2, s.table_stamp(&cx, &t), "a stamp is never reused for other content");
    }

    #[test]
    fn passes_the_snapshot_contract() {
        crate::contract::snapshot_restores_everything(MemStore::default());
    }
}

#[cfg(test)]
mod virtual_tests {
    use super::*;
    use al_builtins::value::RtType;
    use std::rc::Rc;

    /// A table that has one row, 7, and is served by a provider.
    struct One;
    impl VirtualTables for One {
        fn serves(&self, t: &TableDef) -> bool {
            t.name == "V"
        }
        fn get(&self, _: &MemStore, _: &AccessContext, _: &TableDef, pk: &[Value]) -> Result<Option<Row>, StoreError> {
            Ok(matches!(pk, [Value::Integer(7)]).then(|| vec![Value::Integer(7)]))
        }
        fn next(&self, _: &MemStore, _: &AccessContext, _: &TableDef, scan: &Scan) -> Result<Option<Row>, StoreError> {
            Ok(scan.after.is_none().then(|| vec![Value::Integer(7)]))
        }
        fn count(&self, _: &MemStore, _: &AccessContext, _: &TableDef, _: &[FieldFilter]) -> Result<usize, StoreError> {
            Ok(1)
        }
    }

    fn table(name: &str) -> TableDef {
        TableDef {
            object: 0,
            name: name.into(),
            fields: vec![FieldDef {
                id: 1,
                name: "K".into(),
                ty: RtType::Integer,
                stored: true,
            }],
            keys: vec![vec![0]],
            per_company: false,
        }
    }

    #[test]
    fn a_served_table_is_read_from_the_provider_and_never_written() {
        let cx = AccessContext {
            company: "C".into(),
            user: "U".into(),
        };
        let mut s = MemStore::with_virtual_tables(Rc::new(One));
        let v = table("V");
        assert_eq!(s.count(&cx, &v, &[]), Ok(1));
        assert!(s.get(&cx, &v, &[Value::Integer(7)]).unwrap().is_some());
        assert!(s.get(&cx, &v, &[Value::Integer(8)]).unwrap().is_none());
        for r in [
            s.insert(&cx, &v, vec![Value::Integer(8)]).map(|_| ()),
            s.modify(&cx, &v, vec![Value::Integer(7)]).map(|_| ()),
            s.delete(&cx, &v, &[Value::Integer(7)]).map(|_| ()),
            s.delete_all(&cx, &v, &[]).map(|_| ()),
        ] {
            assert!(matches!(r, Err(StoreError::Refused(_))), "{r:?}");
        }
        assert!(!s.has_uncommitted(), "a refused write leaves nothing pending");
        // A table the provider does not serve is an ordinary stored table.
        let w = table("W");
        s.insert(&cx, &w, vec![Value::Integer(1)]).unwrap();
        assert_eq!(s.count(&cx, &w, &[]), Ok(1));
    }
}
