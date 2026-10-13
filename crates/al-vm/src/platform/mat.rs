//! A computed table whose rows are listed in full once, then scanned like a stored one.

use al_builtins::filter::{FieldFilter, key_cmp};
use al_builtins::value::Value;
use al_data::{Row, Scan, StoreError, TableDef, position};
use std::cmp::Ordering;

/// One row of a listed table. A row that could not be worked out (the declaration says something
/// the provider does not understand) is `poison`: it has its key and the reason, and a read that
/// might touch it is refused, while a read that cannot (the key filters exclude it) is answered.
pub(crate) struct Entry {
    pub row: Row,
    pub poison: Option<String>,
}

impl Entry {
    pub(crate) fn ok(row: Row) -> Entry {
        Entry { row, poison: None }
    }
    pub(crate) fn poisoned(key_only: Row, why: String) -> Entry {
        Entry {
            row: key_only,
            poison: Some(why),
        }
    }
}

/// All rows of one table, sorted by primary key.
pub(crate) struct Mat {
    rows: Vec<Row>,
    poison: Vec<Option<String>>,
}

fn passes(row: &Row, filters: &[FieldFilter]) -> bool {
    filters.iter().all(|f| f.expr.matches(&row[f.field]))
}

/// Whether a row known only by its key can pass: the filters on key columns are decided, the
/// others could go either way.
fn may_pass(t: &TableDef, row: &Row, filters: &[FieldFilter]) -> bool {
    filters
        .iter()
        .filter(|f| t.keys[0].contains(&f.field))
        .all(|f| f.expr.matches(&row[f.field]))
}

fn refusal(t: &TableDef, why: &str) -> StoreError {
    StoreError::Refused(format!("{}: {why}", t.name))
}

impl Mat {
    pub(crate) fn new(t: &TableDef, mut entries: Vec<Entry>) -> Mat {
        entries.sort_by(|a, b| key_cmp(&t.primary_key(&a.row), &t.primary_key(&b.row)));
        entries.dedup_by(|a, b| key_cmp(&t.primary_key(&a.row), &t.primary_key(&b.row)) == Ordering::Equal);
        let (rows, poison) = entries.into_iter().map(|e| (e.row, e.poison)).unzip();
        Mat { rows, poison }
    }

    pub(crate) fn get(&self, t: &TableDef, pk: &[Value]) -> Result<Option<Row>, StoreError> {
        Ok(match self.rows.binary_search_by(|r| key_cmp(&t.primary_key(r), pk)) {
            Ok(i) => match &self.poison[i] {
                Some(why) => return Err(refusal(t, why)),
                None => Some(self.rows[i].clone()),
            },
            Err(_) => None,
        })
    }

    /// The first row at or after index `from` (walking towards `ascending`) that passes the
    /// filters; a row that cannot be worked out and might pass refuses.
    fn walk(
        &self,
        t: &TableDef,
        range: std::ops::Range<usize>,
        ascending: bool,
        filters: &[FieldFilter],
    ) -> Result<Option<Row>, StoreError> {
        let at = |i: usize| -> Result<Option<Row>, StoreError> {
            let r = &self.rows[i];
            match &self.poison[i] {
                Some(why) if may_pass(t, r, filters) => Err(refusal(t, why)),
                Some(_) => Ok(None),
                None => Ok(passes(r, filters).then(|| r.clone())),
            }
        };
        if ascending {
            for i in range {
                if let Some(r) = at(i)? {
                    return Ok(Some(r));
                }
            }
        } else {
            for i in range.rev() {
                if let Some(r) = at(i)? {
                    return Ok(Some(r));
                }
            }
        }
        Ok(None)
    }

    pub(crate) fn next(&self, t: &TableDef, scan: &Scan) -> Result<Option<Row>, StoreError> {
        let later = |r: &Row| match scan.after {
            None => true,
            Some(a) => {
                let o = key_cmp(&position(t, scan.key, r), a);
                if scan.ascending {
                    o == Ordering::Greater
                } else {
                    o == Ordering::Less
                }
            }
        };
        if scan.key == 0 {
            // Sorted by this very key: start where the position says and walk.
            let start = match scan.after {
                None => 0,
                Some(a) => self
                    .rows
                    .partition_point(|r| key_cmp(&position(t, 0, r), a) != Ordering::Greater),
            };
            if scan.ascending {
                return self.walk(t, start..self.rows.len(), true, scan.filters);
            }
            let end = match scan.after {
                None => self.rows.len(),
                Some(a) => self
                    .rows
                    .partition_point(|r| key_cmp(&position(t, 0, r), a) == Ordering::Less),
            };
            return self.walk(t, 0..end, false, scan.filters);
        }
        let mut best: Option<(Vec<Value>, &Row)> = None;
        for (i, r) in self.rows.iter().enumerate().filter(|(_, r)| later(r)) {
            if let Some(why) = &self.poison[i] {
                if may_pass(t, r, scan.filters) {
                    return Err(refusal(t, why));
                }
                continue;
            }
            if !passes(r, scan.filters) {
                continue;
            }
            let pos = position(t, scan.key, r);
            let better = best.as_ref().is_none_or(|(b, _)| {
                let o = key_cmp(&pos, b);
                if scan.ascending {
                    o == Ordering::Less
                } else {
                    o == Ordering::Greater
                }
            });
            if better {
                best = Some((pos, r));
            }
        }
        Ok(best.map(|(_, r)| r.clone()))
    }

    pub(crate) fn count(&self, t: &TableDef, filters: &[FieldFilter]) -> Result<usize, StoreError> {
        let mut n = 0;
        for (i, r) in self.rows.iter().enumerate() {
            match &self.poison[i] {
                Some(why) if may_pass(t, r, filters) => return Err(refusal(t, why)),
                Some(_) => {}
                None => n += usize::from(passes(r, filters)),
            }
        }
        Ok(n)
    }
}
