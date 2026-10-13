//! FlowFields (`CalcFormula`), FlowFilters, `CalcFields`, `CalcSums` and
//! `SetAutoCalcFields`.
//!
//! Every behaviour here is pinned by a corpus test (cited at the code) or is
//! refused with `Signal::Unsupported`; the open questions are listed in
//! docs/probes-needed.md. Aggregates are computed over the store's scan API
//! (ADR-003): a FlowField reads the persistent table, one row at a time.

use crate::Session;
use al_builtins::filter::{FieldFilter, FilterExpr, parse_filter_with};
use al_builtins::native::{AlError, Signal, text_of};
use al_builtins::ops;
use al_builtins::value::{RecordState, RtType, Value};
use al_data::{Scan, Store, TableDef, position};
use al_ir as ir;
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;
use std::cell::RefCell;
use std::cmp::Ordering;
use std::rc::Rc;

type Rec = Rc<RefCell<RecordState>>;

fn unsupported<T>(what: impl Into<String>) -> Result<T, Signal> {
    Err(Signal::Unsupported(what.into()))
}

/// A runtime error with a message the corpus pins (error codes are not pinned).
fn error<T>(message: String) -> Result<T, Signal> {
    Err(Signal::Error(AlError {
        message,
        code: String::new(),
        call_stack: String::new(),
    }))
}

/// How BC names a field type in the "must have the same type" message.
fn type_name(t: &RtType) -> String {
    match t {
        RtType::Boolean => "Boolean".into(),
        RtType::Integer => "Integer".into(),
        RtType::BigInteger => "BigInteger".into(),
        RtType::Decimal => "Decimal".into(),
        RtType::Duration => "Duration".into(),
        RtType::Date => "Date".into(),
        RtType::Time => "Time".into(),
        RtType::DateTime => "DateTime".into(),
        RtType::Guid => "Guid".into(),
        RtType::Text(_) => "Text".into(),
        RtType::Code(_) => "Code".into(),
        RtType::Option(_) => "Option".into(),
        RtType::Enum(_) => "Enum".into(),
        other => format!("{other:?}"),
    }
}

/// Types that compare against each other when a filter written for one field
/// is applied to another: text with code, otherwise the same type.
fn same_kind(a: &RtType, b: &RtType) -> bool {
    use RtType as T;
    match (a, b) {
        (T::Text(_) | T::Code(_), T::Text(_) | T::Code(_)) => true,
        (T::Option(_), T::Option(_)) => true,
        (T::Enum(x), T::Enum(y)) => x == y,
        _ => std::mem::discriminant(a) == std::mem::discriminant(b),
    }
}

fn number(v: &Value) -> Option<Decimal> {
    Some(match v {
        Value::Integer(i) => Decimal::from(*i),
        Value::BigInteger(i) => Decimal::from(*i),
        Value::Decimal(d) => *d,
        Value::Duration(ms) => Decimal::from(*ms),
        _ => return None,
    })
}

/// A decimal result as a value of `ty`; whole numbers only for integral
/// types. `drop_small_fraction` (averages): a fraction below one half is dropped.
fn to_number(d: Decimal, ty: &RtType, drop_small_fraction: bool) -> Result<Value, Signal> {
    let d = if drop_small_fraction && !matches!(ty, RtType::Decimal) && d.fract() != Decimal::ZERO {
        if d.fract().abs() >= Decimal::new(5, 1) {
            return unsupported(format!(
                "average {d} into {} (truncation and rounding differ; not pinned by the corpus)",
                type_name(ty)
            ));
        }
        d.trunc()
    } else {
        d
    };
    match ty {
        RtType::Duration => match d.to_i64() {
            Some(ms) if d.fract() == Decimal::ZERO => Ok(Value::Duration(ms)),
            _ => unsupported(format!("{d} as a Duration")),
        },
        _ => ops::convert(Value::Decimal(d), ty),
    }
}

impl Session {
    /// Why a field cannot be handed to `CalcFields`, or `Ok` when it is a
    /// FlowField with a formula this runtime can calculate.
    fn check_calc_field(&mut self, table: usize, pos: usize) -> Result<(), Signal> {
        let engine = self.engine.clone();
        let schema = engine.program.objects[table].table.as_ref().expect("table");
        let f = &schema.fields[pos];
        let tcap = self.table_caption(table);
        // Corpus record/TestCalcFieldsFieldClass.Codeunit.al:76,92,108: only a FlowField
        // (or a Blob) is accepted, and the refusal names the field and its table.
        if f.stored || f.flow_filter {
            if let (true, RtType::Library(n)) = (f.stored, &f.ty) {
                return unsupported(format!(
                    "CalcFields on a {n} field (streams and media are not implemented yet)"
                ));
            }
            return error(format!("The {} field in the {tcap} table must be a FlowField.", f.name));
        }
        if let Some(reason) = &f.calc_refusal {
            return unsupported(reason.clone());
        }
        // Corpus record/TestCalcFieldsFieldClass.Codeunit.al:124.
        let Some(calc) = &f.calc else {
            return error(format!(
                "You must define a CalcFormula for the {} FlowField in the {tcap} table.",
                f.name
            ));
        };
        let src = &engine.program.objects[calc.table].table.as_ref().expect("table").fields;
        match calc.method {
            ir::CalcMethod::Sum | ir::CalcMethod::Average => {
                let s = &src[calc.field.expect("sum and average have a field")];
                if !matches!(
                    s.ty,
                    RtType::Decimal | RtType::Integer | RtType::BigInteger | RtType::Duration
                ) {
                    return unsupported(format!("sum or average over a {} field", type_name(&s.ty)));
                }
                // Corpus record/TestCalcFormulaValidationTests.Codeunit.al:96-130: the FlowField
                // and its source must have the very same type, refused when calculated. (BC 29
                // relaxes Integer/BigInteger; the oracle is BC 27.5.)
                if std::mem::discriminant(&f.ty) != std::mem::discriminant(&s.ty) {
                    let stcap = self.table_caption(calc.table);
                    return error(format!(
                        "The following fields must have the same type:\n\nField: {} <-- {}\nTable: {tcap} <-- {stcap}\nType: {} <-- {}",
                        f.name,
                        s.name,
                        type_name(&f.ty),
                        type_name(&s.ty)
                    ));
                }
            }
            ir::CalcMethod::Min | ir::CalcMethod::Max | ir::CalcMethod::Lookup => {
                let s = &src[calc.field.expect("has a field")];
                if !same_kind(&f.ty, &s.ty) {
                    return unsupported("min, max or lookup into a field of another type");
                }
            }
            ir::CalcMethod::Count => {
                if !matches!(f.ty, RtType::Integer) {
                    return unsupported("count into a field that is not an Integer");
                }
            }
            ir::CalcMethod::Exist => {
                if !matches!(f.ty, RtType::Boolean) {
                    return unsupported("exist into a field that is not a Boolean");
                }
            }
        }
        Ok(())
    }

    fn table_caption(&self, table: usize) -> String {
        let t = self.engine.tables[table].clone().expect("table");
        self.caption(&t)
    }

    /// `CalcFields`: every named field is validated before any is calculated,
    /// and a refused call changes nothing (corpus
    /// record/TestCalcFormulaValidationTests.Codeunit.al:211,
    /// record/TestCalcFieldsFieldClass.Codeunit.al:204).
    pub(crate) fn calc_fields(&mut self, rec: &Rec, positions: &[usize]) -> Result<(), Signal> {
        let table = rec.borrow().table as usize;
        for &p in positions {
            self.check_calc_field(table, p)?;
        }
        let mut out = Vec::new();
        for &p in positions {
            out.push((p, self.calc_value(rec, p, &mut Vec::new())?));
        }
        if rec.borrow().temporary {
            // Whether a FlowField on a temporary record reads the persistent source
            // table or stays uncalculated is not pinned (the corpus only has a source
            // without rows, record/TestFlowFieldContracts.al:152): refuse unless both
            // readings agree.
            for (p, v) in &out {
                let same = matches!(ops::compare(v, &rec.borrow().fields[*p]), Ok(Ordering::Equal));
                if !same {
                    return unsupported("a FlowField calculated on a temporary record (not pinned yet)");
                }
            }
            return Ok(());
        }
        let mut r = rec.borrow_mut();
        for (p, v) in out {
            r.fields[p] = v;
        }
        Ok(())
    }

    /// Calculates the FlowField at `pos` of `rec` from the table as it is now.
    fn calc_value(&mut self, rec: &Rec, pos: usize, stack: &mut Vec<usize>) -> Result<Value, Signal> {
        // Corpus record/TestCalcFormulaFlowFieldValueTests.Codeunit.al:176-221: a formula
        // that needs itself, directly or through another FlowField, is a recursion error.
        if stack.contains(&pos) {
            return error(
                "A FlowField formula depends on its own value. This can be caused by recursive function calls.".into(),
            );
        }
        stack.push(pos);
        let r = self.calc_value_inner(rec, pos, stack);
        stack.pop();
        r
    }

    fn calc_value_inner(&mut self, rec: &Rec, pos: usize, stack: &mut Vec<usize>) -> Result<Value, Signal> {
        let engine = self.engine.clone();
        let table = rec.borrow().table as usize;
        let f = &engine.program.objects[table].table.as_ref().expect("table").fields[pos];
        let calc = f.calc.as_ref().expect("checked by check_calc_field");
        let filters = self.calc_filters(rec, calc, stack)?;
        let src_t = self.engine.tables[calc.table].clone().expect("table");
        let cx = self.cx.clone();
        let store_err = |e: al_data::StoreError| Signal::Unsupported(format!("store error {e:?}"));
        use ir::CalcMethod as M;
        let value = match calc.method {
            M::Count => {
                let n = self.store.count(&cx, &src_t, &filters).map_err(store_err)? as i32;
                Value::Integer(if calc.negate { -n } else { n })
            }
            M::Exist => {
                let found = self
                    .store
                    .next(
                        &cx,
                        &src_t,
                        &Scan {
                            key: 0,
                            filters: &filters,
                            ascending: true,
                            after: None,
                        },
                    )
                    .map_err(store_err)?
                    .is_some();
                // A leading '-' on an exist is a logical NOT (corpus
                // record/TestCalcFormulaSignFiltersTests.Codeunit.al:196).
                Value::Boolean(found != calc.negate)
            }
            M::Sum | M::Average => {
                let at = calc.field.expect("field");
                let (mut sum, mut n) = (Decimal::ZERO, 0i64);
                self.each_row(&src_t, &filters, |row| {
                    sum += number(&row[at]).ok_or_else(|| Signal::Unsupported("sum over a non-number".into()))?;
                    n += 1;
                    Ok(())
                })?;
                let mut d = if calc.method == M::Sum {
                    sum
                } else if n == 0 {
                    Decimal::ZERO
                } else {
                    sum / Decimal::from(n)
                };
                if calc.negate {
                    d = -d;
                }
                // Corpus record/TestCalcFormulaMinMaxAvgTests.Codeunit.al:115: an Integer
                // average of 5.25 or 0.25 is stored as 5 and 0; only fractions below one
                // half are pinned (truncation and rounding agree there).
                to_number(d, &f.ty, calc.method == M::Average)?
            }
            M::Min | M::Max | M::Lookup => {
                let at = calc.field.expect("field");
                if calc.negate {
                    return unsupported("a sign on min, max or lookup");
                }
                let mut best: Option<Value> = None;
                self.each_row(&src_t, &filters, |row| {
                    let v = &row[at];
                    best = match best.take() {
                        None => Some(v.clone()),
                        Some(b) => match (calc.method, ops::compare(v, &b)?) {
                            (M::Min, Ordering::Less) | (M::Max, Ordering::Greater) => Some(v.clone()),
                            (M::Lookup, o) if o != Ordering::Equal => {
                                // Which of several different rows lookup() reads depends on the
                                // key BC picks; not pinned (every corpus lookup has one match or
                                // matches that agree).
                                return unsupported("lookup over matching rows that disagree");
                            }
                            (_, _) => Some(b),
                        },
                    };
                    Ok(())
                })?;
                // Nothing matched: the type default (corpus
                // record/TestCalcFormulaMinMaxAvgTests.Codeunit.al:250).
                match best {
                    Some(v) => v,
                    None => self.default_value(&f.ty),
                }
            }
        };
        ops::convert(value, &f.ty)
    }

    /// The source-table filters of a formula's where-conditions for `rec`.
    fn calc_filters(
        &mut self,
        rec: &Rec,
        calc: &ir::CalcFormula,
        stack: &mut Vec<usize>,
    ) -> Result<Vec<FieldFilter>, Signal> {
        let engine = self.engine.clone();
        let table = rec.borrow().table as usize;
        let own = &engine.program.objects[table].table.as_ref().expect("table").fields;
        let src = &engine.program.objects[calc.table].table.as_ref().expect("table").fields;
        let enums = |id: u32| engine.program.objects[id as usize].enum_values.clone();
        let parse = |text: &str, ty: &RtType| {
            parse_filter_with(text, &[], ty, &enums)
                .map_err(|e| Signal::Unsupported(format!("CalcFormula filter: {e}")))
        };
        // The flow filter the caller set on field `q`, if any.
        let flow = |q: usize| {
            rec.borrow()
                .filters
                .iter()
                .find(|x| x.field == q)
                .map(|x| x.expr.clone())
        };
        // (source field, filter, replaces other conditions on that field)
        let mut parts: Vec<(usize, FilterExpr, bool)> = Vec::new();
        for c in &calc.conds {
            let sty = &src[c.field].ty;
            match &c.value {
                ir::CalcValue::Const(k) => {
                    parts.push((c.field, FilterExpr::Eq(ops::convert(crate::constant(k), sty)?), false));
                }
                ir::CalcValue::Filter(text) => parts.push((c.field, parse(text, sty)?, false)),
                ir::CalcValue::Field(q) => {
                    let pf = &own[*q];
                    if pf.flow_filter {
                        // Corpus record/TestCalcFormulaFlowFiltersTests.Codeunit.al:107: the
                        // caller's whole filter applies; none set means no constraint.
                        if let Some(e) = flow(*q) {
                            if !same_kind(&pf.ty, sty) {
                                return unsupported("a flow filter of another type than the source field");
                            }
                            parts.push((c.field, e, false));
                        }
                    } else if !pf.stored {
                        // Corpus record/TestCalcFormulaFlowFieldValueTests.Codeunit.al:112: a
                        // FlowField on the other side is calculated first, not read from the buffer.
                        self.check_calc_field(table, *q)?;
                        let v = self.calc_value(rec, *q, stack)?;
                        parts.push((c.field, FilterExpr::Eq(ops::convert(v, sty)?), false));
                    } else {
                        let v = rec.borrow().fields[*q].clone();
                        parts.push((c.field, FilterExpr::Eq(ops::convert(v, sty)?), false));
                    }
                }
                ir::CalcValue::UpperLimit(q) => {
                    if !own[*q].flow_filter {
                        return unsupported("upperlimit() of a field that is not a flow filter");
                    }
                    // Corpus record/TestCalcFormulaFlowFiltersTests.Codeunit.al:144: only the
                    // upper bound of the caller's range; none set (or an open end) means none.
                    match flow(*q) {
                        None | Some(FilterExpr::Range(_, None)) => {}
                        Some(FilterExpr::Range(_, Some(hi))) => {
                            parts.push((c.field, FilterExpr::Range(None, Some(hi)), false));
                        }
                        Some(_) => return unsupported("upperlimit() of a filter that is not a range"),
                    }
                }
                ir::CalcValue::FieldFilter(q) => {
                    let v = rec.borrow().fields[*q].clone();
                    let Some(text) = text_of(&v) else {
                        return unsupported("field(filter()) of a field that is not text");
                    };
                    // Corpus record/TestCalcFormulaFlowFiltersTests.Codeunit.al:194-230: the value is
                    // read as a filter expression, blank imposes nothing, and a filter takes over
                    // from a link on the same source field.
                    if !text.trim().is_empty() {
                        parts.push((c.field, parse(&text, sty)?, true));
                    }
                }
            }
        }
        let overridden: Vec<usize> = parts.iter().filter(|p| p.2).map(|p| p.0).collect();
        Ok(parts
            .into_iter()
            .filter(|(field, _, is_override)| *is_override || !overridden.contains(field))
            .map(|(field, expr, _)| FieldFilter { field, expr })
            .collect())
    }

    /// Calls `f` on every row of `t` passing `filters`, in primary key order.
    fn each_row(
        &mut self,
        t: &TableDef,
        filters: &[FieldFilter],
        mut f: impl FnMut(&[Value]) -> Result<(), Signal>,
    ) -> Result<(), Signal> {
        let cx = self.cx.clone();
        let mut after: Option<Vec<Value>> = None;
        loop {
            let next = self
                .store
                .next(
                    &cx,
                    t,
                    &Scan {
                        key: 0,
                        filters,
                        ascending: true,
                        after: after.as_deref(),
                    },
                )
                .map_err(|e| Signal::Unsupported(format!("store error {e:?}")))?;
            let Some(row) = next else { return Ok(()) };
            f(&row)?;
            after = Some(position(t, 0, &row));
        }
    }

    /// `CalcSums`: the sum of stored number fields over the record's own
    /// filters (corpus record/TestRecordFlowField.al:109-160).
    pub(crate) fn calc_sums(&mut self, rec: &Rec, positions: &[usize]) -> Result<(), Signal> {
        let engine = self.engine.clone();
        let table = rec.borrow().table as usize;
        let t = self.engine.tables[table].clone().expect("table");
        let fields = &engine.program.objects[table].table.as_ref().expect("table").fields;
        for &p in positions {
            let f = &fields[p];
            if !f.stored || !matches!(f.ty, RtType::Decimal | RtType::Integer | RtType::BigInteger) {
                return unsupported(format!(
                    "CalcSums on {} (only stored number fields are implemented)",
                    f.name
                ));
            }
        }
        let (filters, flow_field_filters) = self.split_filters(rec);
        if !flow_field_filters.is_empty() {
            return unsupported("CalcSums with a filter on a FlowField");
        }
        let mut sums = vec![Decimal::ZERO; positions.len()];
        self.each_row(&t, &filters, |row| {
            for (s, &p) in sums.iter_mut().zip(positions) {
                *s += number(&row[p]).ok_or_else(|| Signal::Unsupported("sum over a non-number".into()))?;
            }
            Ok(())
        })?;
        // The sums land in the record's fields only once all are known.
        let mut out = Vec::new();
        for (s, &p) in sums.into_iter().zip(positions) {
            out.push((p, to_number(s, &fields[p].ty, false)?));
        }
        let mut r = rec.borrow_mut();
        for (p, v) in out {
            r.fields[p] = v;
        }
        Ok(())
    }

    /// Splits a record's filters into those the store can evaluate (stored
    /// fields) and those on FlowFields. Filters on FlowFilter fields are
    /// dropped: they hold no data and never constrain their own table (corpus
    /// record/TestCalcFormulaFlowFiltersTests.Codeunit.al:258).
    pub(crate) fn split_filters(&self, rec: &Rec) -> (Vec<FieldFilter>, Vec<FieldFilter>) {
        let r = rec.borrow();
        let fields = &self.engine.program.objects[r.table as usize]
            .table
            .as_ref()
            .expect("table")
            .fields;
        let (mut stored, mut flow_fields) = (Vec::new(), Vec::new());
        for f in &r.filters {
            let schema = &fields[f.field];
            if schema.stored {
                stored.push(f.clone());
            } else if !schema.flow_filter {
                flow_fields.push(f.clone());
            }
        }
        (stored, flow_fields)
    }

    /// Whether a row passes the filters set on FlowFields: each is checked
    /// against the field's calculated value for that row (corpus
    /// record/TestRecordFlowFieldFilterVisitor.al:30,49).
    pub(crate) fn passes_flow_field_filters(
        &mut self,
        rec: &Rec,
        row: &[Value],
        filters: &[FieldFilter],
    ) -> Result<bool, Signal> {
        let table = rec.borrow().table as usize;
        let probe = {
            let r = rec.borrow();
            let mut p = r.clone();
            p.fields = row.to_vec();
            p.temporary = false;
            p.auto_calc.clear();
            Rc::new(RefCell::new(p))
        };
        for f in filters {
            self.check_calc_field(table, f.field)?;
            let v = self.calc_value(&probe, f.field, &mut Vec::new())?;
            if !f.expr.matches(&v) {
                return Ok(false);
            }
        }
        Ok(true)
    }

    pub(crate) fn flow_op(&mut self, method: &str, rec: &Rec, args: &[Value]) -> Result<Value, Signal> {
        let positions: Vec<usize> = args
            .iter()
            .map(|a| match a {
                Value::Integer(p) => Ok(*p as usize),
                other => unsupported(format!("field argument {other:?}")),
            })
            .collect::<Result<_, _>>()?;
        match method {
            "CalcFields" => {
                self.calc_fields(rec, &positions)?;
            }
            "CalcSums" => {
                self.calc_sums(rec, &positions)?;
            }
            "SetAutoCalcFields" => {
                let table = rec.borrow().table as usize;
                for &p in &positions {
                    self.check_calc_field(table, p)?;
                }
                rec.borrow_mut().auto_calc = positions;
            }
            other => return unsupported(format!("Record.{other}")),
        }
        Ok(Value::Boolean(true))
    }

    /// After a row is read into `rec`: the fields of `SetAutoCalcFields`.
    pub(crate) fn auto_calculate(&mut self, rec: &Rec) -> Result<(), Signal> {
        let auto = rec.borrow().auto_calc.clone();
        if auto.is_empty() {
            return Ok(());
        }
        self.calc_fields(rec, &auto)
    }
}
