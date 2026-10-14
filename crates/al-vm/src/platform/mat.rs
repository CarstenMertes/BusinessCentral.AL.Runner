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
    /// How many leading primary key columns of a poisoned row are known; the rest are placeholders
    /// that no filter may rule the row out by.
    pub known: usize,
}

impl Entry {
    pub(crate) fn ok(row: Row) -> Entry {
        Entry {
            row,
            poison: None,
            known: usize::MAX,
        }
    }
    pub(crate) fn poisoned(key_only: Row, why: String) -> Entry {
        Entry {
            row: key_only,
            poison: Some(why),
            known: usize::MAX,
        }
    }
    /// A poisoned row of which only the first `known` primary key columns are real: it stands for
    /// every row that starts with them.
    pub(crate) fn poisoned_prefix(key_only: Row, known: usize, why: String) -> Entry {
        Entry {
            row: key_only,
            poison: Some(why),
            known,
        }
    }
}

/// All rows of one table, sorted by primary key.
pub(crate) struct Mat {
    rows: Vec<Row>,
    poison: Vec<Option<String>>,
    known: Vec<usize>,
}

fn passes(row: &Row, filters: &[FieldFilter]) -> bool {
    filters.iter().all(|f| f.expr.matches(&row[f.field]))
}

/// Whether a row known only by its key can pass: the filters on key columns are decided, the
/// others could go either way.
fn may_pass(t: &TableDef, row: &Row, known: usize, filters: &[FieldFilter]) -> bool {
    filters
        .iter()
        .filter(|f| t.keys[0].iter().position(|k| *k == f.field).is_some_and(|i| i < known))
        .all(|f| f.expr.matches(&row[f.field]))
}

fn refusal(t: &TableDef, why: &str) -> StoreError {
    StoreError::Refused(format!("{}: {why}", t.name))
}

impl Mat {
    pub(crate) fn new(t: &TableDef, mut entries: Vec<Entry>) -> Mat {
        entries.sort_by(|a, b| key_cmp(&t.primary_key(&a.row), &t.primary_key(&b.row)));
        entries.dedup_by(|a, b| key_cmp(&t.primary_key(&a.row), &t.primary_key(&b.row)) == Ordering::Equal);
        let mut rows = Vec::new();
        let mut poison = Vec::new();
        let mut known = Vec::new();
        for e in entries {
            rows.push(e.row);
            poison.push(e.poison);
            known.push(e.known);
        }
        Mat { rows, poison, known }
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
                Some(why) if may_pass(t, r, self.known[i], filters) => Err(refusal(t, why)),
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
                if may_pass(t, r, self.known[i], scan.filters) {
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
                Some(why) if may_pass(t, r, self.known[i], filters) => return Err(refusal(t, why)),
                Some(_) => {}
                None => n += usize::from(passes(r, filters)),
            }
        }
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use al_builtins::filter::FilterExpr;
    use al_builtins::value::RtType;
    use al_data::FieldDef;

    fn table() -> TableDef {
        let f = |id, name: &str, ty| FieldDef {
            id,
            name: name.into(),
            ty,
            stored: true,
        };
        TableDef {
            object: 0,
            name: "T".into(),
            fields: vec![f(1, "ID", RtType::Integer), f(2, "Name", RtType::Text(None))],
            keys: vec![vec![0]],
            per_company: false,
        }
    }

    fn row(id: i32, name: &str) -> Row {
        vec![Value::Integer(id), Value::Text(name.into())]
    }

    fn mat() -> Mat {
        Mat::new(
            &table(),
            vec![
                Entry::ok(row(3, "c")),
                Entry::ok(row(1, "a")),
                Entry::poisoned(row(2, ""), "row 2 is not understood".into()),
            ],
        )
    }

    fn scan<'a>(filters: &'a [FieldFilter], after: Option<&'a [Value]>, ascending: bool) -> Scan<'a> {
        Scan {
            key: 0,
            filters,
            ascending,
            after,
        }
    }

    fn id_filter(lo: i32, hi: i32) -> FieldFilter {
        FieldFilter {
            field: 0,
            expr: FilterExpr::Range(Some(Value::Integer(lo)), Some(Value::Integer(hi))),
            group: 0,
        }
    }

    #[test]
    fn rows_come_back_in_key_order_and_a_poisoned_row_refuses_only_a_read_that_could_reach_it() {
        let (t, m) = (table(), mat());
        // The scan from the start reaches row 1, then would pass row 2.
        assert!(matches!(m.next(&t, &scan(&[], None, true)), Ok(Some(r)) if matches!(r[0], Value::Integer(1))));
        let after_1 = [Value::Integer(1), Value::Integer(1)];
        assert!(
            m.next(&t, &scan(&[], Some(&after_1), true)).is_err(),
            "row 2 might be the next row"
        );
        // A key filter that excludes row 2 is answered.
        let f = [id_filter(3, 9)];
        assert!(matches!(m.next(&t, &scan(&f, None, true)), Ok(Some(r)) if matches!(r[0], Value::Integer(3))));
        assert!(matches!(m.count(&t, &f), Ok(1)));
        // A keyed read of the poisoned row refuses; of a missing row answers none.
        assert!(m.get(&t, &[Value::Integer(2)]).is_err());
        assert!(matches!(m.get(&t, &[Value::Integer(9)]), Ok(None)));
        assert!(matches!(m.get(&t, &[Value::Integer(3)]), Ok(Some(_))));
        assert!(
            m.count(&t, &[]).is_err(),
            "a count over everything includes the poisoned row"
        );
    }

    #[test]
    fn a_descending_scan_starts_at_the_last_row() {
        let (t, m) = (table(), mat());
        let f = [id_filter(3, 9)];
        assert!(matches!(m.next(&t, &scan(&f, None, false)), Ok(Some(r)) if matches!(r[0], Value::Integer(3))));
    }
}
