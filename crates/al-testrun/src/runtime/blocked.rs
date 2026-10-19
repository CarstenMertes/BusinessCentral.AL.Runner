//! What a test cannot run for: install codeunits, tables a test touches, blocked routines.
use super::*;

/// Runs every `Subtype = Install` codeunit's OnInstallAppPerCompany, then every
/// OnInstallAppPerDatabase with no company chosen (the order measured on BC 27.5), and commits
/// after each. An installer that cannot run is skipped; returned are the tables it touches and
/// why, so tests that touch them can be reported not run.
pub(crate) fn run_install_codeunits(lowered: &Lowered, session: &mut Session) -> Vec<(BTreeSet<u32>, String)> {
    let mut skipped = Vec::new();
    let installers: Vec<usize> = lowered
        .program
        .objects
        .iter()
        .enumerate()
        .filter(|(oi, o)| {
            o.is_kind(al_ir::ObjKind::Codeunit)
                && !lowered.dependency[*oi]
                && o.properties
                    .iter()
                    .any(|(k, v)| k.eq_ignore_ascii_case("Subtype") && v.trim().eq_ignore_ascii_case("Install"))
        })
        .map(|(oi, _)| oi)
        .collect();
    // Every OnInstallAppPerCompany first, then every OnInstallAppPerDatabase, which has no company
    // (measured on real BC 27.5: docs/probes-needed.md, the same order as the dependency apps').
    let company = session.set_company("");
    session.set_company(&company);
    for (trigger, no_company) in [("OnInstallAppPerCompany", false), ("OnInstallAppPerDatabase", true)] {
        for &oi in &installers {
            let o = &lowered.program.objects[oi];
            let inst = session.instance(oi);
            let Some(ri) = o
                .routines
                .iter()
                .position(|r| r.is_trigger && r.name.eq_ignore_ascii_case(trigger))
            else {
                continue;
            };
            if no_company {
                session.set_company("");
            }
            let why = match blocked_reason(lowered, oi, ri) {
                Some(reason) => Some(reason),
                None => match session.call(oi, ri, inst.clone(), Vec::new()) {
                    Ok(_) => None,
                    Err(Signal::Unsupported(r)) => Some(r),
                    Err(Signal::Error(e)) => Some(format!("failed (behaviour not pinned): {}", e.message)),
                    Err(_) => None,
                },
            };
            session.set_company(&company);
            // A trigger that ran is durable before the next one starts: a later trigger that
            // cannot run must not take its writes with it.
            if why.is_none() {
                al_builtins::native::Ctx::commit(session).expect("commit");
            }
            if let Some(why) = why {
                session.rollback_transaction();
                // Subscribers to events the installer raises are not in its static calls,
                // but they are what it would have written through.
                let mut touched = tables_touched(lowered, oi, ri);
                for (so, sr) in subscribers_reached(&session.engine.clone(), lowered, oi, ri) {
                    touched.extend(tables_touched(lowered, so, sr));
                }
                skipped.push((touched, format!("install codeunit {} could not run: {why}", o.name)));
            }
        }
    }
    session.commit_transaction();
    skipped
}

/// Tables whose records appear in the slots of anything `routine` can reach.
pub(crate) fn tables_touched(lowered: &Lowered, object: usize, routine: usize) -> BTreeSet<u32> {
    pub(crate) fn record_tables(t: &al_builtins::value::RtType, out: &mut BTreeSet<u32>) {
        if let al_builtins::value::RtType::Record(id, _) = t {
            out.insert(*id);
        }
    }
    let mut out = BTreeSet::new();
    let mut seen = BTreeSet::new();
    let mut stack = vec![(object, routine)];
    while let Some((o, r)) = stack.pop() {
        if !seen.insert((o, r)) {
            continue;
        }
        let obj = &lowered.program.objects[o];
        for s in obj.routines[r].locals.iter().chain(&obj.globals) {
            record_tables(&s.ty, &mut out);
        }
        lowered.calls_into(o, r, &mut stack);
    }
    out
}

/// What the static reach of a routine holds (calls, then the subscribers of the events it can set off): the tables it
/// reads or writes and whether it can draw a random number. Used to decide whether a failure after a refused test is
/// doubtful (`run.rs`). Writes are the record methods that change the database (Insert, Modify, Delete, DeleteAll,
/// ModifyAll, Rename) on a record of a known table, plus the source table of a page variable.
#[derive(Debug, Default, Clone)]
pub(crate) struct ReachFacts {
    /// Tables whose records appear in a slot of anything reached.
    pub touched: BTreeSet<u32>,
    /// Tables a reached routine writes.
    pub writes: BTreeSet<u32>,
    /// A write whose table is not known statically (a record in a field, a report or xmlport variable): it may write any table.
    pub writes_any: bool,
    /// A reached routine calls `Random` or `Randomize`.
    pub random: bool,
}

impl ReachFacts {
    /// Whether what is written here can be read from `tables`.
    pub fn writes_meet(&self, tables: &BTreeSet<u32>) -> bool {
        self.writes_any && !tables.is_empty() || !self.writes.is_disjoint(tables)
    }
}

pub(crate) fn reach_facts(engine: &Engine, lowered: &Lowered, object: usize, routine: usize) -> ReachFacts {
    reach_facts_in(&engine.events, lowered, object, routine)
}

/// [`reach_facts`] with the program's event registry instead of an engine.
pub(crate) fn reach_facts_in(
    events: &al_vm::events::Registry,
    lowered: &Lowered,
    object: usize,
    routine: usize,
) -> ReachFacts {
    use al_builtins::value::RtType;
    let objs = &lowered.program.objects;
    let mut out = ReachFacts::default();
    let mut seen = BTreeSet::new();
    let mut stack = vec![(object, routine)];
    stack.extend(subscribers_reached_in(events, lowered, object, routine));
    while let Some((o, r)) = stack.pop() {
        if !seen.insert((o, r)) {
            continue;
        }
        let obj = &objs[o];
        let rt = &obj.routines[r];
        for s in rt.locals.iter().chain(&obj.globals) {
            let mut ty = &s.ty;
            while let RtType::Array(_, inner) = ty {
                ty = inner;
            }
            match ty {
                RtType::Record(t, _) => {
                    out.touched.insert(*t);
                }
                // A page writes its source table; the table is the record of the page's `Rec` slot.
                RtType::Page(p) | RtType::TestPage(p) => {
                    let page = &objs[*p as usize];
                    match page.rec_slot.map(|i| &page.globals[i].ty) {
                        Some(RtType::Record(t, _)) => {
                            out.touched.insert(*t);
                            out.writes.insert(*t);
                        }
                        _ => out.writes_any = true,
                    }
                }
                RtType::Library(_) | RtType::DotNet(_) => {}
                _ => {}
            }
        }
        let slot_ty = |p: &al_ir::Place| -> Option<RtType> {
            let mut p = p;
            while let al_ir::Place::Index(b, _) = p {
                p = b;
            }
            let mut ty = match p {
                al_ir::Place::Local(i) => &rt.locals.get(*i)?.ty,
                al_ir::Place::Global(i) => &obj.globals.get(*i)?.ty,
                _ => return None,
            };
            while let RtType::Array(_, inner) = ty {
                ty = inner;
            }
            Some(ty.clone())
        };
        al_ir::types::for_each_expr_post(&rt.body, &mut |e| {
            let al_ir::Expr::Builtin { id, receiver, .. } = e else {
                return;
            };
            let name = id.name();
            if name.starts_with("System.Random") {
                out.random = true;
            } else if name.starts_with("Record.")
                && matches!(
                    id.method(),
                    "Insert" | "Modify" | "Delete" | "DeleteAll" | "ModifyAll" | "Rename"
                )
            {
                let table = receiver.as_deref().and_then(|a| match a {
                    al_ir::Arg::Ref(p) => slot_ty(p),
                    al_ir::Arg::Value(al_ir::Expr::Load(p)) => slot_ty(p),
                    _ => None,
                });
                match table {
                    Some(RtType::Record(t, _)) => {
                        out.writes.insert(t);
                    }
                    _ => out.writes_any = true,
                }
            }
        });
        lowered.calls_into(o, r, &mut stack);
    }
    out
}

/// Subscriber routines (object, routine) that `routine` of `object` can set off
/// through static calls: those of the events it publishes, and those of the table
/// events of every table whose records it uses -- followed through the routines
/// those subscribers call in turn.
pub(crate) fn subscribers_reached(
    engine: &Engine,
    lowered: &Lowered,
    object: usize,
    routine: usize,
) -> Vec<(usize, usize)> {
    subscribers_reached_in(&engine.events, lowered, object, routine)
}

fn subscribers_reached_in(
    events: &al_vm::events::Registry,
    lowered: &Lowered,
    object: usize,
    routine: usize,
) -> Vec<(usize, usize)> {
    let objs = &lowered.program.objects;
    let mut seen = BTreeSet::new();
    let mut reached = BTreeSet::new();
    let mut stack = vec![(object, routine)];
    while let Some((o, r)) = stack.pop() {
        if !seen.insert((o, r)) {
            continue;
        }
        lowered.calls_into(o, r, &mut stack);
        let mut reach = |sub: (usize, usize)| {
            if reached.insert(sub) {
                stack.push(sub);
            }
        };
        if let Some(p) = events.publishers.get(&(o, r)) {
            for sub in events
                .subscribers
                .get(&(p.object, p.event.clone()))
                .into_iter()
                .flatten()
            {
                reach((sub.object, sub.routine));
            }
        }
        // Table events fire for any table whose record the routine uses: every event of the table object. The result is
        // a set, so the order the subscribers are found in does not matter.
        let mut tables = BTreeSet::new();
        for s in objs[o].routines[r].locals.iter().chain(&objs[o].globals) {
            if let al_builtins::value::RtType::Record(t, _) = &s.ty {
                tables.insert(*t as usize);
            }
        }
        for t in tables {
            for &sub in events.subscribers_of(t) {
                reach(sub);
            }
        }
    }
    reached.into_iter().collect()
}

/// Why routine `routine` of `object` cannot run: the first reason found in
/// anything it can reach through static calls (hard rule 4: decided before running).
pub fn blocked_reason(lowered: &Lowered, object: usize, routine: usize) -> Option<String> {
    blocked_site(lowered, object, routine).map(|b| b.text)
}

/// A routine that cannot run, found from a root by [`blocked_site`].
pub struct Blocked {
    /// The routine that carries the lowering reason.
    pub object: usize,
    pub routine: usize,
    /// `Object.Routine: reason`.
    pub text: String,
}

/// [`blocked_reason`] with the routine the reason was found in.
pub fn blocked_site(lowered: &Lowered, object: usize, routine: usize) -> Option<Blocked> {
    let mut seen = BTreeSet::new();
    let mut stack = vec![(object, routine)];
    // `AL_BLOCK_PATH=1` prints the chain of calls from the root to the routine with the reason.
    let trace = std::env::var_os("AL_BLOCK_PATH").is_some();
    let mut parent = std::collections::BTreeMap::new();
    while let Some((o, r)) = stack.pop() {
        if !seen.insert((o, r)) {
            continue;
        }
        // The routine that writes an expression-bound page control (`pages::expand`) is optional: when it cannot be
        // lowered the control stays read-only, and a test that never sets the control is not refused for it.
        if lowered.program.objects[o].routines[r].name.starts_with("__sourceset_") {
            continue;
        }
        if let Some(reason) = lowered.unsupported[o][r].first() {
            let name = &lowered.program.objects[o].routines[r].name;
            let obj = &lowered.program.objects[o].name;
            if trace {
                let mut at = (o, r);
                let mut chain = vec![at];
                while let Some(&p) = parent.get(&at) {
                    chain.push(p);
                    at = p;
                }
                for (co, cr) in chain.iter().rev() {
                    let ob = &lowered.program.objects[*co];
                    eprintln!("  via {} {} .{}", ob.kind, ob.name, ob.routines[*cr].name);
                }
            }
            return Some(Blocked {
                object: o,
                routine: r,
                text: format!("{obj}.{name}: {reason}"),
            });
        }
        let before = stack.len();
        lowered.calls_into(o, r, &mut stack);
        if trace {
            for &next in &stack[before..] {
                parent.entry(next).or_insert((o, r));
            }
        }
    }
    None
}
