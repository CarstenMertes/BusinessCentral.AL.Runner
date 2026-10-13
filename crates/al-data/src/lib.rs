//! Storage for AL tables: the AL-shaped `Store` trait (ADR-003), the
//! `SnapshotStore` extension (ADR-004), an in-memory implementation, and the
//! contract suite every store must pass.

pub mod contract;
pub mod memory;

use al_builtins::filter::FieldFilter;
use al_builtins::value::{RtType, Value};

#[derive(Debug, Clone, PartialEq)]
pub struct FieldDef {
    pub id: u32,
    pub name: String,
    pub ty: RtType,
    /// FlowFields and FlowFilters are not stored.
    pub stored: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TableDef {
    /// Object index of the table.
    pub object: u32,
    pub name: String,
    pub fields: Vec<FieldDef>,
    /// Keys as field positions; `keys[0]` is the primary key.
    pub keys: Vec<Vec<usize>>,
    /// `DataPerCompany = false` tables are shared by all companies (ADR-005).
    pub per_company: bool,
}

impl TableDef {
    pub fn primary_key(&self, row: &[Value]) -> Vec<Value> {
        self.keys[0].iter().map(|&i| row[i].clone()).collect()
    }
}

/// Who is asking, for which company (ADR-005). Permissions come later.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccessContext {
    pub company: String,
    pub user: String,
}

pub type Row = Vec<Value>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreError {
    Conflict,
    Deadlock,
    ChangedByAnotherUser,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InsertOutcome {
    Inserted,
    AlreadyExists,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteOutcome {
    Done,
    NotFound,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenameOutcome {
    Renamed,
    NotFound,
    AlreadyExists,
}

/// A key-ordered scan: rows passing `filters`, in the order of key `key`
/// (then primary key), ascending or not, strictly after `after` (a position
/// as returned by [`Store::position`]) or from the start.
#[derive(Debug, Clone)]
pub struct Scan<'a> {
    pub key: usize,
    pub filters: &'a [FieldFilter],
    pub ascending: bool,
    pub after: Option<&'a [Value]>,
}

pub trait Store {
    fn insert(&mut self, cx: &AccessContext, t: &TableDef, row: Row) -> Result<InsertOutcome, StoreError>;
    fn get(&self, cx: &AccessContext, t: &TableDef, pk: &[Value]) -> Result<Option<Row>, StoreError>;
    fn modify(&mut self, cx: &AccessContext, t: &TableDef, row: Row) -> Result<WriteOutcome, StoreError>;
    fn delete(&mut self, cx: &AccessContext, t: &TableDef, pk: &[Value]) -> Result<WriteOutcome, StoreError>;
    fn rename(
        &mut self,
        cx: &AccessContext,
        t: &TableDef,
        old_pk: &[Value],
        row: Row,
    ) -> Result<RenameOutcome, StoreError>;
    /// The next row of a scan, if any.
    fn next(&self, cx: &AccessContext, t: &TableDef, scan: &Scan) -> Result<Option<Row>, StoreError>;
    fn count(&self, cx: &AccessContext, t: &TableDef, filters: &[FieldFilter]) -> Result<usize, StoreError>;
    fn delete_all(&mut self, cx: &AccessContext, t: &TableDef, filters: &[FieldFilter]) -> Result<usize, StoreError>;
    /// Makes every change so far durable.
    fn commit(&mut self);
    /// Discards every change since the last commit.
    fn rollback(&mut self);
}

/// The position of `row` in key `key`: key fields then primary key fields.
pub fn position(t: &TableDef, key: usize, row: &[Value]) -> Vec<Value> {
    t.keys[key].iter().chain(&t.keys[0]).map(|&i| row[i].clone()).collect()
}

/// Stores whose whole state can be captured and restored in O(1) (ADR-004).
pub trait SnapshotStore: Store {
    type Snapshot: Clone;
    fn snapshot(&self) -> Self::Snapshot;
    fn restore(&mut self, s: Self::Snapshot);
}
