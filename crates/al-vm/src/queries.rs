//! Query objects: `Open`, `Read`, `Close`, filters on columns, joins, grouping and aggregation.
//!
//! A query is executed as the SQL statement BC compiles it to: a nested-loop join over the
//! dataitems (every dataitem scanned through the store, ADR-003), the filters applied as WHERE
//! conditions, grouping by the non-aggregated columns when an aggregated column is present, the
//! filters on aggregated columns applied to the groups (HAVING), then the sort and the row cap.
//! Every behaviour is pinned by a corpus test cited at the code or is refused with
//! `Signal::Unsupported`; the open questions are in docs/probes-needed.md.

use crate::Session;
use al_builtins::filter::{FieldFilter, FilterExpr};
use al_builtins::native::{AlError, Signal, text_of};
use al_builtins::ops;
use al_builtins::value::{QueryRow, QueryRun, QueryState, RtType, Value};
use al_data::TableDef;
use al_ir as ir;
use rust_decimal::Decimal;
use std::cell::RefCell;
use std::cmp::Ordering;
use std::rc::Rc;

type Qs = Rc<RefCell<QueryState>>;
/// One row of each dataitem's table, in dataitem order.
type JoinedRow = Vec<Vec<Value>>;
/// (table object, version) of each table a query reads.
type Stamps = Vec<(u32, u64)>;

fn unsupported<T>(what: impl Into<String>) -> Result<T, Signal> {
    Err(Signal::Unsupported(what.into()))
}

fn arg_col(args: &[Value], i: usize) -> Result<usize, Signal> {
    match args.get(i) {
        Some(Value::Integer(p)) => Ok(*p as usize),
        other => unsupported(format!("column argument {other:?}")),
    }
}

/// Compares two sort keys: the first `dirs.len()` entries in the given directions, the rest
/// ascending.
fn key_cmp(a: &[Value], b: &[Value], dirs: &[bool]) -> Ordering {
    for (i, (x, y)) in a.iter().zip(b).enumerate() {
        let o = ops::compare(x, y).unwrap_or(Ordering::Equal);
        let o = if dirs.get(i).copied().unwrap_or(true) {
            o
        } else {
            o.reverse()
        };
        if o != Ordering::Equal {
            return o;
        }
    }
    Ordering::Equal
}

fn negate(v: Value) -> Result<Value, Signal> {
    Ok(match v {
        Value::Decimal(d) => Value::Decimal(-d),
        Value::Integer(i) => Value::Integer(-i),
        Value::BigInteger(i) => Value::BigInteger(-i),
        other => return unsupported(format!("ReverseSign on {other:?}")),
    })
}

/// The filters of one execution, split by where they apply.
struct Plan {
    /// WHERE filters on stored fields, per dataitem.
    scan: Vec<Vec<FieldFilter>>,
    /// Flow filters (`filter()` elements over FlowFilter fields), per dataitem.
    flow: Vec<Vec<FieldFilter>>,
    /// Filters on non-aggregated FlowField columns, checked on the projected value.
    post: Vec<(usize, FilterExpr)>,
    /// Filters on aggregated columns, checked on the group.
    having: Vec<(usize, FilterExpr)>,
}

impl Session {
    pub(crate) fn query_op(
        &mut self,
        method: &str,
        recv: Option<&mut Value>,
        args: &mut [Value],
    ) -> Result<Value, Signal> {
        let qs: Qs = match recv {
            Some(Value::Query(q)) => q.clone(),
            other => return unsupported(format!("Query.{method} on {other:?}")),
        };
        let engine = self.engine.clone();
        let object = qs.borrow().object as usize;
        let def = engine.program.objects[object].query.as_ref().expect("a query object");
        match method {
            "Open" => {
                if qs.borrow().run.is_some() {
                    return unsupported("Open on a query that is already open (not pinned)");
                }
                let filters = qs.borrow().filters.clone();
                let (rows, stamps) = self.query_rows(def, &filters)?;
                qs.borrow_mut().run = Some(QueryRun {
                    rows,
                    next: 0,
                    current: None,
                    last_key: None,
                    returned: 0,
                    stamps,
                });
                Ok(Value::Boolean(true))
            }
            "Read" => self.query_read(def, &qs),
            "Close" => {
                qs.borrow_mut().run = None;
                Ok(Value::Null)
            }
            "SetRange" | "SetFilter" => {
                let col = arg_col(args, 0)?;
                if qs.borrow().run.is_some() {
                    return unsupported(format!("{method} on an open query (not pinned)"));
                }
                let ty = def.columns[col].ty.clone();
                let expr = if method == "SetRange" {
                    match args.get(1) {
                        // `SetRange(Column)` removes the filter.
                        None => None,
                        Some(from) => {
                            let from = ops::convert(from.clone(), &ty)?;
                            let to = match args.get(2) {
                                Some(v) => ops::convert(v.clone(), &ty)?,
                                None => from.clone(),
                            };
                            Some(FilterExpr::Range(Some(from), Some(to)))
                        }
                    }
                } else {
                    let text = text_of(args.get(1).unwrap_or(&Value::Null)).unwrap_or_default();
                    let engine = self.engine.clone();
                    let enums = |id: u32| engine.program.objects[id as usize].enum_values.clone();
                    Some(
                        al_builtins::filter::parse_filter_with(&text, &args[2..], &ty, &enums)
                            .map_err(|e| Signal::Unsupported(format!("SetFilter: {e}")))?,
                    )
                };
                let mut q = qs.borrow_mut();
                q.filters.retain(|(c, _)| *c != col);
                if let Some(e) = expr {
                    q.filters.push((col, e));
                }
                Ok(Value::Null)
            }
            "GetFilter" => {
                let col = arg_col(args, 0)?;
                let q = qs.borrow();
                match q.filters.iter().find(|(c, _)| *c == col) {
                    Some((_, e)) => self.filter_text(e).map(Value::Text),
                    None if def.columns[col].static_filter.is_some() => {
                        unsupported("GetFilter of a column with a static ColumnFilter (not pinned)")
                    }
                    None => Ok(Value::Text(String::new())),
                }
            }
            "GetFilters" => {
                // `Caption: filter` pairs joined by `, `, as Record.GetFilters (docs/probes-needed.md).
                let mut parts = Vec::new();
                let q = qs.borrow();
                let mut sorted: Vec<&(usize, FilterExpr)> = q.filters.iter().collect();
                sorted.sort_by_key(|(c, _)| *c);
                for (c, e) in sorted {
                    let Some(caption) = &def.columns[*c].caption else {
                        return unsupported("GetFilters of a column without a Caption literal (not pinned)");
                    };
                    parts.push(format!("{caption}: {}", self.filter_text(e)?));
                }
                Ok(Value::Text(parts.join(", ")))
            }
            "ColumnName" => Ok(Value::Text(def.columns[arg_col(args, 0)?].name.clone())),
            "ColumnCaption" => match &def.columns[arg_col(args, 0)?].caption {
                Some(c) => Ok(Value::Text(c.clone())),
                None => unsupported("ColumnCaption of a column without a Caption literal (the fallback is not pinned)"),
            },
            // Only "non-zero and different per column" is pinned (corpus
            // query/TestQueryObject.al:347-361): the position in the query is used.
            "ColumnNo" => Ok(Value::Integer(arg_col(args, 0)? as i32 + 1)),
            "TopNumberOfRows" => match args.first() {
                None => Ok(Value::Integer(qs.borrow().top.unwrap_or(def.top))),
                Some(n) => {
                    let n = match n {
                        Value::Integer(n) => *n,
                        other => return unsupported(format!("TopNumberOfRows({other:?})")),
                    };
                    if n < 0 {
                        // Corpus query/TestQueryTopProperty.al:178.
                        return Err(Signal::Error(AlError {
                            message: "Top number of rows cannot be negative".into(),
                            code: String::new(),
                            call_stack: String::new(),
                        }));
                    }
                    if qs.borrow().run.is_some() {
                        return unsupported("TopNumberOfRows on an open query (not pinned)");
                    }
                    qs.borrow_mut().top = Some(n);
                    Ok(Value::Null)
                }
            },
            "SecurityFiltering" => {
                let members = al_builtins::sysenum::option_members("SecurityFilter").expect("known");
                match args.first() {
                    // Corpus query/TestQueryObject.al:382-390.
                    Some(Value::Option(i, _)) => {
                        if members[*i as usize].eq_ignore_ascii_case("Validated") {
                            return unsupported(
                                "SecurityFiltering(Validated) on a query raises an error whose text is not pinned",
                            );
                        }
                        qs.borrow_mut().security_filtering = Some(*i);
                        Ok(Value::Null)
                    }
                    None => match qs.borrow().security_filtering {
                        Some(i) => Ok(Value::Option(i, members.into())),
                        None => unsupported("SecurityFiltering() before it is set (the initial value is not pinned)"),
                    },
                    Some(other) => unsupported(format!("SecurityFiltering({other:?})")),
                }
            }
            other => unsupported(format!("Query.{other}")),
        }
    }

    /// The value of column `column` in the row `Read` returned last.
    pub(crate) fn query_column(&mut self, q: Value, column: usize) -> Result<Value, Signal> {
        let Value::Query(qs) = q else {
            return unsupported(format!("a query column of {q:?}"));
        };
        let qs = qs.borrow();
        match qs.run.as_ref().and_then(|r| r.current.as_ref()) {
            Some(row) => Ok(row[column].clone()),
            None => unsupported("reading a query column without a current row (not pinned)"),
        }
    }

    fn query_read(&mut self, def: &ir::Query, qs: &Qs) -> Result<Value, Signal> {
        if qs.borrow().run.is_none() {
            return unsupported("Read on a query that is not open (not pinned)");
        }
        // A write to a table the query reads, made between two Reads, invalidates the result set;
        // Read then reads again and resumes after the last row it returned, in the query's order
        // (corpus query/TestQueryReadAfterWrite.al:102-179).
        let now = self.query_stamps(def);
        if qs.borrow().run.as_ref().expect("open").stamps != now {
            if def.columns.iter().any(|c| c.method.is_some()) {
                return unsupported("a write to a table an open aggregated query reads (the re-read is not pinned)");
            }
            let filters = qs.borrow().filters.clone();
            let (mut rows, stamps) = self.query_rows(def, &filters)?;
            let dirs: Vec<bool> = def.order_by.iter().map(|(_, asc)| *asc).collect();
            let mut q = qs.borrow_mut();
            let run = q.run.as_mut().expect("open");
            if let Some(last) = &run.last_key {
                rows.retain(|r| key_cmp(&r.key, last, &dirs) == Ordering::Greater);
            }
            run.rows = rows;
            run.next = 0;
            run.stamps = stamps;
        }
        let mut q = qs.borrow_mut();
        let top = q.top.unwrap_or(def.top);
        let run = q.run.as_mut().expect("open");
        // The cap counts the rows returned since Open, so a re-read returns only the rows still owed
        // (corpus query/TestQueryOrderAfterWrite.al:127-149, TestQueryTopProperty.al:94-108).
        if (top > 0 && run.returned >= top as usize) || run.next >= run.rows.len() {
            run.current = None;
            return Ok(Value::Boolean(false));
        }
        let row = run.rows[run.next].clone();
        run.next += 1;
        run.returned += 1;
        run.current = Some(row.columns);
        run.last_key = Some(row.key);
        Ok(Value::Boolean(true))
    }

    /// The version of every table the query reads.
    fn query_stamps(&self, def: &ir::Query) -> Stamps {
        let mut out = Stamps::new();
        for item in &def.items {
            let t = self.engine.tables[item.table].as_ref().expect("table");
            if !out.iter().any(|(o, _)| *o == t.object) {
                out.push((t.object, self.store.table_stamp(&self.cx, t)));
            }
        }
        out
    }

    /// Runs the query: its rows in order, and the table versions they were read at.
    fn query_rows(
        &mut self,
        def: &ir::Query,
        runtime: &[(usize, FilterExpr)],
    ) -> Result<(Vec<QueryRow>, Stamps), Signal> {
        let stamps = self.query_stamps(def);
        let plan = self.query_plan(def, runtime)?;
        // The joined rows: one row of each dataitem's table.
        let mut joined: Vec<JoinedRow> = Vec::new();
        self.expand(def, &plan, 0, &mut Vec::new(), &mut joined)?;
        let aggregated = def.columns.iter().any(|c| c.method.is_some());
        let dirs: Vec<bool> = def.order_by.iter().map(|(_, asc)| *asc).collect();
        let mut rows = if aggregated {
            self.group_rows(def, &plan, joined)?
        } else {
            let mut rows = Vec::new();
            for j in joined {
                let columns = self.project(def, &plan, &j)?;
                if plan.post.iter().all(|(c, e)| e.matches(&columns[*c])) {
                    // Order by the OrderBy columns, then the primary key of each dataitem.
                    let mut key: Vec<Value> = def.order_by.iter().map(|(c, _)| columns[*c].clone()).collect();
                    for (item, row) in def.items.iter().zip(&j) {
                        let t = self.engine.tables[item.table].as_ref().expect("table");
                        key.extend(t.primary_key(row));
                    }
                    rows.push(QueryRow { columns, key });
                }
            }
            rows
        };
        rows.sort_by(|a, b| key_cmp(&a.key, &b.key, &dirs));
        // Rows that tie on every OrderBy column and still differ are ordered by the primary key
        // ascending (corpus query/TestQueryOrderAfterWrite.al:92-104, ascending only), or in no
        // pinned way: grouped rows, and ties under a descending OrderBy.
        let k = dirs.len();
        if aggregated || dirs.iter().any(|a| !a) {
            for w in rows.windows(2) {
                if key_cmp(&w[0].key[..k], &w[1].key[..k], &dirs) == Ordering::Equal {
                    return unsupported(
                        "rows that tie on every OrderBy column under a descending OrderBy or in a grouped query (their order is not pinned)",
                    );
                }
            }
        }
        Ok((rows, stamps))
    }

    /// Sorts every filter of the query to the place it applies.
    fn query_plan(&mut self, def: &ir::Query, runtime: &[(usize, FilterExpr)]) -> Result<Plan, Signal> {
        let engine = self.engine.clone();
        let mut plan = Plan {
            scan: vec![Vec::new(); def.items.len()],
            flow: vec![Vec::new(); def.items.len()],
            post: Vec::new(),
            having: Vec::new(),
        };
        for (i, item) in def.items.iter().enumerate() {
            let fields = &engine.program.objects[item.table].table.as_ref().expect("table").fields;
            for c in &item.table_filter {
                let ty = &fields[c.field].ty;
                let expr = match &c.value {
                    ir::CalcValue::Const(k) => FilterExpr::Eq(ops::convert(crate::constant(k), ty)?),
                    ir::CalcValue::Filter(text) => self.parse_query_filter(text, ty)?,
                    _ => return unsupported("a table filter condition of this form"),
                };
                plan.scan[i].push(FieldFilter {
                    field: c.field,
                    expr,
                    group: 0,
                });
            }
        }
        for (ci, c) in def.columns.iter().enumerate() {
            // A runtime filter on a column replaces the column's static ColumnFilter (corpus
            // query/TestQueryColumnFilter.al:62-100); a table filter combines with either
            // (query/TestQueryDataItemTableFilter.al:137-229).
            let expr = match runtime.iter().find(|(r, _)| *r == ci) {
                Some((_, e)) => Some(e.clone()),
                None => match &c.static_filter {
                    Some(ir::CalcValue::Const(k)) => Some(FilterExpr::Eq(ops::convert(crate::constant(k), &c.ty)?)),
                    Some(ir::CalcValue::Filter(text)) => Some(self.parse_query_filter(text, &c.ty)?),
                    Some(_) => return unsupported("a column filter of this form"),
                    None => None,
                },
            };
            let Some(expr) = expr else { continue };
            let item = &def.items[c.item];
            if item.left_outer && item.parent.is_some() {
                return unsupported("a runtime filter on an outer-joined dataitem (not pinned)");
            }
            if c.reverse_sign {
                return unsupported(
                    "a filter on a ReverseSign column (whether it applies before the sign flips is not pinned)",
                );
            }
            if c.method.is_some() {
                plan.having.push((ci, expr));
                continue;
            }
            let field = c.field.expect("a column over a field");
            let schema = &engine.program.objects[item.table].table.as_ref().expect("table").fields[field];
            if c.filter_element && schema.flow_filter {
                // Corpus query/TestQueryFlowFieldColumn.al:150-200: it sets the flow filter of the
                // FlowFields of the dataitem; it never constrains the rows.
                plan.flow[c.item].push(FieldFilter { field, expr, group: 0 });
            } else if schema.stored {
                plan.scan[c.item].push(FieldFilter { field, expr, group: 0 });
            } else {
                plan.post.push((ci, expr));
            }
        }
        Ok(plan)
    }

    fn parse_query_filter(&self, text: &str, ty: &RtType) -> Result<FilterExpr, Signal> {
        self.parse_filter_text(text, ty)
            .map_err(|e| Signal::Unsupported(format!("query filter: {e}")))
    }

    /// Nested-loop join: every row of dataitem `level` that passes its filters and links to the
    /// rows chosen above it, then the next dataitem.
    fn expand(
        &mut self,
        def: &ir::Query,
        plan: &Plan,
        level: usize,
        chosen: &mut Vec<Vec<Value>>,
        out: &mut Vec<JoinedRow>,
    ) -> Result<(), Signal> {
        if level == def.items.len() {
            out.push(chosen.clone());
            return Ok(());
        }
        let item = &def.items[level];
        let t: TableDef = self.engine.tables[item.table].clone().expect("table");
        let mut filters = plan.scan[level].clone();
        if let Some(parent) = item.parent {
            // Corpus query/TestQueryDataItemLinkMultiField.al: every equality of the link holds.
            for (own, theirs) in &item.link {
                let v = ops::convert(chosen[parent][*theirs].clone(), &t.fields[*own].ty)?;
                filters.push(FieldFilter {
                    field: *own,
                    expr: FilterExpr::Eq(v),
                    group: 0,
                });
            }
        }
        let mut rows: Vec<Vec<Value>> = Vec::new();
        self.each_row(&t, &filters, |row| {
            rows.push(row.to_vec());
            Ok(())
        })?;
        if rows.is_empty() && item.left_outer && item.parent.is_some() {
            // Corpus query/TestQueryJoin.al:117-145: the unmatched parent is kept, the child's
            // columns hold their defaults.
            let row = t.fields.iter().map(|f| self.default_value(&f.ty)).collect();
            rows.push(row);
        }
        for row in rows {
            chosen.push(row);
            self.expand(def, plan, level + 1, chosen, out)?;
            chosen.pop();
        }
        Ok(())
    }

    /// The column values of one joined row. Aggregated columns and filter elements are `Null` here.
    fn project(&mut self, def: &ir::Query, plan: &Plan, joined: &[Vec<Value>]) -> Result<Vec<Value>, Signal> {
        let engine = self.engine.clone();
        let mut out = Vec::with_capacity(def.columns.len());
        for c in &def.columns {
            if c.filter_element || c.method.is_some() {
                out.push(Value::Null);
                continue;
            }
            let item = &def.items[c.item];
            let field = c.field.expect("a column over a field");
            let row = &joined[c.item];
            let schema = &engine.program.objects[item.table].table.as_ref().expect("table").fields[field];
            let mut v = if schema.stored {
                row[field].clone()
            } else {
                // A FlowField is calculated per joined row, with the flow filters of its dataitem
                // (corpus query/TestQueryFlowFieldColumn.al:37-200, query/TestQueryJoinFlowFieldGroupBy.al).
                self.check_calc_field(item.table, field)?;
                let probe = self.probe(item.table, row, &plan.flow[c.item]);
                self.calc_value(&probe, field, &mut Vec::new())?
            };
            if c.reverse_sign {
                // Corpus query/TestQueryReverseSign.al:36-77.
                v = negate(v)?;
            }
            out.push(v);
        }
        Ok(out)
    }

    fn probe(
        &mut self,
        table: usize,
        row: &[Value],
        flow: &[FieldFilter],
    ) -> Rc<RefCell<al_builtins::value::RecordState>> {
        let Value::Record(rc) = self.default_value(&RtType::Record(table as u32, false)) else {
            unreachable!("a record default");
        };
        {
            let mut r = rc.borrow_mut();
            r.fields = row.to_vec();
            r.filters = flow.to_vec();
            r.temporary = false;
            r.temp_id = None;
        }
        rc
    }

    /// Groups the joined rows by the non-aggregated columns and aggregates the rest.
    fn group_rows(&mut self, def: &ir::Query, plan: &Plan, joined: Vec<JoinedRow>) -> Result<Vec<QueryRow>, Signal> {
        use ir::QueryMethod as M;
        let group_cols: Vec<usize> = def
            .columns
            .iter()
            .enumerate()
            .filter(|(_, c)| c.method.is_none() && !c.filter_element)
            .map(|(i, _)| i)
            .collect();
        // (group key, the projected rows and their source rows)
        let mut keyed: Vec<(Vec<Value>, JoinedRow)> = Vec::new();
        for j in joined {
            let columns = self.project(def, plan, &j)?;
            if !plan.post.iter().all(|(c, e)| e.matches(&columns[*c])) {
                continue;
            }
            let key = group_cols.iter().map(|c| columns[*c].clone()).collect();
            keyed.push((key, j));
        }
        // Rows with the same group columns form one group (SQL's GROUP BY).
        keyed.sort_by(|a, b| key_cmp(&a.0, &b.0, &[]));
        let mut groups: Vec<(Vec<Value>, Vec<JoinedRow>)> = Vec::new();
        for (key, j) in keyed {
            match groups.last_mut() {
                Some((k, rows)) if key_cmp(k, &key, &[]) == Ordering::Equal => rows.push(j),
                _ => groups.push((key, vec![j])),
            }
        }
        // Corpus query/TestQueryAggregation.al:102-160: without a grouping column the whole table
        // is one group, even when it is empty.
        if groups.is_empty() && group_cols.is_empty() {
            groups.push((Vec::new(), Vec::new()));
        }
        let mut out = Vec::new();
        for (key, rows) in groups {
            let mut columns = vec![Value::Null; def.columns.len()];
            for (c, v) in group_cols.iter().zip(&key) {
                columns[*c] = v.clone();
            }
            for (ci, c) in def.columns.iter().enumerate() {
                let Some(method) = c.method else { continue };
                let values = || -> Vec<Value> {
                    let field = c.field.expect("a column over a field");
                    rows.iter().map(|j| j[c.item][field].clone()).collect()
                };
                let mut v = match method {
                    M::Count => Value::Integer(rows.len() as i32),
                    M::Sum | M::Average => {
                        let vs = values();
                        let mut sum = Decimal::ZERO;
                        for x in &vs {
                            match x {
                                Value::Decimal(d) => sum += d,
                                other => return unsupported(format!("Sum over {other:?}")),
                            }
                        }
                        if method == M::Sum {
                            Value::Decimal(sum)
                        } else if vs.is_empty() {
                            return unsupported("Average over no rows");
                        } else {
                            // Corpus query/TestQueryAggregation.al:36-80 (150, 50). A quotient that
                            // does not terminate is refused: BC's decimal precision is not pinned.
                            let n = Decimal::from(vs.len() as i64);
                            let avg = sum / n;
                            if avg * n != sum {
                                return unsupported("an Average that does not divide exactly");
                            }
                            Value::Decimal(avg)
                        }
                    }
                    M::Min | M::Max => {
                        let vs = values();
                        let mut best: Option<Value> = None;
                        for x in vs {
                            best = Some(match best {
                                None => x,
                                Some(b) => {
                                    let o = ops::compare(&x, &b)?;
                                    if (method == M::Min && o == Ordering::Less)
                                        || (method == M::Max && o == Ordering::Greater)
                                    {
                                        x
                                    } else {
                                        b
                                    }
                                }
                            });
                        }
                        match best {
                            Some(b) => b,
                            None => return unsupported("Min or Max over no rows (not pinned)"),
                        }
                    }
                };
                if c.reverse_sign {
                    // Corpus query/TestQueryReverseSign.al:79-125: the group total is negated.
                    v = negate(v)?;
                }
                columns[ci] = v;
            }
            // HAVING: the filters on aggregated columns see the group's result (corpus
            // query/TestQueryAggregation.al:182-221, TestQueryColumnFilter.al:36-100).
            if !plan.having.iter().all(|(c, e)| e.matches(&columns[*c])) {
                continue;
            }
            // Order by the OrderBy columns, then the remaining grouping columns.
            let mut sort: Vec<Value> = def.order_by.iter().map(|(c, _)| columns[*c].clone()).collect();
            sort.extend(key);
            out.push(QueryRow { columns, key: sort });
        }
        Ok(out)
    }
}
