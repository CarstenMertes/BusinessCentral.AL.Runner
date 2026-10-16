//! `TestPage.Filter` / `TestPart.Filter` (the `TestFilter` handle): the test's way to change which
//! rows an open page has and in what order. It writes the page's user filter, filter group 0
//! (corpus testfilter/TestTestFilter.al, TestTestFilterFilterGroups.al).

use super::*;
use al_builtins::describe::Describe;
use al_builtins::filter::FieldFilter;
use al_builtins::page_op::PageMethod;

impl Session {
    pub(super) fn filter_handle(&self, st: &St) -> Value {
        Value::TestControl(Rc::new(ControlRef {
            page: st.clone(),
            target: ControlTarget::Filter,
        }))
    }

    /// The `TestFilter.*` methods on the page `st`.
    pub(super) fn test_filter_op(&mut self, method: PageMethod, st: &St, args: &mut [Value]) -> Result<Value, Signal> {
        if !self.has_source(st) {
            return unsupported("TestFilter on a page without a source table");
        }
        let rec = self.page_rec(st);
        let table = self.table(&rec)?;
        match method {
            // The first filter on the field across the filter groups, in the order they were
            // written (corpus testfilter/TestTestFilterFilterGroups.al:3 and 4).
            PageMethod::GetFilter => {
                let pos = field_arg(args, 0)?;
                let first = rec.borrow().filters.iter().find(|f| f.field == pos).cloned();
                match first {
                    Some(f) => Ok(Value::Text(self.filter_text(&f.expr)?.into())),
                    None => Ok(Value::Text(String::new().into())),
                }
            }
            PageMethod::SetFilter => {
                let pos = field_arg(args, 0)?;
                let text = match args.get(1) {
                    Some(Value::Text(t) | Value::Code(t)) => t.clone(),
                    other => return unsupported(format!("TestFilter.SetFilter with {}", other.describe())),
                };
                let ty = table.fields[pos].ty.clone();
                let expr = self
                    .parse_filter_text(&text, &ty)
                    .map_err(|e| Signal::Unsupported(format!("TestFilter.SetFilter: {e}")))?;
                // The user filter is group 0, whatever group OnOpenPage left active; setting a
                // field's filter again replaces the group-0 one.
                {
                    // In place when there is one: group 0 keeps its place ahead of the groups the
                    // page wrote after it (corpus testfilter/TestTestFilterFilterGroups.al:4).
                    let mut r = rec.borrow_mut();
                    match r.filters.iter().position(|f| f.field == pos && f.group == 0) {
                        Some(i) => r.filters[i].expr = expr,
                        None => r.filters.push(FieldFilter {
                            field: pos,
                            expr,
                            group: 0,
                        }),
                    }
                }
                self.filter_changed(st)?;
                Ok(Value::Null)
            }
            PageMethod::CurrentKey => {
                let key = rec.borrow().key;
                let names: Vec<&str> = table.keys[key].iter().map(|&p| table.fields[p].name.as_str()).collect();
                Ok(Value::Text(names.join(",").into()))
            }
            PageMethod::SetCurrentKey => {
                let mut wanted = Vec::new();
                for i in 0..args.len() {
                    wanted.push(field_arg(args, i)?);
                }
                let Some(key) = table
                    .keys
                    .iter()
                    .position(|k| k.len() >= wanted.len() && k[..wanted.len()] == wanted[..])
                else {
                    return unsupported("TestFilter.SetCurrentKey without a matching key (error text not pinned)");
                };
                rec.borrow_mut().key = key;
                self.filter_changed(st)?;
                Ok(Value::Null)
            }
            PageMethod::Ascending => match args.first() {
                Some(v) => {
                    rec.borrow_mut().ascending = ops::truthy(v)?;
                    self.filter_changed(st)?;
                    Ok(Value::Null)
                }
                None => Ok(Value::Boolean(rec.borrow().ascending)),
            },
            other => unsupported(format!("TestFilter.{other}")),
        }
    }

    /// The page reads its rows again after the filter or the key changed, and stands on the first
    /// match (corpus handlers/TestPageFilterPosition_Tests.al:SetFilter_ExcludingTheCurrentRow_*,
    /// SetFilter_EvenWhenCurrentRowStillQualifies_RepositionsToTheFirstMatch).
    fn filter_changed(&mut self, st: &St) -> Result<(), Signal> {
        self.save_row(st)?;
        {
            let mut s = st.borrow_mut();
            s.cursor = Cursor::None;
            s.draft = false;
            s.dirty = false;
            s.window = Vec::new();
            s.focus = None;
            s.focus_in_row = false;
        }
        let r = self.page_first_row(st);
        self.torn_down_on_error(st, r)?;
        // A page the filter left without a row shows a blank one, its key pre-filled from a
        // single-valued filter (corpus pageupdate/TestPageUpdateGoneRow.al:
        // Card_OpenEdit_FilterMatchesNothing_NoOpAction_StaysOpen).
        if st.borrow().cursor == Cursor::None {
            let rec = self.page_rec(st);
            let table = rec.borrow().table as usize;
            let fresh = self.initial_fields(table);
            rec.borrow_mut().fields = fresh;
            self.stamp_from_filters(st, false)?;
        }
        if st.borrow().host.is_some() {
            self.sync_dependents(st)
        } else {
            self.sync_parts(st)
        }
    }
}

fn field_arg(args: &[Value], i: usize) -> Result<usize, Signal> {
    match args.get(i) {
        Some(Value::Integer(p)) => Ok(*p as usize),
        other => unsupported(format!("TestFilter field argument {}", other.describe())),
    }
}
