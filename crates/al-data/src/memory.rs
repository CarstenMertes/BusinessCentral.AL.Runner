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
/// A table's place in the store: its company (empty for a table that is not per company) and its object. The company
/// is a shared name, so building a slot for an operation copies a pointer, not a string.
type Slot = (std::sync::Arc<str>, u32);
type Tables = im::OrdMap<Slot, Table>;

/// The rows of one table, built to be handed to a [`MemStore`] by a [`Backing`].
#[derive(Clone)]
pub struct Rows(Table, i64);

impl Rows {
    /// The largest value of the table's AutoIncrement field among the rows (0 when it has none):
    /// the counter continues above it, as an identity column does after a restore.
    pub fn with_auto_increment_floor(mut self, floor: i64) -> Rows {
        self.1 = floor;
        self
    }

    pub fn auto_increment_floor(&self) -> i64 {
        self.1
    }

    /// The table `rows` make. Two rows with the same primary key are an error carrying how many
    /// rows were lost to it: a table is never built from rows that overwrite each other.
    pub fn from_rows(t: &TableDef, rows: Vec<Row>) -> Result<Rows, usize> {
        let n = rows.len();
        let map: Table = rows.into_iter().map(|r| (Key(t.primary_key(&r)), r)).collect();
        if map.len() == n {
            Ok(Rows(map, 0))
        } else {
            Err(n - map.len())
        }
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// Rows a table holds before any write to it, read when the table is first touched (ADR-020: test
/// data). A table the store has no entry for reads as the backing's rows; the first write copies
/// them into the store, and from then on the store holds the table. A rollback to a state without
/// the entry reads the backing again: its rows are committed data.
pub trait Backing {
    /// The company the rows belong to.
    fn company(&self) -> &str;
    /// The rows of table `object` for `company` (the empty name for a table that is not per
    /// company); `None` when the backing has none to offer. Answering is cheap after the first
    /// time: a clone of a shared map.
    fn rows(&self, company: &str, object: u32) -> Option<Rows>;
    /// Whether the table is served at all (a table with rows to offer in some company).
    fn has_rows(&self, object: u32) -> bool {
        self.rows(self.company(), object)
            .or_else(|| self.rows("", object))
            .is_some_and(|r| !r.is_empty())
    }
}

/// A table's rows in the order of one key, to step through them without a pass over the table per step.
#[derive(Clone)]
struct OrderEntry {
    shape: u64,
    rows: usize,
    order: std::rc::Rc<Vec<Key>>,
}

/// Tables with fewer rows than this are walked in a secondary key by looking at every row.
const INDEX_MIN_ROWS: usize = 64;

#[derive(Clone, Default)]
pub struct MemStore {
    tables: Tables,
    committed: Tables,
    /// The value of `counter` when each table last changed. It follows `tables` through commit and
    /// rollback, so a table has the same stamp exactly when it has the same content history.
    versions: im::OrdMap<Slot, u64>,
    committed_versions: im::OrdMap<Slot, u64>,
    /// The value of `counter` when the *shape* of each table last changed: a row was inserted, deleted or
    /// renamed, or a Modify changed a field some key is made of. It follows `tables` like `versions`, so
    /// equal stamps mean the same rows in the same places in every key.
    shapes: im::OrdMap<Slot, u64>,
    committed_shapes: im::OrdMap<Slot, u64>,
    /// The primary keys of a big table in the order of one of its other keys, as of a shape stamp.
    order_cache: std::cell::RefCell<std::collections::HashMap<(Slot, usize), OrderEntry>>,
    /// Handed out by writes; never goes back, so a stamp is never reused.
    counter: u64,
    /// Tables that have no stored rows (ADR-019).
    virtuals: Option<std::rc::Rc<dyn VirtualTables>>,
    /// Rows a table holds before anything wrote to it (ADR-020).
    backing: Option<std::rc::Rc<dyn Backing>>,
    /// While set, a change to a row of the backing (a modify, delete or rename) is written down
    /// here and allowed (ADR-020 amendment: install code may change restored rows). Shared by the
    /// clones of the store, so a snapshot or a rollback does not lose it.
    restored_log: Option<std::rc::Rc<std::cell::RefCell<Vec<RestoredChange>>>>,
}

/// A change to a row the restored company holds, made while recording was on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestoredChange {
    pub table: String,
    /// `Modify`, `Delete`, `Rename` or `DeleteAll`.
    pub op: &'static str,
    /// A Modify that writes the values the row already has, system fields apart. Every other change is `false`.
    pub same: bool,
    /// For a Modify with other values: the names of the fields whose values differ.
    pub fields: Vec<String>,
}

thread_local! {
    /// The company of a table that is not per company; one per thread, so threads do not share its count.
    static NO_COMPANY: std::sync::Arc<str> = std::sync::Arc::from("");
}

fn slot(cx: &AccessContext, t: &TableDef) -> Slot {
    (
        if t.per_company {
            cx.company.clone()
        } else {
            NO_COMPANY.with(|e| e.clone())
        },
        t.object,
    )
}

/// Whether `new` differs from `old` in a field some key of `t` is made of (the primary key included).
fn key_fields_differ(t: &TableDef, old: &[Value], new: &[Value]) -> bool {
    t.keys
        .iter()
        .flatten()
        .any(|&i| al_builtins::ops::compare(&old[i], &new[i]).map_or(true, |o| o != Ordering::Equal))
}

/// `key_cmp(position(t, key, row), other)` without building the position.
fn cmp_row_to_position(t: &TableDef, key: usize, row: &[Value], other: &[Value]) -> Ordering {
    for (&i, x) in t.keys[key].iter().chain(&t.keys[0]).zip(other) {
        match al_builtins::ops::compare(&row[i], x).unwrap_or(Ordering::Equal) {
            Ordering::Equal => continue,
            o => return o,
        }
    }
    (t.keys[key].len() + t.keys[0].len()).cmp(&other.len())
}

/// `key_cmp(position(t, key, a), position(t, key, b))` without building the positions.
fn cmp_rows(t: &TableDef, key: usize, a: &[Value], b: &[Value]) -> Ordering {
    for &i in t.keys[key].iter().chain(&t.keys[0]) {
        match al_builtins::ops::compare(&a[i], &b[i]).unwrap_or(Ordering::Equal) {
            Ordering::Equal => continue,
            o => return o,
        }
    }
    Ordering::Equal
}

fn passes(row: &Row, filters: &[FieldFilter]) -> bool {
    al_builtins::filter::row_passes(row, filters)
}

impl MemStore {
    /// A store whose virtual tables are answered by `v`.
    pub fn with_virtual_tables(v: std::rc::Rc<dyn VirtualTables>) -> MemStore {
        MemStore {
            virtuals: Some(v),
            ..Default::default()
        }
    }

    /// This store with `b` as the rows its untouched tables read.
    pub fn with_backing(mut self, b: std::rc::Rc<dyn Backing>) -> MemStore {
        self.backing = Some(b);
        self
    }

    /// Starts (`true`) or stops (`false`) writing down the changes made to rows the backing holds.
    /// Changes are never refused (ADR-020 amendment). Starting clears the record.
    pub fn record_restored_changes(&mut self, on: bool) {
        self.restored_log = on.then(Default::default);
    }

    /// The changes written down since recording started.
    pub fn restored_changes(&self) -> Vec<RestoredChange> {
        self.restored_log
            .as_ref()
            .map(|l| l.borrow().clone())
            .unwrap_or_default()
    }

    /// Writes down a change to the row with this key if recording is on and the backing holds the
    /// row. `new` is the row a Modify writes.
    fn note_restored(&self, cx: &AccessContext, t: &TableDef, key: &Key, op: &'static str, new: Option<&Row>) {
        let Some(log) = &self.restored_log else { return };
        let slot = slot(cx, t);
        let Some(rows) = self.backing.as_ref().and_then(|b| b.rows(&slot.0, slot.1)) else {
            return;
        };
        let Some(old) = rows.0.get(key) else { return };
        let fields: Vec<String> = new
            .map(|n| {
                (0..old.len().min(n.len()))
                    .filter(|&i| format!("{:?}", n[i]) != format!("{:?}", old[i]))
                    // The system fields (id from 2000000000: SystemModifiedAt, SystemRowVersion ...) change
                    // with every write and do not count as a different value.
                    .filter_map(|i| t.fields.get(i).filter(|f| f.id < 2_000_000_000).map(|f| f.name.clone()))
                    .collect()
            })
            .unwrap_or_default();
        let same = new.is_some() && fields.is_empty();
        log.borrow_mut().push(RestoredChange {
            table: t.name.clone(),
            op,
            same,
            fields,
        });
    }

    /// The rows table `object` has to offer `company`, if the store has a backing.
    pub fn backed_rows(&self, company: &str, object: u32) -> Option<Rows> {
        self.backing.as_ref()?.rows(company, object)
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

    fn read_only(&self, t: &TableDef, what: &str) -> StoreError {
        if let Some(why) = self.virtuals.as_deref().and_then(|v| v.write_refusal(t)) {
            return StoreError::Refused(why);
        }
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
        if self.tables.iter().any(|((_, o), t)| *o == object && !t.is_empty()) {
            return true;
        }
        // A table the store holds is what its rows are now; only an absent one reads the backing.
        let held = self.tables.keys().any(|(_, o)| *o == object);
        !held && self.backing.as_ref().is_some_and(|b| b.has_rows(object))
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
            .any(|((c, _), t)| !c.is_empty() && &**c == company && !t.is_empty())
            || self
                .backing
                .as_ref()
                .is_some_and(|b| !company.is_empty() && b.company().eq_ignore_ascii_case(company))
    }

    /// The companies that hold a row in per-company table `object`, committed or not.
    pub fn companies_with_rows(&self, object: u32) -> Vec<String> {
        let mut out: Vec<String> = self
            .tables
            .iter()
            .filter(|((c, o), t)| *o == object && !c.is_empty() && !t.is_empty())
            .map(|((c, _), _)| c.to_string())
            .collect();
        if let Some(b) = &self.backing {
            let company = b.company();
            let held = self.tables.contains_key(&(std::sync::Arc::from(company), object));
            if !held && b.rows(company, object).is_some_and(|r| !r.is_empty()) {
                out.push(company.to_string());
            }
        }
        out
    }

    /// Removes every row of `company`'s tables, as deleting the company does. Part of the open
    /// transaction: a rollback puts the rows back.
    pub fn drop_company(&mut self, company: &str) {
        let doomed: Vec<Slot> = self
            .tables
            .keys()
            .filter(|(c, _)| !c.is_empty() && &**c == company)
            .cloned()
            .collect();
        for slot in doomed {
            self.tables.remove(&slot);
            self.counter += 1;
            self.versions.insert(slot.clone(), self.counter);
            self.shapes.insert(slot, self.counter);
        }
    }

    /// Moves every row of company `old` to company `new`, as renaming the company does
    /// (record/TestChangeCompanyFiltersCopyDeleteContracts.al:170-212). Part of the open transaction.
    pub fn rename_company(&mut self, old: &str, new: &str) {
        let moved: Vec<Slot> = self
            .tables
            .keys()
            .filter(|(c, _)| !c.is_empty() && &**c == old)
            .cloned()
            .collect();
        for slot in moved {
            if let Some(table) = self.tables.remove(&slot) {
                let target: Slot = (std::sync::Arc::from(new), slot.1);
                self.tables.insert(target.clone(), table);
                self.counter += 1;
                self.versions.insert(slot.clone(), self.counter);
                self.shapes.insert(slot, self.counter);
                self.counter += 1;
                self.versions.insert(target.clone(), self.counter);
                self.shapes.insert(target, self.counter);
            }
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

    /// `touch` for a change of the table's shape (see `shapes`).
    fn touch_shape(&mut self, cx: &AccessContext, t: &TableDef) {
        self.touch(cx, t);
        self.shapes.insert(slot(cx, t), self.counter);
    }

    fn table(&self, cx: &AccessContext, t: &TableDef) -> Option<std::borrow::Cow<'_, Table>> {
        let slot = slot(cx, t);
        match self.tables.get(&slot) {
            Some(table) => Some(std::borrow::Cow::Borrowed(table)),
            None => self
                .backing
                .as_ref()?
                .rows(&slot.0, slot.1)
                .map(|r| std::borrow::Cow::Owned(r.0)),
        }
    }

    /// The primary keys of `table` in the order of key `key`, kept while the shape of the table stays.
    fn order_of(&self, slot: &Slot, key: usize, t: &TableDef, table: &Table) -> std::rc::Rc<Vec<Key>> {
        let shape = self.shapes.get(slot).copied().unwrap_or(0);
        let ck = (slot.clone(), key);
        if let Some(e) = self.order_cache.borrow().get(&ck) {
            if e.shape == shape && e.rows == table.len() {
                return e.order.clone();
            }
        }
        let mut rows: Vec<(&Key, &Row)> = table.iter().collect();
        rows.sort_by(|a, b| cmp_rows(t, key, a.1, b.1));
        let order = std::rc::Rc::new(rows.into_iter().map(|(k, _)| k.clone()).collect::<Vec<_>>());
        self.order_cache.borrow_mut().insert(
            ck,
            OrderEntry {
                shape,
                rows: table.len(),
                order: order.clone(),
            },
        );
        order
    }

    fn table_mut(&mut self, cx: &AccessContext, t: &TableDef) -> &mut Table {
        let slot = slot(cx, t);
        if !self.tables.contains_key(&slot) {
            let start = self
                .backing
                .as_ref()
                .and_then(|b| b.rows(&slot.0, slot.1))
                .map_or_else(Table::new, |r| r.0);
            self.tables.insert(slot.clone(), start);
        }
        self.tables.get_mut(&slot).expect("the slot was just inserted")
    }
}

impl Store for MemStore {
    fn insert(&mut self, cx: &AccessContext, t: &TableDef, row: Row) -> Result<InsertOutcome, StoreError> {
        if self.virtual_of(t).is_some() {
            return Err(self.read_only(t, "Insert"));
        }
        let k = Key(t.primary_key(&row));
        let table = self.table_mut(cx, t);
        if table.contains_key(&k) {
            return Ok(InsertOutcome::AlreadyExists);
        }
        table.insert(k, row);
        self.touch_shape(cx, t);
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
            return Err(self.read_only(t, "Modify"));
        }
        let k = Key(t.primary_key(&row));
        self.note_restored(cx, t, &k, "Modify", Some(&row));
        let table = self.table_mut(cx, t);
        if !table.contains_key(&k) {
            return Ok(WriteOutcome::NotFound);
        }
        // A key is made of fields; when a Modify changes one the row stands elsewhere in that key.
        let moved = table.get(&k).is_some_and(|old| key_fields_differ(t, old, &row));
        table.insert(k, row);
        if moved {
            self.touch_shape(cx, t);
        } else {
            self.touch(cx, t);
        }
        Ok(WriteOutcome::Done)
    }

    fn delete(&mut self, cx: &AccessContext, t: &TableDef, pk: &[Value]) -> Result<WriteOutcome, StoreError> {
        if self.virtual_of(t).is_some() {
            return Err(self.read_only(t, "Delete"));
        }
        self.note_restored(cx, t, &Key(pk.to_vec()), "Delete", None);
        Ok(match self.table_mut(cx, t).remove(&Key(pk.to_vec())) {
            Some(_) => {
                self.touch_shape(cx, t);
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
            return Err(self.read_only(t, "Rename"));
        }
        let new_k = Key(t.primary_key(&row));
        self.note_restored(cx, t, &Key(old.to_vec()), "Rename", None);
        let table = self.table_mut(cx, t);
        if !table.contains_key(&Key(old.to_vec())) {
            return Ok(RenameOutcome::NotFound);
        }
        if table.contains_key(&new_k) {
            return Ok(RenameOutcome::AlreadyExists);
        }
        table.remove(&Key(old.to_vec()));
        table.insert(new_k, row);
        self.touch_shape(cx, t);
        Ok(RenameOutcome::Renamed)
    }

    fn next(&self, cx: &AccessContext, t: &TableDef, scan: &Scan) -> Result<Option<Row>, StoreError> {
        if let Some(v) = self.virtual_of(t) {
            return v.next(self, cx, t, scan);
        }
        let Some(table) = self.table(cx, t) else {
            return Ok(None);
        };
        // In primary key order the table is already in the order of the scan: step to the first row past
        // the position and look at the rows from there on until one passes the filters.
        if scan.key == 0 {
            use std::ops::Bound::{Excluded, Unbounded};
            let k = t.keys[0].len();
            let after = scan.after.map(|a| Key(a[..k.min(a.len())].to_vec()));
            if scan.ascending {
                let lower = after.map_or(Unbounded, Excluded);
                return Ok(table
                    .range((lower, Unbounded))
                    .map(|(_, r)| r)
                    .find(|r| passes(r, scan.filters))
                    .cloned());
            }
            let upper = after.map_or(Unbounded, Excluded);
            return Ok(table
                .range((Unbounded, upper))
                .rev()
                .map(|(_, r)| r)
                .find(|r| passes(r, scan.filters))
                .cloned());
        }
        // A big table in another key: step through the table's rows in that key's order.
        if table.len() >= INDEX_MIN_ROWS {
            let slot = slot(cx, t);
            let order = self.order_of(&slot, scan.key, t, &table);
            let row_at = |k: &Key| table.get(k);
            if order.len() == table.len() && order.iter().take(1).all(|k| row_at(k).is_some()) {
                let found = if scan.ascending {
                    let start = scan.after.map_or(0, |a| {
                        order.partition_point(|k| {
                            row_at(k).is_some_and(|r| cmp_row_to_position(t, scan.key, r, a) != Ordering::Greater)
                        })
                    });
                    order[start..]
                        .iter()
                        .filter_map(row_at)
                        .find(|r| passes(r, scan.filters))
                } else {
                    let end = scan.after.map_or(order.len(), |a| {
                        order.partition_point(|k| {
                            row_at(k).is_some_and(|r| cmp_row_to_position(t, scan.key, r, a) == Ordering::Less)
                        })
                    });
                    order[..end]
                        .iter()
                        .rev()
                        .filter_map(row_at)
                        .find(|r| passes(r, scan.filters))
                };
                return Ok(found.cloned());
            }
        }
        // A small table in another key: every row is looked at, without building its position (a copy of the key fields).
        let mut best: Option<&Row> = None;
        for row in table.values().filter(|r| passes(r, scan.filters)) {
            let after_ok = scan.after.is_none_or(|a| {
                let o = cmp_row_to_position(t, scan.key, row, a);
                if scan.ascending {
                    o == Ordering::Greater
                } else {
                    o == Ordering::Less
                }
            });
            if !after_ok {
                continue;
            }
            let better = best.is_none_or(|b| {
                let o = cmp_rows(t, scan.key, row, b);
                if scan.ascending {
                    o == Ordering::Less
                } else {
                    o == Ordering::Greater
                }
            });
            if better {
                best = Some(row);
            }
        }
        Ok(best.cloned())
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
            return Err(self.read_only(t, "DeleteAll"));
        }
        let doomed: Vec<Key> = self
            .table(cx, t)
            .map(|table| {
                table
                    .iter()
                    .filter(|(_, r)| passes(r, f))
                    .map(|(k, _)| k.clone())
                    .collect()
            })
            .unwrap_or_default();
        for k in &doomed {
            self.note_restored(cx, t, k, "DeleteAll", None);
        }
        let table = self.table_mut(cx, t);
        for k in &doomed {
            table.remove(k);
        }
        if !doomed.is_empty() {
            self.touch_shape(cx, t);
        }
        Ok(doomed.len())
    }

    fn commit(&mut self) {
        self.committed = self.tables.clone();
        self.committed_versions = self.versions.clone();
        self.committed_shapes = self.shapes.clone();
    }

    fn rollback(&mut self) {
        self.tables = self.committed.clone();
        self.versions = self.committed_versions.clone();
        self.shapes = self.committed_shapes.clone();
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
        assert!(
            s.has_company_rows("A") && !s.has_company_rows("a"),
            "company names are case sensitive"
        );
        assert_eq!(s.companies_with_rows(7).len(), 2);
        let before = s.table_stamp(&cx("B"), &t);
        s.drop_company("B");
        assert!(!s.has_company_rows("B") && s.has_company_rows("A"));
        assert_eq!(s.count(&cx("B"), &t, &[]).unwrap(), 0);
        assert_eq!(s.count(&cx("A"), &t, &[]).unwrap(), 1);
        assert_ne!(s.table_stamp(&cx("B"), &t), before, "queries see the change");
        s.rollback();
        assert_eq!(s.count(&cx("B"), &t, &[]).unwrap(), 1);
        s.rename_company("B", "C");
        assert_eq!(s.count(&cx("B"), &t, &[]).unwrap(), 0);
        assert_eq!(s.count(&cx("C"), &t, &[]).unwrap(), 1);
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

    /// Rows a table holds before anything wrote to it: three rows for company `C`, table 7.
    struct ThreeRows {
        reads: std::cell::Cell<usize>,
    }

    impl Backing for ThreeRows {
        fn company(&self) -> &str {
            "C"
        }
        fn rows(&self, company: &str, object: u32) -> Option<Rows> {
            if company != "C" || object != 7 {
                return None;
            }
            self.reads.set(self.reads.get() + 1);
            let t = crate::contract::table();
            Rows::from_rows(
                &t,
                vec![
                    crate::contract::row("A", 1, 1),
                    crate::contract::row("B", 2, 1),
                    crate::contract::row("C", 3, 2),
                ],
            )
            .ok()
        }
    }

    fn backed() -> (MemStore, std::rc::Rc<ThreeRows>) {
        let b = std::rc::Rc::new(ThreeRows {
            reads: Default::default(),
        });
        (MemStore::default().with_backing(b.clone()), b)
    }

    #[test]
    fn a_table_nothing_wrote_to_reads_the_backing_rows() {
        let (s, _) = backed();
        let (t, cx) = (crate::contract::table(), crate::contract::cx("C"));
        assert_eq!(s.count(&cx, &t, &[]).unwrap(), 3);
        assert!(s.get(&cx, &t, &[Value::Code("B".into())]).unwrap().is_some());
        // another company and another table see nothing
        assert_eq!(s.count(&crate::contract::cx("D"), &t, &[]).unwrap(), 0);
        let mut other = t.clone();
        other.object = 8;
        assert_eq!(s.count(&cx, &other, &[]).unwrap(), 0);
    }

    #[test]
    fn a_write_starts_from_the_backing_rows() {
        let (mut s, _) = backed();
        let (t, cx) = (crate::contract::table(), crate::contract::cx("C"));
        assert_eq!(
            s.insert(&cx, &t, crate::contract::row("A", 9, 9)).unwrap(),
            InsertOutcome::AlreadyExists,
            "a restored key cannot be inserted again"
        );
        assert_eq!(
            s.insert(&cx, &t, crate::contract::row("D", 4, 3)).unwrap(),
            InsertOutcome::Inserted
        );
        assert_eq!(s.count(&cx, &t, &[]).unwrap(), 4);
        assert_eq!(
            s.delete(&cx, &t, &[Value::Code("A".into())]).unwrap(),
            WriteOutcome::Done
        );
        assert_eq!(s.count(&cx, &t, &[]).unwrap(), 3);
    }

    #[test]
    fn deleting_every_restored_row_leaves_the_table_empty_not_restored_again() {
        let (mut s, _) = backed();
        let (t, cx) = (crate::contract::table(), crate::contract::cx("C"));
        assert_eq!(s.delete_all(&cx, &t, &[]).unwrap(), 3);
        assert_eq!(s.count(&cx, &t, &[]).unwrap(), 0);
        s.commit();
        s.rollback();
        assert_eq!(s.count(&cx, &t, &[]).unwrap(), 0);
    }

    #[test]
    fn a_rollback_puts_the_backing_rows_back_and_a_commit_keeps_a_write() {
        let (mut s, _) = backed();
        let (t, cx) = (crate::contract::table(), crate::contract::cx("C"));
        s.insert(&cx, &t, crate::contract::row("D", 4, 3)).unwrap();
        s.rollback();
        assert_eq!(
            s.count(&cx, &t, &[]).unwrap(),
            3,
            "the restored rows are committed data"
        );
        s.insert(&cx, &t, crate::contract::row("D", 4, 3)).unwrap();
        s.commit();
        s.rollback();
        assert_eq!(s.count(&cx, &t, &[]).unwrap(), 4);
    }

    #[test]
    fn a_snapshot_taken_before_a_touch_still_reads_the_backing_rows() {
        let (s, _) = backed();
        let (t, cx) = (crate::contract::table(), crate::contract::cx("C"));
        let snap = s.snapshot();
        let mut s = s;
        s.insert(&cx, &t, crate::contract::row("D", 4, 3)).unwrap();
        s.restore(snap);
        assert_eq!(s.count(&cx, &t, &[]).unwrap(), 3);
    }

    #[test]
    fn the_company_and_table_queries_include_the_backing_rows() {
        let (s, _) = backed();
        assert!(s.has_rows(7) && !s.has_rows(8));
        assert!(s.has_company_rows("c") && !s.has_company_rows("D"));
        assert_eq!(s.companies_with_rows(7), vec!["C".to_string()]);
    }

    #[test]
    fn changes_to_backing_rows_are_allowed_and_written_down_when_recording() {
        let (mut s, _) = backed();
        let (t, cx) = (crate::contract::table(), crate::contract::cx("C"));
        s.record_restored_changes(true);
        // the same values: allowed, and said to be the same
        let same = s.get(&cx, &t, &[Value::Code("A".into())]).unwrap().unwrap();
        assert_eq!(s.modify(&cx, &t, same).unwrap(), WriteOutcome::Done);
        // other values, a delete, a rename, a DeleteAll: allowed
        assert_eq!(
            s.modify(&cx, &t, crate::contract::row("A", 9, 9)).unwrap(),
            WriteOutcome::Done
        );
        assert_eq!(
            s.rename(&cx, &t, &[Value::Code("B".into())], crate::contract::row("Z", 1, 1))
                .unwrap(),
            RenameOutcome::Renamed
        );
        // a row the code made itself is not a restored row
        s.insert(&cx, &t, crate::contract::row("D", 4, 3)).unwrap();
        s.modify(&cx, &t, crate::contract::row("D", 5, 3)).unwrap();
        s.delete(&cx, &t, &[Value::Code("D".into())]).unwrap();
        let got: Vec<(&str, bool)> = s.restored_changes().iter().map(|c| (c.op, c.same)).collect();
        assert_eq!(got, vec![("Modify", true), ("Modify", false), ("Rename", false)]);
        assert!(s.restored_changes().iter().all(|c| c.table == t.name));
        s.delete_all(&cx, &t, &[]).unwrap();
        assert_eq!(s.count(&cx, &t, &[]).unwrap(), 0);
        s.record_restored_changes(false);
        assert!(s.restored_changes().is_empty());
    }

    /// Walking a table row by row costs a few comparisons a row, not a pass over the table: 30,000 rows in a
    /// second, where a scan that looks at every row for each step needs minutes. Release only
    /// (`cargo test --release -p al-data -- --ignored`).
    #[test]
    #[ignore]
    fn walking_a_big_table_row_by_row_is_not_quadratic() {
        let (t, cx) = (crate::contract::table(), crate::contract::cx("C"));
        let mut s = MemStore::default();
        for i in 0..30_000 {
            s.insert(&cx, &t, crate::contract::row(&format!("{i:08}"), i, i % 7))
                .unwrap();
        }
        let started = std::time::Instant::now();
        for key in [0, 1] {
            let rows = crate::contract::scan_all(&s, &cx, &t, key, &[], true);
            assert_eq!(rows.len(), 30_000);
        }
        let took = started.elapsed();
        assert!(took < std::time::Duration::from_secs(5), "two walks took {took:?}");
    }

    #[test]
    fn the_backing_is_asked_again_only_while_the_slot_is_absent() {
        let (mut s, b) = backed();
        let (t, cx) = (crate::contract::table(), crate::contract::cx("C"));
        s.insert(&cx, &t, crate::contract::row("D", 4, 3)).unwrap();
        let after_write = b.reads.get();
        for _ in 0..5 {
            s.count(&cx, &t, &[]).unwrap();
        }
        assert_eq!(
            b.reads.get(),
            after_write,
            "a table the store holds is not read from the backing again"
        );
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
