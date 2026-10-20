//! Page parts (ADR-012 follow-up): a `part(...)` control of an open page owns a child page state
//! (the `TestPart` handle is that child's `Value::TestPage`). The child opens right after the
//! host's OnOpenPage, and its rows follow the host's current row through `SubPageLink`.

use super::*;
use al_builtins::filter::{FieldFilter, FilterExpr};

/// The filter group `SubPageLink` filters live in
/// (corpus handlers/TestPagePartLinkFilterGroup.al: `FilterGroup(4)` answers the link).
pub(super) const LINK_GROUP: i32 = 4;

/// Which parts [`Session::sync_parts_with`] re-points, and how they read.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Sync {
    /// The host opens: hidden parts read too.
    Open,
    /// Every visible part, first to last.
    Plain,
    /// The host moved with First, Last, Next or Previous: only the linked parts, hidden ones included.
    Move { backward: bool },
}

impl Session {
    /// Sets `open = false` on `st` and on the pages it shows as parts.
    pub(super) fn mark_closed(&mut self, st: &St) {
        st.borrow_mut().open = false;
        let parts = std::mem::take(&mut st.borrow_mut().parts);
        for (_, child) in parts {
            self.mark_closed(&child);
        }
    }

    /// Creates the page of every part control, before the host's own triggers run: the host's
    /// OnInit and OnOpenPage reach it through `CurrPage.<Part>.Page` (corpus
    /// handlers/TestPageTempPart_Host.al: OnOpenPage hands rows to the part page). A part whose
    /// `Visible` is the literal false is not in the control tree
    /// (testpart/TestTestPart.al:TestPart_InvisiblePart_IsNotInTheControlTreeAtAll).
    pub(super) fn create_parts(&mut self, st: &St) -> Result<(), Signal> {
        let engine = self.engine();
        let page = page_of(&engine, st);
        for (i, c) in page.controls.iter().enumerate() {
            if c.kind != ir::ControlKind::Part || self.control_eliminated(st, i) || self.area_removed(st, i) {
                continue;
            }
            let Some(part_page) = c.part_page else {
                return unsupported(format!("part {} shows no page", c.name));
            };
            // A part page the host's code reached before the host ran (see `part_page`, pinned by pages/lt-part-never-ran) is the
            // one the page opens with: what that call set on it stays (Warehouse Pick Summary.SetRecords hands the summary
            // rows to its part before the page runs).
            let early = {
                let mut s = st.borrow_mut();
                let at = s
                    .hidden_parts
                    .iter()
                    .position(|(c, p)| *c == i && !p.borrow().open && !p.borrow().ran);
                at.map(|at| s.hidden_parts.remove(at).1)
            };
            if let Some(child) = early {
                st.borrow_mut().parts.push((i, child));
                continue;
            }
            let child = Rc::new(RefCell::new(PageState::closed(part_page as u32)));
            child.borrow_mut().host = Some((Rc::downgrade(st), i));
            self.page_instance(&child)?;
            st.borrow_mut().parts.push((i, child));
        }
        Ok(())
    }

    fn init_one_part(&mut self, child: &St) -> Result<(), Signal> {
        if !std::mem::replace(&mut child.borrow_mut().inited, true) {
            self.page_trigger(child, "oninit", vec![])?;
        }
        Ok(())
    }

    /// Runs the OnInit of every part page that has not run it yet.
    pub(super) fn init_parts(&mut self, st: &St) -> Result<(), Signal> {
        let parts = st.borrow().parts.clone();
        for (_, child) in parts {
            if !std::mem::replace(&mut child.borrow_mut().inited, true) {
                self.page_trigger(&child, "oninit", vec![])?;
            }
        }
        Ok(())
    }

    /// Opens the parts right after the host's OnOpenPage: the ones whose `Visible` came out false
    /// are dropped, the others run their OnInit and OnOpenPage.
    pub(super) fn open_parts(&mut self, st: &St) -> Result<(), Signal> {
        // A part is opened inside its host, so the host's read-only state reaches it (corpus
        // handlers/TestPageNewRowLine_Tests.al:ReadOnlyHostPart_NextPastLastDataRow_ReturnsFalse).
        let mode = if self.page_editable(st) {
            PageMode::Edit
        } else {
            PageMode::View
        };
        let parts = st.borrow().parts.clone();
        let mut kept = Vec::new();
        let mut hidden = Vec::new();
        for (i, child) in parts {
            if self.control_visible(st, i)? {
                kept.push((i, child));
            } else {
                hidden.push((i, child));
            }
        }
        st.borrow_mut().parts = kept.clone();
        st.borrow_mut().hidden_parts = hidden.clone();
        for (_, child) in kept.into_iter().chain(hidden) {
            self.open_page_as(&child, mode, None, None, true)?;
        }
        Ok(())
    }

    /// Sets the mode of the pages `st` shows as parts, and of theirs.
    pub(super) fn set_part_modes(&mut self, st: &St, mode: PageMode) {
        let parts = st.borrow().parts.clone();
        for (_, child) in parts {
            child.borrow_mut().mode = mode;
            self.set_part_modes(&child, mode);
        }
    }

    /// Whether the part control that shows `st` says `Editable = false`. What that does to the
    /// part's rows is pinned only for SetValue, which writes; New() and Next() onto the new-row line
    /// of such a part are refused.
    pub(super) fn part_control_read_only(&self, st: &St) -> bool {
        let Some((host, idx)) = st.borrow().host.clone() else {
            return false;
        };
        let Some(host) = host.upgrade() else { return false };
        let engine = self.engine();
        matches!(
            page_of(&engine, &host).controls[idx].prop("editable"),
            Some(ir::PropVal::Lit(v)) if v.trim().eq_ignore_ascii_case("false")
        )
    }

    /// What the part controls that show `st` (its host's, and theirs up the chain) say about `Editable()` (`editable`
    /// true) or `Enabled()` of a field of `st`. `Some(false)`: the answer is No. `None`: the part controls say nothing, the
    /// field answers by itself. Pinned only for a field read through a part whose control says the literal `Editable =
    /// false`: Editable() No (misc/lt-probes-needed-7/PnGPartRows.al:117 ReadOnlyPartSetValue `ro-descr-editable`; a part a page
    /// extension adds, pageext/lt-u-page-props/UPageProps.al:239-240 PageExt_PartStates `ext_part_row_*_editable`).
    /// An Editable or Enabled expression that comes out true counts as no property (assumption
    /// part-field-under-true-part-expression). Everything else a part control's Editable or Enabled can do to its fields
    /// is refused (hard rule 4): an expression that comes out false (Tests-ERM ERM Sales Quotes BlankSalesQuoteLinesUneditable
    /// reads a field behind `Editable = IsSalesLinesEditable` with the flag false), `Enabled = false`, Enabled() behind
    /// `Editable = false`, and `Editable = false` on a part control further up the chain.
    pub(super) fn part_field_gate(&mut self, st: &St, editable: bool) -> Result<Option<bool>, Signal> {
        let what = if editable { "Editable()" } else { "Enabled()" };
        let engine = self.engine();
        let mut answer = None;
        let mut at = st.borrow().host.clone();
        let mut direct = true;
        while let Some((host, idx)) = at {
            let Some(host) = host.upgrade() else { break };
            for name in ["enabled", "editable"] {
                let shown = if name == "enabled" { "Enabled" } else { "Editable" };
                match page_of(&engine, &host).controls[idx].prop(name) {
                    None => {}
                    Some(ir::PropVal::Lit(v)) => {
                        let v = is_true(v);
                        if !v && name == "editable" && editable && direct {
                            answer = Some(false);
                            continue;
                        }
                        if !v {
                            return unsupported(format!(
                                "{what} of a field on a part whose control says {shown} = false (not pinned)"
                            ));
                        }
                    }
                    // An expression that comes out true is taken as no property (assumption
                    // part-field-under-true-part-expression); one that comes out false is not pinned.
                    Some(_) => {
                        if !self.prop_bool_now(&host, idx, name, true)? {
                            return unsupported(format!(
                                "{what} of a field on a part whose control has an {shown} expression that is false (not pinned)"
                            ));
                        }
                        al_builtins::assumptions::assume(
                            al_builtins::assumptions::Assumption::PartFieldUnderTruePartExpression,
                        );
                    }
                }
            }
            direct = false;
            at = host.borrow().host.clone();
        }
        Ok(answer)
    }

    pub(super) fn refuse_read_only_part(&self, st: &St, what: &str) -> Result<(), Signal> {
        if self.part_control_read_only(st) {
            return unsupported(format!(
                "{what} through a part whose control says Editable = false (not pinned)"
            ));
        }
        Ok(())
    }

    /// The `SubPageView` of the part control that shows `st`.
    pub(super) fn sub_page_view(&self, st: &St) -> Option<String> {
        let (host, idx) = st.borrow().host.clone()?;
        let host = host.upgrade()?;
        let engine = self.engine();
        page_of(&engine, &host).controls[idx].part_link.as_ref()?.view.clone()
    }

    /// The page shown by part control `control` of `st`.
    pub(super) fn part_page(&mut self, st: &St, control: usize, runtime: bool) -> Result<St, Signal> {
        // A Page variable's procedure that reaches a part before the page ever ran (Check Credit Limit.PopulateDataOnNotification,
        // which Cust-Check Cr. Limit calls on a Page variable to fill a notification) gets the part page created and not opened, as
        // a hidden part is (pages/lt-part-never-ran/NeverRanReadRec.al, NeverRanUpdate.al, NeverRanGetRecord.al, NeverRanSetRecord.al); after the page ran and closed it stays refused.
        let never_ran = runtime && !st.borrow().open && !st.borrow().ran;
        if never_ran {
        } else {
            match self.require_open(st) {
                // The page's own code reaching a part after the page ran and closed is not a TestPage the test closed (Posted Purchase
                // Document Lines.CopyLineToDoc reads `CurrPage.PostedRcpts.PAGE` after its RunModal returned): refused, as the other
                // calls of such a page are (see test_page_op).
                Err(Signal::Error(e)) if runtime && e.code == "TestPageNotOpened" => {
                    return unsupported(
                        "a part of a page that is not open, reached by the code of that page (what it does then is not pinned)",
                    );
                }
                r => r?,
            }
        }
        let found = st
            .borrow()
            .parts
            .iter()
            .find(|(i, _)| *i == control)
            .map(|(_, c)| c.clone());
        // The page's own code reaches a part whose Visible came out false (Base Application's journals call
        // `CurrPage.WorkflowStatusBatch.PAGE.SetFilterOnWorkflowRecord` behind `Visible = <variable>`; Tests-General
        // Journal, 7 tests; corpus pages/lt-hidden-part-currpage/HiddenPartCurrPageEditable.al).
        // A part whose Visible expression came out false is in the test page too, answering Visible() No and First() Yes
        // (misc/lt-probes-needed-7/PnGPartTriggers.al:HiddenPartExpression); only a literal `Visible = false` leaves it out
        // (testpart/TestPartNotOnThePage, which never creates the part).
        let found = found.or_else(|| {
            // Test code finds it only when the Visible expression hid it; a part a literal `Visible = false` left out, which page
            // code created on first reach, stays "not found" (misc/lt-probes-needed-9/P99InvisiblePart.al:P99_StaticInvisible).
            st.borrow()
                .hidden_parts
                .iter()
                .find(|(i, _)| *i == control)
                .map(|(_, c)| c.clone())
                // A part the session's application areas removed is not on the test page either (types/lt-conversions-hidden-parts-doctype/
                // P142HiddenPartArea.al:P142_ServiceAreaPartRemoved: `The part with ID = n was not found on the page.`).
                .filter(|_| runtime || !(self.control_eliminated(st, control) || self.area_removed(st, control)))
        });
        // A part the control tree dropped (Visible = false, or an ApplicationArea the session does not enable) is
        // still there for the page's own code: Base Application's Sales Credit Memo calls
        // `CurrPage.IncomingDocAttachFactBox.Page.LoadDataFromRecord(Rec)` in OnAfterGetCurrRecord. It is created on the first such reach, not opened, and kept out of the tree (corpus pages/lt-hidden-part-currpage/HiddenPartCurrPageEditable.al: Editable_LiteralInvisible_OpenEdit, Editable_ExprInvisible_OpenEdit).
        let found = match found {
            None if runtime => {
                let engine = self.engine();
                let part_page = page_of(&engine, st).controls[control].part_page;
                match part_page {
                    Some(pp) => {
                        let child = Rc::new(RefCell::new(PageState::closed(pp as u32)));
                        child.borrow_mut().host = Some((Rc::downgrade(st), control));
                        self.page_instance(&child)?;
                        st.borrow_mut().hidden_parts.push((control, child.clone()));
                        self.init_one_part(&child)?;
                        // It is not opened: its OnOpenPage and OnAfterGetRecord do not run, only the call that reached it
                        // (pages/lt-page-filtergroup-assumptions/P99InvisiblePart.al:P99_StaticInvisible, the log is
                        // `host-OnAfterGetCurrRecord | part-Ping | host-after-Ping`).
                        Some(child)
                    }
                    None => None,
                }
            }
            other => other,
        };
        match found {
            Some(c) => Ok(c),
            None => {
                // Text and code: testpart/TestPartTriggerOrderAndRefresh.al:316-340. The id is
                // `control_id` of the declaring object and the part's name (testpart/TestTestPart.al:233).
                let engine = self.engine();
                let ctl = &page_of(&engine, st).controls[control];
                let declaring = self.object_id_of(ctl.owner.unwrap_or(st.borrow().object as usize) as u32);
                Err(error_code(
                    format!(
                        "The part with ID = {} was not found on the page.",
                        control_id(declaring, &ctl.name)
                    ),
                    "TestPartNotFound",
                ))
            }
        }
    }

    /// Reads the link of every part off the host's current row and loads the parts' first rows.
    /// Providers go first: a part linked through `Provider` reads the provider's current row.
    pub(super) fn sync_parts(&mut self, st: &St) -> Result<(), Signal> {
        self.sync_parts_with(st, Sync::Plain)
    }

    /// [`Self::sync_parts`] as the host opens: the parts hidden by a Visible expression read their rows too, in the order of the
    /// controls (pages/lt-page-filtergroup-assumptions/P99InvisiblePart.al:P99_FlagFalseAtOpen).
    pub(super) fn sync_parts_at_open(&mut self, st: &St) -> Result<(), Signal> {
        self.sync_parts_with(st, Sync::Open)
    }

    /// [`Self::sync_parts`] after the host moved to another row with First, Last, Next or Previous. Only the parts with a
    /// SubPageLink read their rows again; a part without one keeps what it shows and runs no trigger
    /// (pages/lt-part-reads/PartReadsOpen.al:269-283 and :336-379, `-Plain` and `-View` empty;
    /// pages/lt-part-subpageview-filter/PartViewFilter.al:381 `next-GtOnly`; types/lt-conversions-hidden-parts-doctype/
    /// P142HiddenPartArea.al:386, :420). A linked part hidden by a Visible expression reads too (PartReadsOpen.al `-Hidden`).
    /// After Previous and First the linked parts read their rows last to first, once each (PartReadsOpen.al:358-379: 1, 2
    /// and 3 rows; PartViewFilter.al:387-389); after Next and Last first to last, then the first row once more.
    pub(super) fn sync_parts_after_move(&mut self, st: &St, backward: bool) -> Result<(), Signal> {
        self.sync_parts_with(st, Sync::Move { backward })
    }

    fn sync_parts_with(&mut self, st: &St, how: Sync) -> Result<(), Signal> {
        let engine = self.engine();
        let page = page_of(&engine, st);
        let opening = how == Sync::Open;
        let mut parts = st.borrow().parts.clone();
        if opening || matches!(how, Sync::Move { .. }) {
            let hidden = st.borrow().hidden_parts.clone();
            parts.extend(
                hidden
                    .into_iter()
                    .filter(|(i, c)| !self.control_eliminated(st, *i) && (opening || c.borrow().open)),
            );
            parts.sort_by_key(|(i, _)| *i);
        }
        if let Sync::Move { .. } = how {
            let mut linked = Vec::new();
            for (i, child) in parts {
                let entries = page.controls[i].part_link.as_ref().map(|l| &l.entries[..]).unwrap_or(&[]);
                if entries.is_empty() {
                    continue;
                }
                // A part linked by `const` and `filter` terms only reads nothing off the host's row; whether it reads its rows
                // again after a move is not pinned.
                let reads_a_field = entries.iter().any(|e| matches!(e, ir::LinkEntry::Field(..)));
                if !reads_a_field && child.borrow().open {
                    return unsupported(
                        "a host move with a part linked by const or filter terms only (whether it reads its rows again is not pinned)",
                    );
                }
                linked.push((i, child));
            }
            parts = linked;
        }
        let has_provider = |i: usize| {
            page.controls[i]
                .part_link
                .as_ref()
                .is_some_and(|l| l.provider.is_some())
        };
        for pass in [false, true] {
            for (i, child) in &parts {
                if has_provider(*i) == pass {
                    if opening && self.view_names_link_field(st, *i, child)? {
                        child.borrow_mut().read_backwards = Some(al_builtins::value::ReadBackwards::AtOpen);
                    }
                    if let Sync::Move { backward: true } = how {
                        // The pinned parts walk the table; a part with its own OnFindRecord or OnNextRecord is not pinned.
                        if self.declares(child, "onfindrecord") || self.declares(child, "onnextrecord") {
                            return unsupported(
                                "a host moved back with a linked part that declares OnFindRecord or OnNextRecord (reads not pinned)",
                            );
                        }
                        child.borrow_mut().read_backwards = Some(al_builtins::value::ReadBackwards::AfterMove);
                    }
                    self.sync_part(st, *i, child)?;
                    // A part that found no row to fill does not carry the order to a later fill.
                    child.borrow_mut().read_backwards = None;
                }
            }
        }
        Ok(())
    }

    /// The open parts of `st` (shown, and hidden by a Visible expression) whose `CurrPage.Update` waits for its refresh.
    pub(super) fn parts_waiting_refresh(&self, st: &St) -> Vec<(usize, St)> {
        let s = st.borrow();
        s.parts
            .iter()
            .chain(s.hidden_parts.iter())
            .filter(|(_, c)| c.borrow().open && c.borrow().refresh)
            .cloned()
            .collect()
    }

    /// The host's own code (its OnAfterGetCurrRecord as it opens or moves) called a procedure of a part that ran
    /// `CurrPage.Update`: once the host's parts have followed its row, such a part fills its rows again from the first, as
    /// it does when it opens (pages/lt-hidden-part-currpage/HiddenPartCurrPageUpdateFalse.al:196-198 and
    /// HiddenPartCurrPageUpdateTrue.al:196-198: `part-OnAfterGetRecord:1 | :2 | :1` after `host-end`, twice at open, once
    /// after Next). `before` are the parts that waited already before the host's code ran. Pinned for a part without a
    /// SubPageLink hidden by a Visible expression, standing on no row typed into; any other part is refused.
    /// `pinned` is false for a host move other than Next: such a refresh is refused.
    pub(super) fn refresh_parts_asked_by_host(
        &mut self,
        st: &St,
        before: &[(usize, St)],
        pinned: bool,
    ) -> Result<(), Signal> {
        let asked: Vec<(usize, St)> = self
            .parts_waiting_refresh(st)
            .into_iter()
            .filter(|(i, _)| !before.iter().any(|(b, _)| b == i))
            .collect();
        if asked.is_empty() {
            return Ok(());
        }
        let engine = self.engine();
        let page = page_of(&engine, st);
        for (i, child) in asked {
            let hidden = st.borrow().hidden_parts.iter().any(|(h, _)| *h == i);
            let linked = page.controls[i]
                .part_link
                .as_ref()
                .is_some_and(|l| !l.entries.is_empty() || l.provider.is_some());
            let (draft, dirty) = {
                let c = child.borrow();
                (c.draft, c.dirty)
            };
            let shape_pinned = pinned
                && hidden
                && !linked
                && !draft
                && !dirty
                && !self.declares(&child, "onfindrecord")
                && !self.declares(&child, "onnextrecord");
            if !shape_pinned {
                // Kept, not pinned: before this refresh was pinned such a request was never carried out. Refusing it loses a
                // baseline case (handlers/TestPageActionRunObjectKindsPrecompiled_Tests.al:101: Base Application's
                // Receivables-Payables, whose host updates its shown parts from OnAfterGetCurrRecord), so it stays so.
                continue;
            }
            child.borrow_mut().refresh = false;
            self.sync_part(st, i, &child)?;
        }
        Ok(())
    }

    /// Whether the part control's SubPageView restricts, with a `const` condition, a field its SubPageLink also names.
    /// Such a part reads its rows last to first, once each, when the host opens (pages/lt-part-reads/PartReadsOpen.al:
    /// OpenView_SixParts_ReadsPerPartLogged `open-Same`, 3 rows; misc/lt-probes-needed-7/PnGPartTriggers.al:
    /// OpenOrderOfHostWithSixParts, 2 rows). Only `const` on a linked field is pinned: another condition on a linked field
    /// is refused, since the reads it gives are not known.
    fn view_names_link_field(&self, host: &St, control: usize, child: &St) -> Result<bool, Signal> {
        let engine = self.engine();
        let page = page_of(&engine, host);
        let Some(link) = page.controls[control].part_link.as_ref() else {
            return Ok(false);
        };
        let Some(view) = link.view.as_ref() else {
            return Ok(false);
        };
        if !self.has_source(child) {
            return Ok(false);
        }
        let table = self.table(&self.page_rec(child))?;
        let view = view.to_ascii_lowercase();
        let mut found = false;
        for entry in &link.entries {
            let ir::LinkEntry::Field(pf, _) = entry else { continue };
            let name = table.fields[*pf].name.to_ascii_lowercase();
            for form in [format!("\"{name}\""), name.clone()] {
                let Some(at) = view
                    .find(&format!("{form} ="))
                    .or_else(|| view.find(&format!("{form}=")))
                else {
                    continue;
                };
                let after = view[at + form.len()..]
                    .trim_start()
                    .trim_start_matches('=')
                    .trim_start();
                if !after.starts_with("const(") {
                    return unsupported(format!(
                        "a SubPageView condition other than const on the linked field {name} (not pinned)"
                    ));
                }
                found = true;
                break;
            }
        }
        Ok(found)
    }

    /// A write into part `child` (SetValue, New) while its host stands on a typed row that is not stored yet: the host row is
    /// saved first, and the parts link to it (pages/lt-part-line-before-host-key/PartLineBeforeHostKey.al:406
    /// `HP.OnInsertRecord:D2 | H.OnInsert:D2 | L.OnValidate(Document No.):D2 | ...` for New(); handlers/
    /// TestPageOpenNewPartLink.al:100 OpenNew_NumberTyped_PartWriteCarriesTheNumber for a write).
    pub(super) fn save_unsaved_host_first(&mut self, child: &St) -> Result<(), Signal> {
        let Some(host) = child.borrow().host.as_ref().and_then(|(h, _)| h.upgrade()) else {
            return Ok(());
        };
        let unsaved = {
            let h = host.borrow();
            h.open && h.host.is_none() && h.draft && !h.draft_stored && h.dirty && h.cursor != Cursor::None
        };
        if !unsaved || !self.has_source(&host) {
            return Ok(());
        }
        // A new host whose fields were typed before its key: what BC stores then is not known (pages/
        // lt-new-line-stamps-filters/NewLineStampsFilters.al:264 PartB_OpenNewHeader_TypeThenNo stores the line with a
        // blank link though the key was typed; the rule is not found).
        if host.borrow().typed_before_key {
            return unsupported(
                "a part written into while its new host row had a field typed before its key (what is stored is not pinned)",
            );
        }
        self.save_row(&host)
    }

    /// After part `child` moved, the parts that take their link from it follow.
    pub(super) fn sync_dependents(&mut self, child: &St) -> Result<(), Signal> {
        let Some((host, idx)) = child.borrow().host.clone() else {
            return Ok(());
        };
        let Some(host) = host.upgrade() else { return Ok(()) };
        let engine = self.engine();
        let page = page_of(&engine, &host);
        let parts = host.borrow().parts.clone();
        for (i, sibling) in &parts {
            let provider = page.controls[*i].part_link.as_ref().and_then(|l| l.provider);
            if provider == Some(idx) {
                self.sync_part(&host, *i, sibling)?;
            }
        }
        Ok(())
    }

    /// The page part `control` of `host` reads its link from: the Provider part, else the host.
    fn link_page(&self, host: &St, control: usize) -> Option<St> {
        let engine = self.engine();
        let page = page_of(&engine, host);
        match page.controls[control].part_link.as_ref().and_then(|l| l.provider) {
            Some(pi) => host
                .borrow()
                .parts
                .iter()
                .find(|(i, _)| *i == pi)
                .map(|(_, c)| c.clone()),
            None => Some(host.clone()),
        }
    }

    /// Points part `child` (control `control` of `host`) at the host's current row and loads its
    /// first row.
    fn sync_part(&mut self, host: &St, control: usize, child: &St) -> Result<(), Signal> {
        if !child.borrow().open {
            return Ok(());
        }
        // What was typed into the row the part stands on is saved before it moves on.
        self.save_row(child)?;
        // The filter `SetSelectionFilter(Rec)` left on the part's own record is gone once the part refreshes with its host
        // (HiddenPartSelectionFilter.al:SelectionFilter_VisiblePart_FilterAndCount `rec-before=/3` after Next).
        if std::mem::take(&mut child.borrow_mut().selection_on_rec) {
            self.page_rec(child)
                .borrow_mut()
                .filters
                .retain(|f| f.group == LINK_GROUP);
        }
        let engine = self.engine();
        let page = page_of(&engine, host);
        if let (Some(link), true) = (page.controls[control].part_link.as_ref(), self.has_source(child)) {
            let source = self.link_page(host, control).and_then(|p| {
                self.has_source(&p).then(|| {
                    let t = self.page_rec(&p).borrow().table as usize;
                    (p, t)
                })
            });
            let rec = self.page_rec(child);
            let table = self.table(&rec)?;
            let mut fresh: Vec<FieldFilter> = Vec::new();
            let mut fields = Vec::new();
            for entry in &link.entries {
                match entry {
                    ir::LinkEntry::Field(pf, hf) => {
                        fields.push(*pf);
                        let ty = table.fields[*pf].ty.clone();
                        let Some((src_page, src_table)) = source.as_ref() else {
                            return unsupported("a SubPageLink to a page without a source table");
                        };
                        // The row the link reads, else the type's blank value: a page that shows
                        // no row links to nothing (corpus testpart/TestPartProviderMove.al:
                        // ProviderLinkedPart_HostMovesToAnEmptyProvider_ReadsBlank).
                        // A host row that is not stored yet links to nothing either, also after its key was typed: the
                        // part's blank line takes the blank value and validates it (pages/lt-part-line-before-host-key/
                        // PartLineBeforeHostKey.al:398, :400 `H.OnValidate(No.):D2 | L.OnValidate(Document No.): | ...`).
                        let unsaved_host = {
                            let p = src_page.borrow();
                            p.host.is_none() && p.draft && !p.draft_stored
                        };
                        let value = if src_page.borrow().cursor == Cursor::None || unsaved_host {
                            let host_ty = engine.program.objects[*src_table]
                                .table
                                .as_ref()
                                .map(|t| t.fields[*hf].ty.clone())
                                .ok_or_else(|| Signal::Unsupported("a link to a table without a schema".into()))?;
                            Value::default_for(&host_ty)
                        } else {
                            self.page_rec(src_page).borrow().fields[*hf].clone()
                        };
                        let value = ops::convert(value, &ty)?;
                        fresh.push(FieldFilter {
                            field: *pf,
                            expr: FilterExpr::Range(Some(value.clone()), Some(value)),
                            group: LINK_GROUP,
                        });
                    }
                    ir::LinkEntry::Filter(pf, text) => {
                        if text == "''" {
                            al_builtins::assumptions::assume(
                                al_builtins::assumptions::Assumption::SubPageLinkEmptyTerm,
                            );
                        }
                        fields.push(*pf);
                        let ty = table.fields[*pf].ty.clone();
                        let expr = self
                            .parse_filter_text(text, &ty)
                            .map_err(|e| Signal::Unsupported(format!("SubPageLink filter: {e}")))?;
                        fresh.push(FieldFilter {
                            field: *pf,
                            expr,
                            group: LINK_GROUP,
                        });
                    }
                }
            }
            let mut r = rec.borrow_mut();
            r.filters
                .retain(|f| !(f.group == LINK_GROUP && fields.contains(&f.field)));
            r.filters.extend(fresh);
        }
        {
            let mut s = child.borrow_mut();
            s.cursor = Cursor::None;
            s.draft = false;
            s.dirty = false;
            s.window = Vec::new();
            s.focus = None;
            s.focus_in_row = false;
            s.refused = false;
            s.pending = None;
        }
        let mode = child.borrow().mode;
        self.load_first_row(child, mode, false)
    }

    /// A field the host just changed is read by the link of some parts: they follow the new value
    /// (corpus handlers/TestPageOpenNewPartLink.al:OpenNew_NumberTyped_PartWriteCarriesTheNumber).
    pub(super) fn resync_parts_linked_by(&mut self, st: &St, control: usize) -> Result<(), Signal> {
        if st.borrow().parts.is_empty() || !st.borrow().open {
            return Ok(());
        }
        let engine = self.engine();
        let page = page_of(&engine, st);
        let ir::Binding::Field(pos) = page.controls[control].binding else {
            return Ok(());
        };
        // A key field typed on a host row that is stored (a new row the host inserted when a field outside the key was
        // typed first): BC keeps the parts' blank lines on the old link value (pages/lt-new-line-stamps-filters/
        // NewLineStampsFilters.al:264 PartB_OpenNewHeader_TypeThenNo, the line is stored with a blank "Document No.");
        // the rule is not found, so it is refused.
        let key_of_stored_row = {
            let s = st.borrow();
            !s.draft && s.cursor == Cursor::Row
        } && self.has_source(st)
            && self.table(&self.page_rec(st))?.keys[0].contains(&pos)
            && st.borrow().typed_before_key;
        let parts = st.borrow().parts.clone();
        for (i, child) in &parts {
            if key_of_stored_row
                && page.controls[*i].part_link.as_ref().is_some_and(|l| {
                    l.entries
                        .iter()
                        .any(|e| matches!(e, ir::LinkEntry::Field(_, hf) if *hf == pos))
                })
            {
                return unsupported(
                    "a key field typed on a new host row stored after a field outside its key was typed, with parts linked on it (not pinned)",
                );
            }
            let reads = page.controls[*i].part_link.as_ref().is_some_and(|l| {
                l.provider.is_none()
                    && l.entries
                        .iter()
                        .any(|e| matches!(e, ir::LinkEntry::Field(_, hf) if *hf == pos))
            });
            if reads {
                self.sync_part(st, *i, child)?;
            }
        }
        Ok(())
    }

    /// A part list that shows rows of the host's own table, edited through SetValue on the host's current row: the host card reads that
    /// row's new values and runs its OnAfterGetCurrRecord once more, so its fields show the edit, whatever `UpdatePropagation` says
    /// (pages/lt-page-controls-aa/AaPartsLists.al:AA_UpdatePropagation_Both line 191, _SubPart line 218, _Default line 245). The part
    /// saves its row later, when its cursor leaves it (testpart/TestPartCloseLinkEditableContracts.al:Parts_EnabledReadsTheControl_
    /// EditableFalseDoesNotBlockWrites). Pinned for a host card over the same table, without OnFindRecord; any other pairing keeps the
    /// earlier behavior.
    pub(super) fn refresh_host_after_part_edit(&mut self, st: &St) -> Result<(), Signal> {
        let Some(host) = st.borrow().host.as_ref().and_then(|(h, _)| h.upgrade()) else {
            return Ok(());
        };
        if host.borrow().open
            && self.is_list(&host)
            && self.is_list(st)
            && self.has_source(&host)
            && self.has_source(st)
        {
            let (a, b) = (self.page_rec(&host), self.page_rec(st));
            if self.table(&a)?.object == self.table(&b)?.object {
                return unsupported("a part edit on a list host over the same table (not pinned)");
            }
        }
        if !host.borrow().open
            || self.is_list(&host)
            || !self.is_list(st)
            || !self.has_source(&host)
            || !self.has_source(st)
            || host.borrow().cursor != Cursor::Row
            || host.borrow().draft
            || self.declares(&host, "onfindrecord")
            || self.declares(&host, "onnextrecord")
        {
            return Ok(());
        }
        let (a, b) = (self.page_rec(&host), self.page_rec(st));
        let table = self.table(&a)?;
        if table.object != self.table(&b)?.object {
            return Ok(());
        }
        if !Self::same_key(
            &table.primary_key(&a.borrow().fields),
            &table.primary_key(&b.borrow().fields),
        ) {
            return Ok(());
        }
        let fields = b.borrow().fields.clone();
        a.borrow_mut().fields = fields;
        let r = (|| {
            self.populate_current(&host)?;
            self.page_trigger(&host, "onaftergetcurrrecord", vec![])?;
            self.page_event(&host, "onaftergetcurrrecordevent", None, None)
        })();
        self.torn_down_on_error(&host, r).map(|_| ())
    }

    /// Saves what was typed into the rows the parts stand on.
    pub(super) fn save_parts(&mut self, st: &St) -> Result<(), Signal> {
        let parts = st.borrow().parts.clone();
        for (_, child) in parts {
            if child.borrow().open {
                self.save_row(&child)?;
            }
        }
        Ok(())
    }
}
