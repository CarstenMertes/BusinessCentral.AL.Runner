//! `Filter Helper Triggers.MakeDateFilter`: the platform asks subscribers to repair a Date filter text it cannot read.
//!
//! Pinned by events/lt-platform-triggers/PtObserve.al (`Filter_Date`, `Bad_Date`), against PtFixtures.al (the subscriber)
//! and PtObjects.al (`datefiltertext`, `baddate`):
//! - The event fires once for each piece of the text that does not read as a date, with that piece (`311220` of
//!   `010120..311220`), before any error is raised.
//! - If the subscriber changes the piece and the new text does not read either, the event fires again with the new text.
//!   When a call leaves the text as it was, the piece has failed.
//! - A text with a `..` whose piece failed then fires once more with the whole text, by the same rule.
//!   A text without `..` fires only for the piece (`baddate` m1: one call).
//! - A repair that makes every piece read gives the filter built from the repaired pieces (`01/01/20..12/31/20`);
//!   a failure raises the error about the original text, whatever the subscribers did (`err-same`).
//!
//! Not pinned: a text with other filter syntax (`|`, `&`, `<`, `>`, `=`, `@`, quotes, wildcards, `...`), a session that is not
//! English, placeholders. The event is not raised for those (as before this change; a probe is wanted). Refused when it is raised:
//! more than one failing piece, a subscriber that changes the text a third time in a row.

use crate::Session;
use al_builtins::native::Signal;
use al_builtins::value::Value;

fn unpinned<T>(why: &str) -> Result<T, Signal> {
    Err(Signal::Unsupported(format!("MakeDateFilter: {why} (not pinned)")))
}

impl Session {
    /// One `MakeDateFilter` call: the text the subscribers leave in `DateFilterText`.
    fn make_date_filter(&mut self, text: &str) -> Result<String, Signal> {
        let names = ["DateFilterText".to_string()];
        let mut args = [Value::Text(text.to_string().into())];
        let call = crate::dispatch::EventCall {
            object: crate::events::FILTER_HELPER_TRIGGERS,
            event: "makedatefilter",
            element: None,
            isolated: false,
            names: &names,
            sender: None,
        };
        // Microsoft's own subscriber (System Application "Filter Tokens") is not run: it reads date formulas, which are not
        // pinned, and running it turned a passing corpus case into a refusal (filters/lt-date-text-forms/
        // DateFilterTextForms.al:GetRange_DayAboveTwelve_English). So only the subscribers of the app under test run.
        self.events.skip_dependency_subscribers = true;
        let raised = self.raise(&call, &mut args);
        self.events.skip_dependency_subscribers = false;
        raised?;
        Ok(al_builtins::native::text_of(&args[0]).unwrap_or_default().to_string())
    }

    /// Asks the subscribers about `piece` until it reads or a call leaves it alone. `Ok(None)`: it does not read.
    fn repair_date_piece(&mut self, piece: &str, reads: &dyn Fn(&str) -> bool) -> Result<Option<String>, Signal> {
        let mut current = piece.to_string();
        for round in 0..3 {
            let next = self.make_date_filter(&current)?;
            if next == current {
                return Ok(None);
            }
            if reads(&next) {
                return Ok(Some(next));
            }
            if round == 2 {
                return unpinned("a subscriber changed an unreadable text three times in a row");
            }
            current = next;
        }
        Ok(None)
    }

    /// The text to parse instead of `text`, after the subscribers had their say, or `None` when the text stays unreadable.
    /// `reads` says whether a Date filter text parses.
    pub(crate) fn repair_date_filter_text(
        &mut self,
        text: &str,
        reads: &dyn Fn(&str) -> bool,
    ) -> Result<Option<String>, Signal> {
        let t = text.trim();
        if t.contains("...")
            || t.chars().any(|c| "|&<>=@*?'\"()%".contains(c))
            || t.matches("..").count() > 1
        {
            // Not the pinned shape: the event is not raised for it (as before this change).
            return Ok(None);
        }
        let pieces: Vec<&str> = t.split("..").collect();
        let mut fixed: Vec<String> = pieces.iter().map(|p| p.trim().to_string()).collect();
        let mut failed = 0;
        for (i, p) in pieces.iter().enumerate() {
            let p = p.trim();
            if p.is_empty() || reads(p) {
                continue;
            }
            failed += 1;
            if failed > 1 {
                return unpinned("two unreadable pieces in one Date text");
            }
            match self.repair_date_piece(p, reads)? {
                Some(new) => fixed[i] = new,
                None => {}
            }
        }
        let rebuilt = fixed.join("..");
        if reads(&rebuilt) {
            return Ok(Some(rebuilt));
        }
        if pieces.len() > 1 {
            // The whole text is asked about as well (Filter_Date m1, m2).
            return self.repair_date_piece(t, reads);
        }
        Ok(None)
    }
}
