//! Whether a static event subscriber can tell one codeunit instance from another.
//!
//! A static subscriber of a codeunit that is not `SingleInstance` runs on an instance whose
//! lifetime the corpus does not pin (a fresh one per event, or one for the session). That choice
//! only matters if the subscriber can observe it, and it can only observe it through the global
//! variables of its codeunit: a routine that never touches a global (nor calls a procedure of its
//! own codeunit that does) behaves the same on any instance. Such a subscriber is run; one that
//! may observe the instance stays refused (`dispatch.rs`).

use crate::Engine;
use al_ir::Routine;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

thread_local! {
    /// (engine, object, routine) to the answer, so a hot event does not scan the IR each time.
    static MEMO: RefCell<HashMap<(usize, usize, usize), bool>> = RefCell::new(HashMap::new());
}

/// The text a routine's body prints as: an IR place of a global prints `Global(n)` and a call prints
/// `Call { object: o, routine: r, ...`.
fn body_text(r: &Routine) -> String {
    format!("{:?}", r.body)
}

/// The routines of `object` that `text` calls (any receiver: conservatively treated as the same
/// instance).
fn same_object_calls(text: &str, object: usize) -> Vec<usize> {
    let pat = format!("Call {{ object: {object}, routine: ");
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(i) = rest.find(&pat) {
        rest = &rest[i + pat.len()..];
        let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
        if let Ok(n) = digits.parse() {
            out.push(n);
        }
    }
    out
}

/// Whether routine `routine` of `object`, or a procedure of the same object it calls, uses a global.
pub(crate) fn may_observe_globals(engine: &Engine, object: usize, routine: usize) -> bool {
    let key = (engine as *const Engine as usize, object, routine);
    if let Some(known) = MEMO.with(|m| m.borrow().get(&key).copied()) {
        return known;
    }
    let routines = &engine.program.objects[object].routines;
    let mut seen = HashSet::new();
    let mut todo = vec![routine];
    let mut observes = false;
    while let Some(r) = todo.pop() {
        if !seen.insert(r) {
            continue;
        }
        let Some(rt) = routines.get(r) else {
            observes = true;
            break;
        };
        let text = body_text(rt);
        if text.contains("Global(") {
            observes = true;
            break;
        }
        todo.extend(same_object_calls(&text, object));
    }
    MEMO.with(|m| m.borrow_mut().insert(key, observes));
    observes
}
