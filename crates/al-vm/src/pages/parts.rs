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

    /// Opens the page of every part control that is in the control tree: a part whose `Visible` is
    /// false is not (corpus testpart/TestTestPart.al:TestPart_InvisiblePart_IsNotInTheControlTreeAtAll).
    pub(super) fn open_parts(&mut self, st: &St) -> Result<(), Signal> {
        let engine = self.engine();
        let page = page_of(&engine, st);
        // A part is opened inside its host, so the host's read-only state reaches it (corpus
        // handlers/TestPageNewRowLine_Tests.al:ReadOnlyHostPart_NextPastLastDataRow_ReturnsFalse).
        let mode = if self.page_editable(st) {
            PageMode::Edit
        } else {
            PageMode::View
        };
        for (i, c) in page.controls.iter().enumerate() {
            if c.kind != ir::ControlKind::Part {
                continue;
            }
            if !self.control_visible(st, i)? {
                continue;
            }
            let Some(part_page) = c.part_page else {
                return unsupported(format!("part {} shows no page", c.name));
            };
            let child = Rc::new(RefCell::new(PageState::closed(part_page as u32)));
            child.borrow_mut().host = Some((Rc::downgrade(st), i));
            st.borrow_mut().parts.push((i, child.clone()));
            self.open_page_as(&child, mode, None, None, true)?;
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
    pub(super) fn part_page(&self, st: &St, control: usize) -> Result<St, Signal> {
        self.require_open(st)?;
        let found = st
            .borrow()
            .parts
            .iter()
            .find(|(i, _)| *i == control)
            .map(|(_, c)| c.clone());
        match found {
            Some(c) => Ok(c),
            None => {
                // The id is generated per compilation; the corpus asserts only the tail
                // (testpart/TestTestPart.al:TestPart_InvisiblePart_IsNotInTheControlTreeAtAll).
                Err(error("The part with ID = 0 was not found on the page."))
            }
        }
    }

    /// Reads the link of every part off the host's current row and loads the parts' first rows.
    /// Providers go first: a part linked through `Provider` reads the provider's current row.
    pub(super) fn sync_parts(&mut self, st: &St) -> Result<(), Signal> {
        let engine = self.engine();
        let page = page_of(&engine, st);
        let parts = st.borrow().parts.clone();
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

    /// The fields of the row part `control` of `host` reads its link from, `None` while that page
    /// shows no row.
    fn link_source(&self, host: &St, control: usize) -> Option<(St, Vec<Value>)> {
        let engine = self.engine();
        let page = page_of(&engine, host);
        let source = match page.controls[control].part_link.as_ref().and_then(|l| l.provider) {
            Some(pi) => host
                .borrow()
                .parts
                .iter()
                .find(|(i, _)| *i == pi)
                .map(|(_, c)| c.clone())?,
            None => host.clone(),
        };
        if !self.has_source(&source) || source.borrow().cursor == Cursor::None {
            return None;
        }
        let fields = self.page_rec(&source).borrow().fields.clone();
        Some((source, fields))
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
            let source = self.link_source(host, control);
            let rec = self.page_rec(child);
            let table = self.table(&rec)?;
            let mut fresh: Vec<FieldFilter> = Vec::new();
            let mut fields = Vec::new();
            for entry in &link.entries {
                match entry {
                    ir::LinkEntry::Field(pf, hf) => {
                        fields.push(*pf);
                        let ty = table.fields[*pf].ty.clone();
                        let host_ty = {
                            let src_obj = match &source {
                                Some((s, _)) => s.borrow().object as usize,
                                None => host.borrow().object as usize,
                            };
                            let tbl = engine.program.objects[src_obj].rec_slot.map(|slot| {
                                match &engine.program.objects[src_obj].globals[slot].ty {
                                    RtType::Record(t, _) => *t as usize,
                                    _ => unreachable!("Rec is a record"),
                                }
                            });
                            tbl.and_then(|t| engine.program.objects[t].table.as_ref())
                                .map(|t| t.fields[*hf].ty.clone())
                        };
                        let value = match (&source, host_ty) {
                            (Some((_, f)), _) => f[*hf].clone(),
                            (None, Some(t)) => Value::default_for(&t),
                            (None, None) => return unsupported("a SubPageLink to a host without a source table"),
                        };
                        let value = ops::convert(value, &ty)?;
                        fresh.push(FieldFilter {
                            field: *pf,
                            expr: FilterExpr::Range(Some(value.clone()), Some(value)),
                            group: LINK_GROUP,
                        });
                    }
                    ir::LinkEntry::Filter(pf, text) => {
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
