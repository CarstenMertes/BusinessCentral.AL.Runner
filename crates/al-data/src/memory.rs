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
    /// Whether any write since the last commit or rollback is pending.
    pub fn has_uncommitted(&self) -> bool {
        !(self.tables.ptr_eq(&self.committed) || (self.tables.is_empty() && self.committed.is_empty()))
    }

    /// Whether a write has changed the data since `mark`, a clone of this store taken earlier.
    /// O(1): persistent maps that were not written to share their root.
    pub fn written_since(&self, mark: &MemStore) -> bool {
        !(self.tables.ptr_eq(&mark.tables) || (self.tables.is_empty() && mark.tables.is_empty()))
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
        let k = Key(t.primary_key(&row));
        let table = self.table_mut(cx, t);
        if table.contains_key(&k) {
            return Ok(InsertOutcome::AlreadyExists);
        }
        table.insert(k, row);
        Ok(InsertOutcome::Inserted)
    }

    fn get(&self, cx: &AccessContext, t: &TableDef, pk: &[Value]) -> Result<Option<Row>, StoreError> {
        Ok(self.table(cx, t).and_then(|m| m.get(&Key(pk.to_vec())).cloned()))
    }

    fn modify(&mut self, cx: &AccessContext, t: &TableDef, row: Row) -> Result<WriteOutcome, StoreError> {
        let k = Key(t.primary_key(&row));
        let table = self.table_mut(cx, t);
        if !table.contains_key(&k) {
            return Ok(WriteOutcome::NotFound);
        }
        table.insert(k, row);
        Ok(WriteOutcome::Done)
    }

    fn delete(&mut self, cx: &AccessContext, t: &TableDef, pk: &[Value]) -> Result<WriteOutcome, StoreError> {
        Ok(match self.table_mut(cx, t).remove(&Key(pk.to_vec())) {
            Some(_) => WriteOutcome::Done,
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
        Ok(RenameOutcome::Renamed)
    }

    fn next(&self, cx: &AccessContext, t: &TableDef, scan: &Scan) -> Result<Option<Row>, StoreError> {
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
        Ok(self
            .table(cx, t)
            .map_or(0, |m| m.values().filter(|r| passes(r, f)).count()))
    }

    fn delete_all(&mut self, cx: &AccessContext, t: &TableDef, f: &[FieldFilter]) -> Result<usize, StoreError> {
        let table = self.table_mut(cx, t);
        let doomed: Vec<Key> = table
            .iter()
            .filter(|(_, r)| passes(r, f))
            .map(|(k, _)| k.clone())
            .collect();
        for k in &doomed {
            table.remove(k);
        }
        Ok(doomed.len())
    }

    fn commit(&mut self) {
        self.committed = self.tables.clone();
    }

    fn rollback(&mut self) {
        self.tables = self.committed.clone();
    }
}

impl SnapshotStore for MemStore {
    type Snapshot = MemStore;
    fn snapshot(&self) -> MemStore {
        self.clone()
    }
    fn restore(&mut self, s: MemStore) {
        *self = s;
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
    fn reports_writes_since_a_mark() {
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
        let mark = s.clone();
        assert!(!s.written_since(&mark));
        s.insert(&cx, &t, vec![Value::Integer(1)]).unwrap();
        assert!(s.written_since(&mark));
        assert!(s.has_uncommitted());
        let mark = s.clone();
        s.commit();
        assert!(!s.has_uncommitted());
        assert!(!s.written_since(&mark), "a commit changes no data");
        s.insert(&cx, &t, vec![Value::Integer(2)]).unwrap();
        s.rollback();
        assert!(!s.written_since(&mark), "the rollback restored the state at the mark");
    }

    #[test]
    fn passes_the_snapshot_contract() {
        crate::contract::snapshot_restores_everything(MemStore::default());
    }
}
