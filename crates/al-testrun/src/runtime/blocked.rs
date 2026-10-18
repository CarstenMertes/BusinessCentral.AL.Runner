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
    use al_builtins::value::RtType;
    let objs = &lowered.program.objects;
    let mut out = ReachFacts::default();
    let mut seen = BTreeSet::new();
    let mut stack = vec![(object, routine)];
    stack.extend(subscribers_reached(engine, lowered, object, routine));
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
    let objs = &lowered.program.objects;
    let mut seen = BTreeSet::new();
    let mut reached = BTreeSet::new();
    let mut stack = vec![(object, routine)];
    while let Some((o, r)) = stack.pop() {
        if !seen.insert((o, r)) {
            continue;
        }
        lowered.calls_into(o, r, &mut stack);
        let mut keys = Vec::new();
        if let Some(p) = engine.events.publishers.get(&(o, r)) {
            keys.push((p.object, p.event.clone()));
        }
        // Table events fire for any table whose record the routine uses.
        let mut tables = BTreeSet::new();
        for s in objs[o].routines[r].locals.iter().chain(&objs[o].globals) {
            if let al_builtins::value::RtType::Record(t, _) = &s.ty {
                tables.insert(*t as usize);
            }
        }
        keys.extend(
            engine
                .events
                .subscribers
                .keys()
                .filter(|(po, _)| tables.contains(po))
                .cloned(),
        );
        for k in keys {
            for sub in engine.events.subscribers.get(&k).into_iter().flatten() {
                if reached.insert((sub.object, sub.routine)) {
                    stack.push((sub.object, sub.routine));
                }
            }
        }
    }
    reached.into_iter().collect()
}

// ---- Permission lowering (ADR-027 open decision 3, owner 2026-10-18) ---------------------------------------------
// Permissions are not enforced: every access is granted, as for the SUPER user a BC test session runs as. A test
// that lowers its permissions could pass or fail for the wrong reason here, so it is not run.
//
// The one platform call that lowers is `PermissionTestHelper.AddEffectivePermissionSet`: Microsoft's
// "Permissions Mock".Assign makes it, under Set, SetExactPermissionSet and every "Library - Lower Permissions"
// procedure (corpus permissions/TestPermissionsMockEnforcement.al:25-41 pins that the sets are enforced). Start,
// Stop and ClearAssignments do not lower: with no set assigned the user's own permissions apply
// (TestPermissionsMockEnforcement.al:64-83). The mock's procedures do nothing while it is not started
// (TestPermissionsMockLifecycle.al:84-105), and BC's AL test runner starts it before every test method when codeunit
// "Reset State Before Test Run" exists (Tests-TestLibraries); this runtime does not run that codeunit, so the mock
// is never started here and a lowering would silently do nothing. The checks below decide statically instead.

/// The platform class whose `AddEffectivePermissionSet` narrows the session's permission sets.
const PERMISSION_TEST_HELPER: &str = "Microsoft.Dynamics.Nav.Runtime.PermissionTestHelper";
/// Codeunit "Reset State Before Test Run" (Tests-TestLibraries): run before every test method by BC's AL test runner
/// ("ALTestRunner Reset Environment", `ClearLegacyLibraries`, in Microsoft's Test Runner app) when it exists.
const RESET_STATE_BEFORE_TEST_RUN: u32 = 130301;
/// Codeunit "Set State Test Run" (Tests-TestLibraries): run by the same runner code next, only for a test method
/// whose TestPermissions is not Disabled. It narrows the session to the "D365 Full Access" sets.
const SET_STATE_TEST_RUN: u32 = 130302;

/// What BC's AL test runner does to the permissions mock before each test method of this program.
#[derive(Debug, Default, Clone)]
pub(crate) struct RunnerPermissions {
    /// Routines that call `PermissionTestHelper.AddEffectivePermissionSet` directly.
    lowering: BTreeSet<(usize, usize)>,
    /// The runner starts the mock before every test: a call that lowers takes effect.
    starts_mock: bool,
    /// The routine through which the runner lowers a test whose TestPermissions is not Disabled.
    lowers_unless_disabled: Option<(usize, usize)>,
}

fn dotnet_type_is(program: &al_ir::Program, ty: Option<usize>, name: &str) -> bool {
    ty.and_then(|t| program.dotnet_types.get(t))
        .is_some_and(|d| d.type_name.eq_ignore_ascii_case(name))
}

/// Whether the body of a routine has a `PermissionTestHelper` expression that `op` accepts.
fn calls_permission_helper(
    program: &al_ir::Program,
    rt: &al_ir::Routine,
    op: &dyn Fn(&al_ir::DotNetOp) -> bool,
) -> bool {
    let mut hit = false;
    al_ir::types::for_each_expr_post(&rt.body, &mut |e| {
        if let al_ir::Expr::DotNet { ty, op: o, .. } = e {
            hit |= op(o) && dotnet_type_is(program, *ty, PERMISSION_TEST_HELPER);
        }
    });
    hit
}

/// The first routine of `sites` that anything `routine` of `object` reaches through static calls.
fn reaches_any(
    lowered: &Lowered,
    object: usize,
    routine: usize,
    sites: &BTreeSet<(usize, usize)>,
) -> Option<(usize, usize)> {
    if sites.is_empty() {
        return None;
    }
    let mut seen = BTreeSet::new();
    let mut stack = vec![(object, routine)];
    while let Some(at) = stack.pop() {
        if !seen.insert(at) {
            continue;
        }
        if sites.contains(&at) {
            return Some(at);
        }
        lowered.calls_into(at.0, at.1, &mut stack);
    }
    None
}

fn on_run_of(lowered: &Lowered, id: u32) -> Option<(usize, usize)> {
    let objs = &lowered.program.objects;
    let oi = objs
        .iter()
        .position(|o| o.is_kind(al_ir::ObjKind::Codeunit) && o.id == Some(id))?;
    let ri = objs[oi]
        .routines
        .iter()
        .position(|r| r.is_trigger && r.name.eq_ignore_ascii_case("OnRun"))?;
    Some((oi, ri))
}

impl RunnerPermissions {
    pub(crate) fn of(lowered: &Lowered) -> RunnerPermissions {
        let program = &lowered.program;
        // Without the runner's codeunits nothing starts or narrows the mock (the corpus programs): skip the scan.
        let reset = on_run_of(lowered, RESET_STATE_BEFORE_TEST_RUN);
        let set_state = on_run_of(lowered, SET_STATE_TEST_RUN);
        if reset.is_none() && set_state.is_none() {
            return RunnerPermissions::default();
        }
        let helper = |t: &al_builtins::value::RtType| {
            matches!(t, al_builtins::value::RtType::DotNet(i)
                if dotnet_type_is(program, Some(*i as usize), PERMISSION_TEST_HELPER))
        };
        let mut lowering = BTreeSet::new();
        let mut constructs = BTreeSet::new();
        // Only an object that holds a PermissionTestHelper variable can call it.
        for (oi, o) in program.objects.iter().enumerate() {
            let holds = o.globals.iter().any(|g| helper(&g.ty))
                || o.routines.iter().any(|r| r.locals.iter().any(|l| helper(&l.ty)));
            if !holds {
                continue;
            }
            for (ri, r) in o.routines.iter().enumerate() {
                if calls_permission_helper(
                    program,
                    r,
                    &|op| matches!(op, al_ir::DotNetOp::Call(m) if m.eq_ignore_ascii_case("AddEffectivePermissionSet")),
                ) {
                    lowering.insert((oi, ri));
                }
                if calls_permission_helper(program, r, &|op| matches!(op, al_ir::DotNetOp::Construct)) {
                    constructs.insert((oi, ri));
                }
            }
        }
        let starts_mock = reset.is_some_and(|(o, r)| reaches_any(lowered, o, r, &constructs).is_some());
        let lowers_unless_disabled = set_state.and_then(|(o, r)| reaches_any(lowered, o, r, &lowering));
        RunnerPermissions {
            lowering,
            starts_mock,
            lowers_unless_disabled,
        }
    }

    /// Why `routine` of `object` (a test method, or a test codeunit's OnRun) lowers the session's permissions on BC.
    pub(crate) fn lowered(&self, lowered: &Lowered, object: usize, routine: usize) -> Option<String> {
        let objs = &lowered.program.objects;
        let site = |(o, r): (usize, usize)| format!("{}.{}", objs[o].name, objs[o].routines[r].name);
        let obj = &objs[object];
        let rt = &obj.routines[routine];
        let is_test = rt.attributes.iter().any(|a| a.name.eq_ignore_ascii_case("Test"));
        if let (true, Some(at)) = (is_test, self.lowers_unless_disabled) {
            if let Some(value) = test_permissions(obj, rt).filter(|v| !v.eq_ignore_ascii_case("Disabled")) {
                return Some(format!(
                    "TestPermissions = {value}: before this test BC's AL test runner runs codeunit {SET_STATE_TEST_RUN} \
                     \"Set State Test Run\", which lowers the permissions of the test session ({}); {LOWERING_NOT_ENFORCED}",
                    site(at)
                ));
            }
        }
        if !self.starts_mock {
            return None;
        }
        reaches_any(lowered, object, routine, &self.lowering).map(|at| {
            format!(
                "{}: lowers the permissions of the test session; {LOWERING_NOT_ENFORCED}",
                site(at)
            )
        })
    }
}

const LOWERING_NOT_ENFORCED: &str = "permissions are not enforced here (every access is granted, as for a SUPER user \
     on BC), so a test that lowers them is not run (ADR-027 decision 3)";

/// The TestPermissions BC's runner passes for a test method: the method's `[TestPermissions]` attribute unless it is
/// InheritFromTestCodeunit, then the codeunit's property. With neither, BC passes Restrictive (AL Runner v2 read it
/// from the platform's `NavTestExecution`; not pinned by the corpus, and refusing is the side that cannot give a
/// false result).
fn test_permissions(obj: &al_ir::Object, routine: &al_ir::Routine) -> Option<String> {
    routine
        .attributes
        .iter()
        .filter(|a| a.name.eq_ignore_ascii_case("TestPermissions"))
        .filter_map(|a| {
            a.text
                .split_once("::")
                .map(|(_, v)| v.trim_end_matches([')', ']', ' ']).to_string())
        })
        .find(|v| !v.eq_ignore_ascii_case("InheritFromTestCodeunit"))
        .or_else(|| {
            obj.properties
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case("TestPermissions"))
                .map(|(_, v)| v.trim().to_string())
        })
        .or_else(|| Some("Restrictive".into()))
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
