//! The test client for pages (ADR-012): the part of BC's test framework that
//! opens a page, moves on its rows, reads and writes its controls and closes
//! it again. The page itself runs as ordinary AL (triggers, procedures,
//! property routines) on an `Instance` in this session.
//!
//! Every rule below cites the corpus test that pins it; behaviour no test
//! pins raises `Signal::Unsupported`.

use crate::{Engine, Session};
use al_builtins::native::{AlError, Signal};
use al_builtins::ops;
use al_builtins::pagefmt::{self, Blank, ControlFormat};
use al_builtins::value::{ControlRef, ControlTarget, Cursor, PageMode, PageState, RecordState, RtType, Value};
use al_ir as ir;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

type St = Rc<RefCell<PageState>>;
type Rec = Rc<RefCell<RecordState>>;

/// corpus testpageteardown/TestPageTeardown.al:227, testpageclose/TestPageClosingActions.al:338
const NOT_OPEN: &str = "The TestPage is not open.";
/// corpus testpageclose/TestPageClosingActions.al:339
const ALREADY_OPEN: &str = "The TestPage is already open.";

fn error(message: impl Into<String>) -> Signal {
    Signal::Error(AlError {
        message: message.into(),
        code: String::new(),
        call_stack: String::new(),
    })
}

fn unsupported<T>(what: impl Into<String>) -> Result<T, Signal> {
    Err(Signal::Unsupported(what.into()))
}

fn page_of<'a>(engine: &'a Engine, st: &St) -> &'a ir::Page {
    let object = st.borrow().object as usize;
    engine.program.objects[object]
        .page
        .as_ref()
        .expect("a TestPage is typed by a page object")
}

/// `'text'` → `text`; anything else as written.
fn unquote(s: &str) -> String {
    let t = s.trim();
    match t.strip_prefix('\'').and_then(|x| x.strip_suffix('\'')) {
        Some(inner) => inner.replace("''", "'"),
        None => t.to_string(),
    }
}

fn is_true(s: &str) -> bool {
    s.trim().eq_ignore_ascii_case("true")
}

impl Session {
    /// `TP.Ctl`: a handle on control `control` of the test page `page`.
    pub(crate) fn page_control(&mut self, page: Value, control: usize) -> Result<Value, Signal> {
        match page {
            Value::TestPage(p) => Ok(Value::TestControl(Rc::new(ControlRef {
                page: p,
                target: ControlTarget::Control(control),
            }))),
            other => Err(Signal::Unsupported(format!("control of {other:?}"))),
        }
    }

    fn engine(&self) -> Arc<Engine> {
        self.engine.clone()
    }

    /// The page's `Rec`.
    fn page_rec(&self, st: &St) -> Rec {
        let object = st.borrow().object as usize;
        let slot = self.engine.program.objects[object]
            .rec_slot
            .expect("page with a source table");
        let inst = st.borrow().instance.clone().expect("open page");
        let v = inst.borrow().globals[slot].clone();
        match v {
            Value::Record(r) => r,
            other => panic!("Rec is {other:?}"),
        }
    }

    fn require_open(&self, st: &St) -> Result<(), Signal> {
        if st.borrow().open { Ok(()) } else { Err(error(NOT_OPEN)) }
    }

    /// Runs page trigger `name` (lower case) if the page declares it.
    fn page_trigger(&mut self, st: &St, name: &str, args: Vec<Value>) -> Result<Option<Value>, Signal> {
        let engine = self.engine();
        let object = st.borrow().object as usize;
        let Some(r) = page_of(&engine, st)
            .triggers
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, r)| *r)
        else {
            return Ok(None);
        };
        let inst = st.borrow().instance.clone().expect("open page");
        self.call(object, r, inst, args).map(|(v, _)| Some(v))
    }

    /// Calls routine `r` of the page.
    fn page_routine(&mut self, st: &St, r: usize) -> Result<Value, Signal> {
        let object = st.borrow().object as usize;
        let inst = st.borrow().instance.clone().expect("open page");
        self.call(object, r, inst, vec![]).map(|(v, _)| v)
    }

    /// The `TestPage.*` / `TestField.*` methods (ADR-012).
    pub(crate) fn page_op(
        &mut self,
        method: &str,
        recv: Option<&mut Value>,
        args: &mut [Value],
    ) -> Result<Value, Signal> {
        match recv {
            Some(Value::TestPage(st)) => {
                let st = st.clone();
                self.test_page_op(method, &st, args)
            }
            Some(Value::TestControl(c)) => {
                let c = c.clone();
                self.test_control_op(method, &c, args)
            }
            other => unsupported(format!("{method} on {other:?}")),
        }
    }

    fn test_page_op(&mut self, method: &str, st: &St, args: &mut [Value]) -> Result<Value, Signal> {
        match method.split_once('.').map(|(_, m)| m).unwrap_or(method) {
            "OpenEdit" => self.page_open(st, PageMode::Edit),
            "OpenView" => self.page_open(st, PageMode::View),
            "OpenNew" => self.page_open(st, PageMode::New),
            "Close" => self.page_close(st),
            m @ ("First" | "Last" | "Next" | "Previous") => {
                self.require_open(st)?;
                self.page_move(st, m)
            }
            "GoToKey" => {
                self.require_open(st)?;
                self.page_go_to_key(st, args)
            }
            "GoToRecord" => {
                self.require_open(st)?;
                let Some(Value::Record(r)) = args.first() else {
                    return unsupported("GoToRecord without a record");
                };
                let table = self.table(r)?;
                let mut key: Vec<Value> = table.primary_key(&r.borrow().fields);
                self.page_go_to_key(st, &mut key)
            }
            other => unsupported(format!("TestPage.{other}")),
        }
    }

    // ---- opening and closing ------------------------------------------------------------

    fn page_open(&mut self, st: &St, mode: PageMode) -> Result<Value, Signal> {
        // corpus testpageclose/TestPageClosingActions.al:19-24: an open variable cannot be opened.
        if st.borrow().open {
            return Err(error(ALREADY_OPEN));
        }
        let engine = self.engine();
        let object = st.borrow().object as usize;
        let page = page_of(&engine, st);
        if let Some(why) = page.unsupported.first() {
            return unsupported(why.clone());
        }
        let inst = self.instance(object);
        inst.borrow_mut().globals[page.curr_page_slot] = Value::TestPage(st.clone());
        {
            let mut s = st.borrow_mut();
            s.open = true;
            s.mode = mode;
            s.instance = Some(inst);
            s.cursor = Cursor::None;
            s.draft = false;
            s.dirty = false;
            s.frozen_visible = Vec::new();
            s.window = Vec::new();
        }
        let result = (|| {
            self.page_trigger(st, "oninit", vec![])?;
            self.page_trigger(st, "onopenpage", vec![])?;
            let new_line = self.offers_new_line(st)?;
            st.borrow_mut().new_line = new_line;
            // Visible is evaluated once, at open.
            let mut frozen = Vec::new();
            for i in 0..page.controls.len() {
                frozen.push(self.prop_bool_now(st, i, "visible", true)?);
            }
            st.borrow_mut().frozen_visible = frozen;
            if mode == PageMode::New {
                return unsupported("OpenNew");
            }
            self.page_first_row(st)
        })();
        if let Err(e) = result {
            // The open raises the error itself and leaves the variable closed
            // (corpus testpageteardown/TestPageTeardown.al:3-24).
            st.borrow_mut().open = false;
            return Err(e);
        }
        Ok(Value::Null)
    }

    fn page_close(&mut self, st: &St) -> Result<Value, Signal> {
        self.require_open(st)?;
        // corpus handlers/TestPageRecordTriggers.al:187 OnClosePage_RunsWhenThePageIsClosed
        self.page_trigger(st, "onclosepage", vec![])?;
        st.borrow_mut().open = false;
        Ok(Value::Null)
    }

    /// Whether the page carries the blank line past its data: an editable,
    /// insert-allowed repeater opened with OpenEdit (corpus
    /// handlers/TestPageNewRowLine_Tests.al:1-30, gating arms).
    fn offers_new_line(&mut self, st: &St) -> Result<bool, Signal> {
        let engine = self.engine();
        let object = st.borrow().object as usize;
        let page = page_of(&engine, st);
        let has_repeater = page.controls.iter().any(|c| c.kind == ir::ControlKind::Repeater);
        let props = &engine.program.objects[object].properties;
        let prop_false = |n: &str| {
            props
                .iter()
                .any(|(k, v)| k.eq_ignore_ascii_case(n) && v.trim().eq_ignore_ascii_case("false"))
        };
        Ok(has_repeater
            && st.borrow().mode == PageMode::Edit
            && !prop_false("Editable")
            && !prop_false("InsertAllowed"))
    }

    // ---- rows ---------------------------------------------------------------------------

    /// Lands on the first row of the view when the page opens: a stored row, else
    /// the new-row line, else nothing. The expressions of the controls are evaluated for
    /// an empty view too (corpus autoformat/TestAutoFormatExpressionTiming.al:5-12).
    fn page_first_row(&mut self, st: &St) -> Result<(), Signal> {
        let rec = self.page_rec(st);
        self.ret_used = true;
        let found = self.record_op("FindFirst", &rec, &mut [])?;
        if matches!(found, Value::Boolean(true)) {
            st.borrow_mut().cursor = Cursor::Row;
            self.arrive(st)
        } else if st.borrow().new_line {
            self.land_on_new_line(st)?;
            self.populate_expressions(st)
        } else {
            self.populate_expressions(st)
        }
    }

    fn land_on_new_line(&mut self, st: &St) -> Result<(), Signal> {
        let rec = self.page_rec(st);
        let table = rec.borrow().table as usize;
        let fresh = self.initial_fields(table);
        rec.borrow_mut().fields = fresh;
        let mut s = st.borrow_mut();
        s.cursor = Cursor::NewLine;
        s.draft = true;
        Ok(())
    }

    /// The expressions every row population evaluates, whatever the control's Visible
    /// (corpus autoformat/TestAutoFormatExpressionTiming.al:5-12): AutoFormatExpression and
    /// CaptionClass. They run after OnOpenPage and before OnAfterGetRecord.
    fn populate_expressions(&mut self, st: &St) -> Result<(), Signal> {
        let engine = self.engine();
        let page = page_of(&engine, st);
        for c in &page.controls {
            for name in ["autoformatexpression", "captionclass"] {
                match c.prop(name) {
                    Some(ir::PropVal::Routine(r)) => {
                        let v = self.page_routine(st, *r)?;
                        if name == "captionclass" {
                            if let Value::Text(t) | Value::Code(t) = &v {
                                if !t.starts_with("3,") {
                                    return unsupported(format!("CaptionClass `{t}` is resolved by the platform"));
                                }
                            }
                        }
                    }
                    // An area no built-in resolver claims goes to the Caption Class codeunit's
                    // OnResolveCaptionClass event (corpus autoformat/TestCaptionClassResolverFailure.al:1-12).
                    Some(ir::PropVal::Lit(l)) if name == "captionclass" && !unquote(l).starts_with("3,") => {
                        return unsupported(format!("CaptionClass `{}` is resolved by the platform", unquote(l)));
                    }
                    _ => {}
                }
            }
        }
        Ok(())
    }

    /// Populates the current row: the control expressions, then OnAfterGetRecord.
    fn populate_current(&mut self, st: &St) -> Result<(), Signal> {
        st.borrow_mut().draft = false;
        st.borrow_mut().dirty = false;
        self.populate_expressions(st)?;
        self.page_trigger(st, "onaftergetrecord", vec![])?;
        Ok(())
    }

    fn is_list(&self, st: &St) -> bool {
        let engine = self.engine();
        page_of(&engine, st)
            .controls
            .iter()
            .any(|c| c.kind == ir::ControlKind::Repeater)
    }

    fn row_key(&self, rec: &Rec) -> Result<Vec<Value>, Signal> {
        let table = self.table(rec)?;
        Ok(table.primary_key(&rec.borrow().fields))
    }

    /// A repeater populates a window of rows at a time; a row inside the window it already
    /// holds is not populated again (corpus autoformat/TestAutoFormatExpressionTiming.al:5-12:
    /// a failing row 20 of 120 fails the open, row 30 does not, and a move to row 120
    /// populates a new window). The window is 20 rows: the corpus pins it only between 20 and 24.
    const WINDOW: usize = 20;

    fn populate_window(&mut self, st: &St) -> Result<(), Signal> {
        let rec = self.page_rec(st);
        let start = self.row_key(&rec)?;
        let mut keys = Vec::new();
        for i in 0..Self::WINDOW {
            keys.push(self.row_key(&rec)?);
            self.populate_current(st)?;
            if i + 1 == Self::WINDOW {
                break;
            }
            self.ret_used = true;
            if !matches!(
                self.record_op("Next", &rec, &mut [Value::Integer(1)])?,
                Value::Integer(1)
            ) {
                break;
            }
        }
        // Back on the row the cursor is on.
        self.ret_used = true;
        self.record_op("Get", &rec, &mut start.clone())?;
        st.borrow_mut().window = keys;
        Ok(())
    }

    fn in_window(&self, st: &St, key: &[Value]) -> bool {
        st.borrow().window.iter().any(|k| {
            k.len() == key.len()
                && k.iter().zip(key).all(|(a, b)| {
                    ops::compare(a, b)
                        .map(|o| o == std::cmp::Ordering::Equal)
                        .unwrap_or(false)
                })
        })
    }

    /// The cursor has landed on a stored row: populate it as the page type does, then
    /// OnAfterGetCurrRecord.
    fn arrive(&mut self, st: &St) -> Result<(), Signal> {
        if self.is_list(st) {
            let key = self.row_key(&self.page_rec(st))?;
            if !self.in_window(st, &key) {
                self.populate_window(st)?;
            }
        } else {
            self.populate_current(st)?;
        }
        self.page_trigger(st, "onaftergetcurrrecord", vec![])?;
        Ok(())
    }

    /// Moves the cursor; the Boolean the move methods return.
    fn page_move(&mut self, st: &St, how: &str) -> Result<Value, Signal> {
        let r = self.page_move_inner(st, how);
        self.torn_down_on_error(st, r).map(Value::Boolean)
    }

    /// An error escaping a row population tears the page down. On a card the move reads as
    /// "not open"; on a list it raises the error itself (corpus
    /// autoformat/TestAutoFormatExpressionTiming.al, RowFormat_List_LastOntoAFailingRowBeyondTheWindow).
    fn torn_down_on_error<T>(&mut self, st: &St, r: Result<T, Signal>) -> Result<T, Signal> {
        match r {
            Err(Signal::Error(e)) => {
                st.borrow_mut().open = false;
                if self.is_list(st) {
                    Err(Signal::Error(e))
                } else {
                    Err(error(NOT_OPEN))
                }
            }
            other => other,
        }
    }

    fn page_move_inner(&mut self, st: &St, how: &str) -> Result<bool, Signal> {
        let rec = self.page_rec(st);
        let cursor = st.borrow().cursor;
        self.ret_used = true;
        let found = |v: Value| match v {
            Value::Integer(n) => n != 0,
            Value::Boolean(b) => b,
            _ => false,
        };
        let landed = match how {
            "First" => found(self.record_op("FindFirst", &rec, &mut [])?),
            "Last" => found(self.record_op("FindLast", &rec, &mut [])?),
            "Next" => match cursor {
                Cursor::Row => {
                    if found(self.record_op("Next", &rec, &mut [Value::Integer(1)])?) {
                        true
                    } else if st.borrow().new_line {
                        // corpus handlers/TestPageNewRowLine_Tests.al: Next() past the last data
                        // row lands on the blank line and answers true.
                        self.land_on_new_line(st)?;
                        return Ok(true);
                    } else {
                        false
                    }
                }
                _ => false,
            },
            "Previous" => match cursor {
                Cursor::NewLine => found(self.record_op("FindLast", &rec, &mut [])?),
                Cursor::Row => found(self.record_op("Next", &rec, &mut [Value::Integer(-1)])?),
                Cursor::None => false,
            },
            _ => unreachable!(),
        };
        if landed {
            st.borrow_mut().cursor = Cursor::Row;
            self.arrive(st)?;
        }
        Ok(landed)
    }

    /// GoToKey / GoToRecord: a forward scan from the first row of the view, populating every row it
    /// passes, so a row whose population fails stops the search there (corpus
    /// autoformat/TestAutoFormatExpressionTiming.al, RowFormat_GoToKeyPastTheFailingRow_ReadsAsTheTestPageNotOpen).
    /// A key no row has leaves the page on the row it stood on
    /// (corpus handlers/TestPageGoToRecordNonKeyRefresh_Tests.al:1-15).
    fn page_go_to_key(&mut self, st: &St, key: &mut [Value]) -> Result<Value, Signal> {
        let rec = self.page_rec(st);
        let table = self.table(&rec)?;
        let want: Vec<Value> = table.keys[0]
            .iter()
            .enumerate()
            .map(|(n, &f)| match key.get(n) {
                Some(v) => ops::convert(v.clone(), &table.fields[f].ty),
                None => Ok(self.default_value(&table.fields[f].ty)),
            })
            .collect::<Result<_, _>>()?;
        let before_fields = rec.borrow().fields.clone();
        let (before_cursor, before_draft) = (st.borrow().cursor, st.borrow().draft);
        self.ret_used = true;
        let mut at = self.record_op("FindFirst", &rec, &mut [])?;
        let mut found = false;
        while matches!(at, Value::Boolean(true)) {
            st.borrow_mut().cursor = Cursor::Row;
            let r = self.arrive(st);
            self.torn_down_on_error(st, r)?;
            let here = self.row_key(&rec)?;
            if here.len() == want.len()
                && here
                    .iter()
                    .zip(&want)
                    .all(|(a, b)| ops::compare(a, b).map(|o| o.is_eq()).unwrap_or(false))
            {
                found = true;
                break;
            }
            self.ret_used = true;
            at = self.record_op("Next", &rec, &mut [Value::Integer(1)])?;
            at = Value::Boolean(matches!(at, Value::Integer(n) if n != 0));
        }
        if !found {
            rec.borrow_mut().fields = before_fields;
            let mut s = st.borrow_mut();
            s.cursor = before_cursor;
            s.draft = before_draft;
        }
        Ok(Value::Boolean(found))
    }

    // ---- controls -----------------------------------------------------------------------

    fn test_control_op(&mut self, method: &str, c: &ControlRef, args: &mut [Value]) -> Result<Value, Signal> {
        self.require_open(&c.page)?;
        let st = &c.page;
        let m = method.split_once('.').map(|(_, m)| m).unwrap_or(method);
        let ControlTarget::Control(idx) = c.target else {
            return unsupported(format!("{method} on a built-in control"));
        };
        match m {
            "Value" => {
                if self.shows_no_row(st, idx) {
                    return Ok(Value::Text(String::new()));
                }
                let v = self.control_value(st, idx)?;
                let text = self.control_text(st, idx, &v)?;
                Ok(Value::Text(text))
            }
            "AsInteger" => self.control_as(st, idx, &RtType::Integer),
            "AsDecimal" => self.control_as(st, idx, &RtType::Decimal),
            "AsBoolean" => self.control_as(st, idx, &RtType::Boolean),
            "AsDate" => self.control_as(st, idx, &RtType::Date),
            "AsTime" => self.control_as(st, idx, &RtType::Time),
            "AsDateTime" => self.control_as(st, idx, &RtType::DateTime),
            "AssertEquals" => self.control_assert_equals(st, idx, args),
            "Caption" => Ok(Value::Text(self.control_caption(st, idx)?)),
            "Editable" => Ok(Value::Boolean(self.control_editable(st, idx)?)),
            "Enabled" => Ok(Value::Boolean(self.control_enabled(st, idx)?)),
            "Visible" => Ok(Value::Boolean(self.control_visible(st, idx)?)),
            "Invoke" => self.action_invoke(st, idx),
            other => unsupported(format!("TestField.{other}")),
        }
    }

    /// `TestAction.Invoke`: runs the action's OnAction when it is enabled; a disabled action
    /// does nothing and raises nothing (corpus handlers/TestPageActionEnabledSourceField_Tests.al,
    /// TPAE_ActionEnabledBoolField_FalseRow_IsDisabledAndItsTriggerIsSkipped).
    fn action_invoke(&mut self, st: &St, idx: usize) -> Result<Value, Signal> {
        if !self.control_enabled(st, idx)? {
            return Ok(Value::Null);
        }
        let engine = self.engine();
        if let Some(r) = page_of(&engine, st).controls[idx].trigger("onaction") {
            self.page_routine(st, r)?;
            self.show_row_page_code_positioned(st)?;
        }
        Ok(Value::Null)
    }

    /// A page that showed no row shows one again once page code has inserted a row the view
    /// admits and left Rec on it; code that only changes the buffer, sets a key it never
    /// inserts, or reads a row the view hides shows nothing (corpus
    /// testpart/BnrBlankPart.al:1-30, NoRow_Action_* arms).
    fn show_row_page_code_positioned(&mut self, st: &St) -> Result<(), Signal> {
        if !st.borrow().open || st.borrow().cursor != Cursor::None {
            return Ok(());
        }
        let rec = self.page_rec(st);
        let table = self.table(&rec)?;
        let probe = Rc::new(RefCell::new(rec.borrow().clone()));
        let pk = table.primary_key(&rec.borrow().fields);
        for (n, &f) in table.keys[0].iter().enumerate() {
            let mut p = probe.borrow_mut();
            p.filters.retain(|x| x.field != f);
            p.filters.push(al_builtins::filter::FieldFilter {
                field: f,
                expr: al_builtins::filter::FilterExpr::Range(Some(pk[n].clone()), Some(pk[n].clone())),
            });
        }
        self.ret_used = true;
        if matches!(self.record_op("FindFirst", &probe, &mut [])?, Value::Boolean(true)) {
            let fields = probe.borrow().fields.clone();
            rec.borrow_mut().fields = fields;
            st.borrow_mut().cursor = Cursor::Row;
            let r = self.arrive(st);
            self.torn_down_on_error(st, r)?;
        }
        Ok(())
    }

    /// The value a control shows: its binding, read now.
    fn control_value(&mut self, st: &St, idx: usize) -> Result<Value, Signal> {
        let engine = self.engine();
        match page_of(&engine, st).controls[idx].binding.clone() {
            ir::Binding::Field(pos) => Ok(self.page_rec(st).borrow().fields[pos].clone()),
            ir::Binding::Var(slot) => {
                let inst = st.borrow().instance.clone().expect("open page");
                let v = inst.borrow().globals[slot].clone();
                Ok(v)
            }
            ir::Binding::Expr(r) => self.page_routine(st, r),
            ir::Binding::None => unsupported("a control without a source"),
        }
    }

    fn control_as(&mut self, st: &St, idx: usize, to: &RtType) -> Result<Value, Signal> {
        if self.shows_no_row(st, idx) {
            return Ok(Value::default_for(to));
        }
        let v = self.control_value(st, idx)?;
        ops::convert(v, to)
    }

    /// A control bound to the page's record reads blank when the page shows no row: whatever its
    /// type, and on a field with an InitValue too (corpus testpart/BnrBlankPart.al:1-30).
    fn shows_no_row(&self, st: &St, idx: usize) -> bool {
        let engine = self.engine();
        st.borrow().cursor == Cursor::None
            && matches!(page_of(&engine, st).controls[idx].binding, ir::Binding::Field(_))
    }

    /// The source field of a control bound to a record field.
    fn control_field(&self, st: &St, idx: usize) -> Option<ir::Field> {
        let engine = self.engine();
        let page = page_of(&engine, st);
        let ir::Binding::Field(pos) = page.controls[idx].binding else {
            return None;
        };
        let table = self.page_rec(st).borrow().table as usize;
        engine.program.objects[table]
            .table
            .as_ref()
            .map(|t| t.fields[pos].clone())
    }

    /// How a control spells its value: the control's own properties, then the source field's.
    fn control_format(&mut self, st: &St, idx: usize) -> Result<ControlFormat, Signal> {
        let engine = self.engine();
        let page = page_of(&engine, st);
        let ctl = &page.controls[idx];
        let field = self.control_field(st, idx);
        let from_field = |n: &str| {
            field
                .as_ref()
                .and_then(|f| f.props.iter().find(|(k, _)| k == n).map(|(_, v)| v.clone()))
        };
        let lit = |n: &str| match ctl.prop(n) {
            Some(ir::PropVal::Lit(v)) => Some(v.clone()),
            _ => None,
        };
        let mut f = ControlFormat {
            decimal_places: lit("decimalplaces")
                .or_else(|| from_field("decimalplaces"))
                .and_then(|p| pagefmt::parse_decimal_places(&p)),
            ..Default::default()
        };
        // An auto-format expression carries a precision: `<Precision,4:4><Standard Format,0>`
        // (corpus autoformat/TestAutoFormat.al, TestAutoFormatExpressionTiming.al).
        if f.decimal_places.is_none() {
            let expr = match ctl.prop("autoformatexpression") {
                Some(ir::PropVal::Lit(v)) => Some(unquote(v)),
                Some(ir::PropVal::Routine(r)) => match self.page_routine(st, *r)? {
                    Value::Text(t) | Value::Code(t) => Some(t),
                    _ => None,
                },
                None => None,
            };
            f.decimal_places = expr.and_then(|e| pagefmt::parse_precision(&e));
        }
        f.blank_zero = lit("blankzero")
            .or_else(|| from_field("blankzero"))
            .is_some_and(|v| is_true(&v));
        if let Some(b) = lit("blanknumbers").or_else(|| from_field("blanknumbers")) {
            f.blank = Blank::parse(&b).unwrap_or_default();
        }
        f.option_captions = lit("optioncaption")
            .or_else(|| from_field("optioncaption"))
            .map(|c| unquote(&c).split(',').map(str::to_string).collect());
        Ok(f)
    }

    fn control_text(&mut self, st: &St, idx: usize, v: &Value) -> Result<String, Signal> {
        if let Value::Library(name, _) = v {
            return unsupported(format!("control text of a {name} value"));
        }
        let f = self.control_format(st, idx)?;
        let engine = self.engine();
        let caption = move |id: u32, ordinal: i32| {
            let o = &engine.program.objects[id as usize];
            o.enum_captions
                .iter()
                .find(|(v, _)| *v == ordinal)
                .or_else(|| o.enum_values.iter().find(|(v, _)| *v == ordinal))
                .map(|(_, c)| c.clone())
                .unwrap_or_else(|| ordinal.to_string())
        };
        Ok(pagefmt::control_text(v, &f, &caption))
    }

    /// The type of the value a control holds.
    fn control_type(&self, st: &St, idx: usize, v: &Value) -> Option<RtType> {
        if let Some(f) = self.control_field(st, idx) {
            return Some(f.ty);
        }
        let engine = self.engine();
        if let ir::Binding::Var(slot) = page_of(&engine, st).controls[idx].binding {
            let object = st.borrow().object as usize;
            return Some(engine.program.objects[object].globals[slot].ty.clone());
        }
        let _ = v;
        None
    }

    /// corpus handlers/TestPageDecimalAssertEquals_Tests.al:85, TestPageVariableControlErrorName.al:89:
    /// `AssertEquals for Field: <control> Expected = '<expected>', Actual = '<actual>'`.
    fn control_assert_equals(&mut self, st: &St, idx: usize, args: &mut [Value]) -> Result<Value, Signal> {
        let actual = self.control_value(st, idx)?;
        let actual_text = if self.shows_no_row(st, idx) {
            String::new()
        } else {
            self.control_text(st, idx, &actual)?
        };
        let expected = args.first().cloned().unwrap_or(Value::Null);
        // A text is compared as written; any other value is converted to the field's type and
        // spelled through the control (corpus handlers/TestPageFieldFormats_Tests.al:1-40).
        let expected_text = match &expected {
            Value::Text(t) | Value::Code(t) => t.clone(),
            other => {
                let converted = match self.control_type(st, idx, &actual) {
                    Some(rt) => ops::convert(other.clone(), &rt)?,
                    None => other.clone(),
                };
                self.control_text(st, idx, &converted)?
            }
        };
        if expected_text != actual_text {
            let engine = self.engine();
            let name = page_of(&engine, st).controls[idx].name.clone();
            return Err(error(format!(
                "AssertEquals for Field: {name} Expected = '{expected_text}', Actual = '{actual_text}'"
            )));
        }
        Ok(Value::Null)
    }

    /// The control's caption: its own `Caption`, else the source field's, else its name.
    fn control_caption(&mut self, st: &St, idx: usize) -> Result<String, Signal> {
        let engine = self.engine();
        let ctl = &page_of(&engine, st).controls[idx];
        // CaptionClass `3,<text>` is a caption the platform resolves to <text>
        // (corpus autoformat/TestAutoFormatExpressionTiming.al, WorkingCaptionClass_OpensAndReadsItsCaption).
        let class = match ctl.prop("captionclass") {
            Some(ir::PropVal::Lit(c)) => Some(unquote(c)),
            Some(ir::PropVal::Routine(r)) => match self.page_routine(st, *r)? {
                Value::Text(t) | Value::Code(t) => Some(t),
                _ => None,
            },
            None => None,
        };
        if let Some(c) = class {
            return match c.split_once(',') {
                Some(("3", text)) => Ok(text.to_string()),
                _ => unsupported(format!("CaptionClass `{c}`")),
            };
        }
        if let Some(ir::PropVal::Lit(c)) = ctl.prop("caption") {
            return Ok(unquote(c));
        }
        if let Some(f) = self.control_field(st, idx) {
            if let Some((_, c)) = f.props.iter().find(|(k, _)| k == "caption") {
                return Ok(unquote(c));
            }
            return Ok(f.name);
        }
        Ok(ctl.name.clone())
    }

    /// A boolean control property, evaluated now: a literal, a routine, or `default`.
    fn prop_bool_now(&mut self, st: &St, idx: usize, name: &str, default: bool) -> Result<bool, Signal> {
        let engine = self.engine();
        match page_of(&engine, st).controls[idx].prop(name) {
            None => Ok(default),
            Some(ir::PropVal::Lit(v)) => Ok(is_true(v)),
            Some(ir::PropVal::Routine(r)) => ops::truthy(&self.page_routine(st, *r)?),
        }
    }

    fn page_editable(&self, st: &St) -> bool {
        let engine = self.engine();
        let object = st.borrow().object as usize;
        let props = &engine.program.objects[object].properties;
        st.borrow().mode != PageMode::View
            && !props
                .iter()
                .any(|(k, v)| k.eq_ignore_ascii_case("Editable") && v.trim().eq_ignore_ascii_case("false"))
    }

    /// Editable and Enabled follow the page state live (corpus
    /// handlers/TestPageControlPropertyLiveness_Tests.al:73, TestPageEditable_Tests.al).
    fn control_editable(&mut self, st: &St, idx: usize) -> Result<bool, Signal> {
        Ok(self.page_editable(st) && self.prop_bool_now(st, idx, "editable", true)?)
    }

    fn control_enabled(&mut self, st: &St, idx: usize) -> Result<bool, Signal> {
        self.prop_bool_now(st, idx, "enabled", true)
    }

    fn control_visible(&mut self, st: &St, idx: usize) -> Result<bool, Signal> {
        let engine = self.engine();
        let page = page_of(&engine, st);
        // Own Visible is the open-time value; a group's is live and so are the groups around the
        // control (corpus handlers/TestPageControlPropertyLiveness_Tests.al:56).
        let own = if page.controls[idx].kind == ir::ControlKind::Group {
            self.prop_bool_now(st, idx, "visible", true)?
        } else {
            st.borrow().frozen_visible.get(idx).copied().unwrap_or(true)
        };
        let mut visible = own;
        let mut parent = page.controls[idx].parent;
        while let Some(p) = parent {
            visible &= self.prop_bool_now(st, p, "visible", true)?;
            parent = page.controls[p].parent;
        }
        Ok(visible)
    }
}
