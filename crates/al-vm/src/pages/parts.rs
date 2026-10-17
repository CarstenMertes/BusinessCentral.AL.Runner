//! Page parts (ADR-012 follow-up): a `part(...)` control of an open page owns a child page state
//! (the `TestPart` handle is that child's `Value::TestPage`). The child opens right after the
//! host's OnOpenPage, and its rows follow the host's current row through `SubPageLink`.

use super::*;
use al_builtins::filter::{FieldFilter, FilterExpr};

/// The filter group `SubPageLink` filters live in
/// (corpus handlers/TestPagePartLinkFilterGroup.al: `FilterGroup(4)` answers the link).
pub(super) const LINK_GROUP: i32 = 4;

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
                .filter(|_| runtime || !self.control_eliminated(st, control))
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
        self.sync_parts_with(st, false)
    }

    /// [`Self::sync_parts`] as the host opens: the parts hidden by a Visible expression read their rows too, in the order of the
    /// controls (pages/lt-page-filtergroup-assumptions/P99InvisiblePart.al:P99_FlagFalseAtOpen).
    pub(super) fn sync_parts_at_open(&mut self, st: &St) -> Result<(), Signal> {
        self.sync_parts_with(st, true)
    }

    fn sync_parts_with(&mut self, st: &St, opening: bool) -> Result<(), Signal> {
        let engine = self.engine();
        let page = page_of(&engine, st);
        let mut parts = st.borrow().parts.clone();
        if opening {
            let hidden = st.borrow().hidden_parts.clone();
            parts.extend(hidden.into_iter().filter(|(i, _)| !self.control_eliminated(st, *i)));
            parts.sort_by_key(|(i, _)| *i);
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
                    self.sync_part(st, *i, child)?;
                }
            }
        }
        Ok(())
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
                        let value = if src_page.borrow().cursor == Cursor::None {
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
        let parts = st.borrow().parts.clone();
        for (i, child) in &parts {
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
