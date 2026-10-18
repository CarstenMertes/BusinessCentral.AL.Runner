//! The per-test record of what the code raised and touched, for the impact index of `alrunner impact-index build`
//! (ADR-043 section 2.2). Off unless asked for (`Session::enable_impact_trace`): then each note costs one branch.
//!
//! What is noted, and where:
//! - an event raised, at the raise and before the subscriber lookup, so a raise nobody listens to is noted too:
//!   a publisher routine (`dispatch::publish`), the table events (`records::table_event`), the page events
//!   (`pages::page_event`, the field and action events), and the platform events the runtime raises itself
//!   (`Global Triggers`, `OnResolveCaptionClass`, `CustomDocumentMergerEx`);
//! - a table, for every record operation on it (`records::record_op`, which typed records, `RecordRef` and the
//!   page client all go through), and whether the operation writes;
//! - a page whose instance is made (`pages::page_instance`: a `TestPage`, a `Page` variable, a part);
//! - a report whose layout and data items a use reads (`reports::report_schema`).

use std::collections::HashSet;

/// What a run of code raised and touched. Objects are indexes into `Program::objects`, or the pseudo keys of
/// `events` (`CAPTION_CLASS`, `GLOBAL_TRIGGERS`, `REPORTING_TRIGGERS`) for an event of a platform codeunit that is
/// not in the program.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Touches {
    /// (publisher object, lower-case event name). An event raised for an element (a field, an action) is noted
    /// twice: as `event` and as `event/element` (element lower-case).
    pub events: HashSet<(usize, String)>,
    /// Tables a record operation used (read or write).
    pub tables: HashSet<u32>,
    /// Tables a record operation wrote (insert, modify, delete, rename and their bulk forms, validate excluded).
    pub writes: HashSet<u32>,
    /// Pages whose instance was made.
    pub pages: HashSet<u32>,
    /// Reports that were used.
    pub reports: HashSet<u32>,
}

impl Touches {
    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
            && self.tables.is_empty()
            && self.writes.is_empty()
            && self.pages.is_empty()
            && self.reports.is_empty()
    }
}

impl crate::Session {
    /// Starts noting what the code raises and touches (`alrunner impact-index build`).
    pub fn enable_impact_trace(&mut self) {
        *self.coverage.impact.borrow_mut() = Some(Touches::default());
        self.coverage.impact_on = true;
    }

    /// What was noted since the last call; noting starts again. Empty when the trace is off.
    pub fn take_impact_trace(&mut self) -> Touches {
        match self.coverage.impact.borrow_mut().as_mut() {
            Some(t) => std::mem::take(t),
            None => Touches::default(),
        }
    }

    #[inline]
    pub(crate) fn note_event(&self, object: usize, event: &str, element: Option<&str>) {
        if !self.coverage.impact_on {
            return;
        }
        if let Some(t) = self.coverage.impact.borrow_mut().as_mut() {
            t.events.insert((object, event.to_ascii_lowercase()));
            if let Some(e) = element {
                t.events
                    .insert((object, format!("{}/{}", event.to_ascii_lowercase(), e.to_lowercase())));
            }
        }
    }

    #[inline]
    pub(crate) fn note_table(&self, table: u32, write: bool) {
        if !self.coverage.impact_on {
            return;
        }
        if let Some(t) = self.coverage.impact.borrow_mut().as_mut() {
            t.tables.insert(table);
            if write {
                t.writes.insert(table);
            }
        }
    }

    #[inline]
    pub(crate) fn note_page(&self, page: usize) {
        if !self.coverage.impact_on {
            return;
        }
        if let Some(t) = self.coverage.impact.borrow_mut().as_mut() {
            t.pages.insert(page as u32);
        }
    }

    #[inline]
    pub(crate) fn note_report(&self, report: usize) {
        if !self.coverage.impact_on {
            return;
        }
        if let Some(t) = self.coverage.impact.borrow_mut().as_mut() {
            t.reports.insert(report as u32);
        }
    }
}
