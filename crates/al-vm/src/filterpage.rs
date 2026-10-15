//! `FilterPageBuilder`: an ordered, keyed set of filter controls, each holding a record buffer.
//! Everything up to `RunModal` is an object-graph operation (corpus
//! filterpage/TestFilterPageBuilder.al); `RunModal` needs a client the corpus never provides and
//! is refused.

use crate::Session;
use al_builtins::describe::Describe;
use al_builtins::filter::FieldFilter;
use al_builtins::native::{AlError, Signal};
use al_builtins::ops;
use al_builtins::value::{FilterControl, FilterPageState, LibraryValue, RtType, Value};
use std::cell::RefCell;
use std::rc::Rc;

type Lib = Rc<RefCell<LibraryValue>>;

fn unsupported<T>(what: impl Into<String>) -> Result<T, Signal> {
    Err(Signal::Unsupported(what.into()))
}

/// An argument error: code `Argument` (filterpage/W3FilterPageBuilder.al, BC 27.5 and 29.0).
fn arg_error(message: String) -> Signal {
    Signal::Error(AlError {
        message,
        code: "Argument".into(),
        call_stack: String::new(),
    })
}

fn al_error(message: String) -> Signal {
    Signal::Error(AlError {
        message,
        code: String::new(),
        call_stack: String::new(),
    })
}

/// The caption of a builder nobody assigned one to: the platform's localized default, which the
/// corpus only knows to be non-empty (filterpage/TestFilterPageBuilder.al:
/// FilterPageBuilder_PageCaption_FreshBuilder_IsNonEmptyDefault). docs/probes-needed.md.
const DEFAULT_CAPTION: &str = "Filter Page";

fn text(args: &[Value], i: usize) -> Result<String, Signal> {
    match args.get(i).and_then(al_builtins::native::text_of) {
        Some(t) => Ok(t),
        None => unsupported(format!("FilterPageBuilder argument {i}: {}", args.get(i).describe())),
    }
}

/// The state behind `cell`; an untouched builder becomes an empty one on first use.
fn state_of(cell: &Lib) -> Result<std::cell::RefMut<'_, FilterPageState>, Signal> {
    let mut lib = cell.borrow_mut();
    if matches!(&*lib, LibraryValue::Empty) {
        *lib = LibraryValue::FilterPageBuilder(FilterPageState::default());
    }
    std::cell::RefMut::filter_map(lib, |l| match l {
        LibraryValue::FilterPageBuilder(s) => Some(s),
        _ => None,
    })
    .map_err(|_| Signal::Unsupported("FilterPageBuilder method on another kind of value".into()))
}

/// A copy of a builder for `:=`: the control collection is copied, the record behind each control
/// is shared (corpus filterpage/TestFilterPageBuilder.al:FilterPageBuilder_Assignment_*).
pub(crate) fn copy_for_assignment(cell: &Lib) -> Lib {
    let copy = match &*cell.borrow() {
        LibraryValue::FilterPageBuilder(s) => LibraryValue::FilterPageBuilder(s.clone()),
        _ => LibraryValue::Empty,
    };
    Rc::new(RefCell::new(copy))
}

impl Session {
    pub(crate) fn filter_page_builder_op(
        &mut self,
        id: &str,
        recv: Option<Value>,
        args: &[Value],
        ret_used: bool,
    ) -> Result<Value, Signal> {
        let Some(Value::Library(_, cell)) = recv else {
            return unsupported(format!("{id} without a FilterPageBuilder"));
        };
        let method = id
            .strip_prefix("FilterPageBuilder.")
            .and_then(|m| m.split('#').next())
            .unwrap_or(id);
        // The calls that fail answer `fallback` when their result is used and raise otherwise
        // (filterpage/TestFilterPageBuilder.al: the trappable-return convention, header item 4).
        let fail = |message: String, fallback: Value| {
            if ret_used {
                Ok(fallback)
            } else {
                Err(arg_error(message))
            }
        };
        match method {
            "Count" => Ok(Value::Integer(state_of(&cell)?.controls.len() as i32)),
            "Name" => {
                let Some(Value::Integer(i)) = args.first() else {
                    return unsupported("FilterPageBuilder.Name without an integer index");
                };
                let s = state_of(&cell)?;
                let n = s.controls.len() as i32;
                // :FilterPageBuilder_Name_IndexZero_Throws, :Name_IndexPastEnd_Throws
                match usize::try_from(i - 1).ok().and_then(|k| s.controls.get(k)) {
                    Some(c) => Ok(Value::Text(c.name.clone())),
                    None => Err(al_error(format!(
                        "The filter control index value {i} is out of range. The value must be greater than or equal to 1 and less than or equal to {n}."
                    ))),
                }
            }
            "PageCaption" => match args.first() {
                Some(v) => {
                    state_of(&cell)?.caption = Some(text(std::slice::from_ref(v), 0)?);
                    Ok(Value::Null)
                }
                None => Ok(Value::Text(
                    state_of(&cell)?
                        .caption
                        .clone()
                        .unwrap_or_else(|| DEFAULT_CAPTION.to_string()),
                )),
            },
            "AddTable" | "AddRecord" | "AddRecordRef" => {
                let name = text(args, 0)?;
                let (table, record) = match method {
                    "AddTable" => {
                        let Some(Value::Integer(no)) = args.get(1) else {
                            return unsupported("FilterPageBuilder.AddTable without an integer table id");
                        };
                        if *no < 1 {
                            // :FilterPageBuilder_AddTable_ZeroTableId_Throws
                            return fail(
                                "The filter page table ID must be a positive number greater than zero.".into(),
                                Value::Text(String::new()),
                            );
                        }
                        let Some(table) = self.table_object(i64::from(*no)) else {
                            return unsupported(format!(
                                "FilterPageBuilder.AddTable of table {no}, which no table declares (error text not pinned)"
                            ));
                        };
                        let Value::Record(rec) = self.default_value(&RtType::Record(table as u32, false)) else {
                            return unsupported("a record buffer for FilterPageBuilder.AddTable");
                        };
                        (table as u32, rec)
                    }
                    "AddRecord" => match args.get(1) {
                        Some(Value::Record(r)) => (r.borrow().table, Rc::new(RefCell::new(r.borrow().clone()))),
                        other => return unsupported(format!("FilterPageBuilder.AddRecord with {}", other.describe())),
                    },
                    _ => match args.get(1) {
                        Some(Value::Library(_, l)) => match &*l.borrow() {
                            // The builder keeps the RecordRef's own buffer: a filter set on the RecordRef afterwards shows in the
                            // builder's view, where AddRecord copies a Record (filterpage/W3FilterPageBuilder.al:
                            // AddRecord_CopiesTheFiltersOfTheRecord, BC 27.5 and 29.0).
                            LibraryValue::RecordRef(r) => (r.borrow().table, r.clone()),
                            other => {
                                return unsupported(format!(
                                    "FilterPageBuilder.AddRecordRef with a RecordRef that is not open ({other})",
                                    other = other.describe()
                                ));
                            }
                        },
                        other => {
                            return unsupported(format!("FilterPageBuilder.AddRecordRef with {}", other.describe()));
                        }
                    },
                };
                let mut s = state_of(&cell)?;
                match s.controls.iter().find(|c| c.name == name) {
                    // :AddTable_SameNameSameTable_IsIdempotent
                    Some(c) if c.table == table => Ok(Value::Text(name)),
                    // :AddTable_SameNameDifferentTable_Throws pins only that the text names the
                    // control; the rest of the sentence is docs/probes-needed.md.
                    Some(_) => {
                        drop(s);
                        fail(
                            format!("A table that has the name {name} already exists with a different ID."),
                            Value::Text(String::new()),
                        )
                    }
                    None => {
                        s.controls.push(FilterControl {
                            name: name.clone(),
                            table,
                            record,
                        });
                        Ok(Value::Text(name))
                    }
                }
            }
            "AddField" => {
                let name = text(args, 0)?;
                let Some(control) = self.filter_control(&cell, &name)? else {
                    // The unknown-name answer is pinned for a used result only
                    // (:AddField_UnknownControl_ReturnsFalse).
                    return fail(
                        format!("Filter control {name} is not defined in the FilterPageBuilder variable."),
                        Value::Boolean(false),
                    );
                };
                let pos = match args.get(1) {
                    Some(Value::Library(_, l)) => match &*l.borrow() {
                        LibraryValue::FieldRef(r, pos) if r.borrow().table == control.table => *pos,
                        LibraryValue::FieldRef(..) => {
                            return unsupported("FilterPageBuilder.AddField with a field of another table");
                        }
                        other => return unsupported(format!("FilterPageBuilder.AddField with {}", other.describe())),
                    },
                    other => return unsupported(format!("FilterPageBuilder.AddField with {}", other.describe())),
                };
                if let Some(filter) = args.get(2) {
                    let filter = text(std::slice::from_ref(filter), 0)?;
                    let t = self.table(&control.record)?;
                    let ty = t.fields[pos].ty.clone();
                    let expr = self
                        .parse_filter_text(&filter, &ty)
                        .map_err(|e| Signal::Unsupported(format!("FilterPageBuilder.AddField: {e}")))?;
                    let mut r = control.record.borrow_mut();
                    let group = r.filter_group;
                    r.filters.retain(|f| !(f.field == pos && f.group == group));
                    r.filters.push(FieldFilter {
                        field: pos,
                        expr,
                        group,
                    });
                }
                Ok(Value::Boolean(true))
            }
            "GetView" => {
                let name = text(args, 0)?;
                let use_names = match args.get(1) {
                    Some(v) => ops::truthy(v)?,
                    None => true,
                };
                match self.filter_control(&cell, &name)? {
                    // :GetView_UnknownControl_ReturnsEmpty
                    None => Ok(Value::Text(String::new())),
                    Some(c) => {
                        let t = self.table(&c.record)?;
                        // :GetView_CaptionsFlag_RendersAnOptionFilterDifferently
                        // The view starts with `VERSION(1) ` like a Record's (filterpage/W3FilterPageBuilder.al:
                        // AddRecord_CopiesTheFiltersOfTheRecord, BC 27.5 and 29.0).
                        self.get_view_with(&c.record, &t, use_names, !use_names, false)
                            .map(|v| Value::Text(format!("VERSION(1) {v}")))
                    }
                }
            }
            "SetView" => {
                let name = text(args, 0)?;
                let view = text(args, 1)?;
                let Some(control) = self.filter_control(&cell, &name)? else {
                    // :SetView_UnknownControl_Throws names the control; the rest is
                    // docs/probes-needed.md. :SetView_ReturnCaptured_UnknownControl_ReturnsFalse
                    return fail(
                        format!("Filter control {name} is not defined in the FilterPageBuilder variable."),
                        Value::Boolean(false),
                    );
                };
                if view.is_empty() {
                    // :SetView_EmptyView_ReturnsFalse pins the used result only.
                    return fail("The filter page view cannot be empty.".into(), Value::Boolean(false));
                }
                let t = self.table(&control.record)?;
                self.set_view(&control.record, &t, &view)?;
                Ok(Value::Boolean(true))
            }
            "RunModal" => {
                unsupported("FilterPageBuilder.RunModal opens a filter page; no corpus test pins how a test answers it")
            }
            other => unsupported(format!("FilterPageBuilder.{other}")),
        }
    }

    fn filter_control(&self, cell: &Lib, name: &str) -> Result<Option<FilterControl>, Signal> {
        Ok(state_of(cell)?.controls.iter().find(|c| c.name == name).cloned())
    }
}
