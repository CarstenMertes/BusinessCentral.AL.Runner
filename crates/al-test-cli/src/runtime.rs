//! Runs test codeunits the way BC's test runner does, as far as the corpus
//! pins it: one instance per test codeunit, `OnRun` once before the tests
//! (codeunit/TestCodeunitOnRun.al), tests in declaration order.

use crate::report::Outcome;
use al_builtins::native::Signal;
use al_sema::lower::Lowered;
use al_vm::{Engine, Session, UiKind};
use std::collections::BTreeSet;
use std::sync::Arc;

/// Reads an app folder: its `app.json` manifest and every `.al` file under it.
pub fn load_app(dir: &std::path::Path) -> Result<al_sema::workspace::AppSource, String> {
    let manifest: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(dir.join("app.json")).map_err(|e| format!("{}: {e}", dir.display()))?,
    )
    .map_err(|e| format!("{}/app.json: {e}", dir.display()))?;
    let mut files = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).map_err(|e| e.to_string())? {
            let p = e.map_err(|e| e.to_string())?.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x == "al") {
                let rel = p.strip_prefix(dir).unwrap_or(&p).to_string_lossy().into_owned();
                files.push((
                    rel,
                    std::fs::read_to_string(&p).map_err(|e| format!("{}: {e}", p.display()))?,
                ));
            }
        }
    }
    files.sort();
    let s = |k: &str| manifest[k].as_str().unwrap_or_default().to_string();
    Ok(al_sema::workspace::AppSource {
        id: s("id"),
        name: s("name"),
        dependencies: manifest["dependencies"]
            .as_array()
            .map(|a| a.iter().filter_map(|d| d["id"].as_str().map(String::from)).collect())
            .unwrap_or_default(),
        files,
    })
}

/// Every corpus test's outcome on this runtime, keyed by (codeunit id, procedure).
pub struct CorpusRuntime {
    outcomes: std::collections::HashMap<(u32, String), Outcome>,
    pub panics: usize,
}

impl CorpusRuntime {
    pub fn run(apps: Vec<al_sema::workspace::AppSource>, symbols: &BTreeSet<String>) -> CorpusRuntime {
        CorpusRuntime::run_for(apps, symbols, None)
    }

    /// Runs for a target: builtins newer than the target's runtime are refused (ADR-015).
    /// Without a target, the newest catalog is used.
    pub fn run_for(
        apps: Vec<al_sema::workspace::AppSource>,
        symbols: &BTreeSet<String>,
        target: Option<&al_version::Target>,
    ) -> CorpusRuntime {
        let (_ws, lowered) = match target {
            Some(t) => al_sema::lower::compile_for(apps, symbols, t),
            None => al_sema::lower::compile(apps, symbols),
        };
        let blocked = lowered
            .unsupported
            .iter()
            .map(|o| o.iter().map(|r| r.first().cloned()).collect())
            .collect();
        let engine = Engine::new(lowered.program.clone(), blocked);
        let mut outcomes = std::collections::HashMap::new();
        let mut panics = 0;
        for (oi, o) in lowered.program.objects.iter().enumerate() {
            let is_test = o.kind == "codeunit"
                && o.properties
                    .iter()
                    .any(|(k, v)| k.eq_ignore_ascii_case("Subtype") && v.trim().eq_ignore_ascii_case("Test"));
            let Some(id) = o.id.filter(|_| is_test) else { continue };
            // A Rust panic is a runtime bug; contain it to this codeunit and report it.
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run_codeunit(&engine, &lowered, oi)));
            match result {
                Ok(list) => {
                    for (name, outcome) in list {
                        outcomes.insert((id, name.to_lowercase()), outcome);
                    }
                }
                Err(p) => {
                    panics += 1;
                    let msg = p
                        .downcast_ref::<String>()
                        .cloned()
                        .or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string()))
                        .unwrap_or_default();
                    for r in o
                        .routines
                        .iter()
                        .filter(|r| r.attributes.iter().any(|a| a.name.eq_ignore_ascii_case("Test")))
                    {
                        outcomes.insert(
                            (id, r.name.to_lowercase()),
                            Outcome::NotRun(format!("runtime panic (bug): {msg}")),
                        );
                    }
                }
            }
        }
        CorpusRuntime { outcomes, panics }
    }
}

impl crate::runner::Runner for CorpusRuntime {
    fn outcome(&self, case: &crate::oracle::CaseId) -> Outcome {
        self.outcomes
            .get(&(case.codeunit, case.name.to_lowercase()))
            .cloned()
            .unwrap_or_else(|| Outcome::NotRun("test procedure not found in the compiled corpus".into()))
    }
}

/// Outcomes of every `[Test]` procedure of test codeunit `object`, in order.
pub fn run_codeunit(engine: &Arc<Engine>, lowered: &Lowered, object: usize) -> Vec<(String, Outcome)> {
    let obj = &lowered.program.objects[object];
    let mut session = Session::new(engine.clone());
    let instance = session.instance(object);
    let tests: Vec<usize> = (0..obj.routines.len())
        .filter(|&r| {
            obj.routines[r]
                .attributes
                .iter()
                .any(|a| a.name.eq_ignore_ascii_case("Test"))
        })
        .collect();
    let all = |o: Outcome| {
        tests
            .iter()
            .map(|&r| (obj.routines[r].name.clone(), o.clone()))
            .collect::<Vec<_>>()
    };
    // [ErrorBehavior] is not implemented; tests in such a codeunit depend on each other, so none
    // of them runs. ([CommitBehavior] and [TransactionModel] are: see al-vm transactions.rs.)
    if let Some(a) = tests
        .iter()
        .flat_map(|&r| &obj.routines[r].attributes)
        .find(|a| a.name.eq_ignore_ascii_case("ErrorBehavior"))
    {
        return all(Outcome::NotRun(format!("[{}] is not implemented yet", a.name)));
    }
    // A [TransactionModel] other than AutoCommit, AutoRollback or None is refused.
    let mut models = std::collections::HashMap::new();
    for &r in &tests {
        for a in obj.routines[r]
            .attributes
            .iter()
            .filter(|a| a.name.eq_ignore_ascii_case("TransactionModel"))
        {
            match al_vm::TxModel::from_attribute(&a.text) {
                Some(m) => {
                    models.insert(r, m);
                }
                None => return all(Outcome::NotRun(format!("{}: unknown transaction model", a.text))),
            }
        }
    }
    // What publishing the app does first: run its install codeunits, then commit.
    let skipped_installs = run_install_codeunits(lowered, &mut session);
    session.in_test_codeunit = true;
    if let Some(on_run) = (0..obj.routines.len())
        .find(|&r| obj.routines[r].is_trigger && obj.routines[r].name.eq_ignore_ascii_case("OnRun"))
    {
        if let Some(reason) = blocked_reason(lowered, object, on_run) {
            return all(Outcome::NotRun(format!("OnRun: {reason}")));
        }
        match session.call(object, on_run, instance.clone(), Vec::new()) {
            // NavTestCodeunit.DoRunAsync commits after a declared OnRun
            // (codeunit/TestTestCodeunitOnRun.al).
            Ok(_) => session.commit_transaction(),
            Err(Signal::Unsupported(r)) => return all(Outcome::NotRun(format!("OnRun: {r}"))),
            Err(Signal::Error(e)) => {
                return all(Outcome::NotRun(format!(
                    "OnRun failed (runner behaviour not pinned): {}",
                    e.message
                )));
            }
            Err(_) => {}
        }
    }
    let mut out = Vec::new();
    for r in tests {
        let routine = &obj.routines[r];
        let handlers = resolve_handlers(lowered, object, r);
        let outcome = if let Err(reason) = &handlers {
            Outcome::NotRun(reason.clone())
        } else if let Some(reason) = blocked_reason(lowered, object, r) {
            Outcome::NotRun(reason)
        } else if let Some(reason) = skipped_installs
            .iter()
            .find(|(tables, _)| !tables.is_disjoint(&tables_touched(lowered, object, r)))
            .map(|(_, why)| why)
        {
            Outcome::NotRun(reason.clone())
        } else {
            let bound = handlers.unwrap_or_default();
            match bind_handlers(&mut session, &instance, &bound) {
                Err(reason) => Outcome::NotRun(reason),
                Ok(()) => {
                    session.last_error = Default::default();
                    let model = models.get(&r).copied().unwrap_or_default();
                    // TransactionModel::None is handled before the method body: every active
                    // transaction is ended on the way in (codeunit/TestWriteTxTestBoundary.al:
                    // Test08). Nothing is pending here, because every earlier test ended its
                    // transaction at its own boundary, so which way it ends is not observable.
                    if model == al_vm::TxModel::None {
                        session.commit_transaction();
                    }
                    session.tx_model = model;
                    let result = session.call(object, r, instance.clone(), Vec::new());
                    session.tx_model = al_vm::TxModel::AutoCommit;
                    let unexecuted = session.end_ui_scope();
                    match result {
                        // AutoRollback rolls the test's work back the moment it finishes, pass
                        // or fail (record/TestTransactionModelAutoRollback.al:Test01-Test03).
                        Ok(_) if unexecuted.is_empty() && model == al_vm::TxModel::AutoRollback => {
                            session.rollback_transaction();
                            Outcome::Pass
                        }
                        // The default TransactionModel commits at the test boundary; a failed
                        // test's transaction is rolled back (corpus codeunit/TestWriteTxTestBoundary.al:26).
                        Ok(_) if unexecuted.is_empty() => {
                            session.commit_transaction();
                            Outcome::Pass
                        }
                        // corpus handlers/TestPageQueryCloseMessageHandler_Tests.al:51: a declared
                        // handler that never fires is a test failure. The text after the prefix
                        // is not pinned (docs/probes-needed.md).
                        Ok(_) => {
                            session.rollback_transaction();
                            Outcome::Fail(format!(
                                "The following UI handlers were not executed: {}",
                                unexecuted.join(", ")
                            ))
                        }
                        Err(Signal::Error(e)) => {
                            session.rollback_transaction();
                            Outcome::Fail(e.message)
                        }
                        Err(Signal::Unsupported(r)) => Outcome::NotRun(r),
                        Err(other) => Outcome::Fail(format!("unexpected signal {other:?}")),
                    }
                }
            }
        };
        out.push((routine.name.clone(), outcome));
    }
    out
}

/// The handler functions test routine `test` of `object` declares with
/// `[HandlerFunctions('A,B')]`, as (kind, routine index), or why the test
/// cannot run with them. Handlers whose kind is not implemented are refused
/// here, by name and kind (hard rule 4).
fn resolve_handlers(lowered: &Lowered, object: usize, test: usize) -> Result<Vec<(UiKind, usize)>, String> {
    let obj = &lowered.program.objects[object];
    let mut out: Vec<(UiKind, usize)> = Vec::new();
    for attr in obj.routines[test]
        .attributes
        .iter()
        .filter(|a| a.name.eq_ignore_ascii_case("HandlerFunctions"))
    {
        let names: Vec<String> = match attr.args.as_slice() {
            [al_ir::Const::Text(list)] => list.split(',').map(|n| n.trim().to_string()).collect(),
            _ => return Err(format!("{} is not a list of handler names", attr.text)),
        };
        for name in names {
            let Some(hr) = obj.routines.iter().position(|r| r.name.eq_ignore_ascii_case(&name)) else {
                return Err(format!("handler function {name} not found in {}", obj.name));
            };
            if out.iter().any(|(_, r)| *r == hr) {
                continue;
            }
            let routine = &obj.routines[hr];
            let kind_attr =
                routine.attributes.iter().map(|a| a.name.as_str()).find(|n| {
                    n.to_ascii_lowercase().ends_with("handler") && !n.eq_ignore_ascii_case("HandlerFunctions")
                });
            let Some(kind_attr) = kind_attr else {
                return Err(format!(
                    "{} is named in [HandlerFunctions] but is no handler function",
                    routine.name
                ));
            };
            let Some(kind) = UiKind::from_attribute(kind_attr) else {
                let why = match kind_attr.to_ascii_lowercase().as_str() {
                    "requestpagehandler" | "filterpagehandler" => {
                        "it needs request and filter pages (docs/adr/012-testpage-runtime.md)"
                    }
                    "sendnotificationhandler" | "recallnotificationhandler" => "it needs the Notification type",
                    "sessionsettingshandler" => "it needs the SessionSettings type",
                    _ => "this handler kind",
                };
                return Err(format!(
                    "handler {} is a [{kind_attr}]: {why}, which is not implemented yet",
                    routine.name
                ));
            };
            let page_of = |r: &al_ir::Routine| match r.locals.first().map(|l| &l.ty) {
                Some(al_builtins::value::RtType::TestPage(p)) => Some(*p),
                _ => None,
            };
            let is_page = matches!(kind, UiKind::Page | UiKind::ModalPage);
            if let Some((_, other)) = out
                .iter()
                .find(|(k, r)| *k == kind && (!is_page || page_of(&obj.routines[*r]) == page_of(routine)))
            {
                return Err(format!(
                    "{} and {} are both {kind:?} handlers; which one BC would bind is not pinned",
                    obj.routines[*other].name, routine.name
                ));
            }
            out.push((kind, hr));
        }
    }
    // Code reachable from a handler must be runnable too (decided before running).
    for (_, hr) in &out {
        if let Some(reason) = blocked_reason(lowered, object, *hr) {
            return Err(format!("handler {}: {reason}", obj.routines[*hr].name));
        }
    }
    Ok(out)
}

/// Binds the resolved handlers to the test codeunit's instance for one test method.
fn bind_handlers(
    session: &mut Session,
    instance: &std::rc::Rc<std::cell::RefCell<al_builtins::value::Instance>>,
    handlers: &[(UiKind, usize)],
) -> Result<(), String> {
    let object = instance.borrow().object as usize;
    let mut bound = Vec::new();
    for (kind, routine) in handlers {
        bound.push(session.ui_handler(*kind, object, *routine, instance.clone())?);
    }
    session.begin_ui_scope(bound);
    Ok(())
}

/// Runs every `Subtype = Install` codeunit's OnInstallAppPerDatabase, then
/// its OnInstallAppPerCompany (doc: Writing extension install code), and
/// commits. An installer that cannot run is skipped; returned are the tables
/// it touches and why, so tests that touch them can be reported not run.
fn run_install_codeunits(lowered: &Lowered, session: &mut Session) -> Vec<(BTreeSet<u32>, String)> {
    let mut skipped = Vec::new();
    for (oi, o) in lowered.program.objects.iter().enumerate() {
        let install = o.kind == "codeunit"
            && o.properties
                .iter()
                .any(|(k, v)| k.eq_ignore_ascii_case("Subtype") && v.trim().eq_ignore_ascii_case("Install"));
        if !install {
            continue;
        }
        let inst = session.instance(oi);
        for trigger in ["OnInstallAppPerDatabase", "OnInstallAppPerCompany"] {
            let Some(ri) = o
                .routines
                .iter()
                .position(|r| r.is_trigger && r.name.eq_ignore_ascii_case(trigger))
            else {
                continue;
            };
            let why = match blocked_reason(lowered, oi, ri) {
                Some(reason) => Some(reason),
                None => match session.call(oi, ri, inst.clone(), Vec::new()) {
                    Ok(_) => None,
                    Err(Signal::Unsupported(r)) => Some(r),
                    Err(Signal::Error(e)) => Some(format!("failed (behaviour not pinned): {}", e.message)),
                    Err(_) => None,
                },
            };
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
        session.commit_transaction();
    }
    skipped
}

/// Tables whose records appear in the slots of anything `routine` can reach.
fn tables_touched(lowered: &Lowered, object: usize, routine: usize) -> BTreeSet<u32> {
    fn record_tables(t: &al_builtins::value::RtType, out: &mut BTreeSet<u32>) {
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
        stack.extend(lowered.calls[o][r].iter().copied());
    }
    out
}

/// Subscriber routines (object, routine) that `routine` of `object` can set off
/// through static calls: those of the events it publishes, and those of the table
/// events of every table whose records it uses -- followed through the routines
/// those subscribers call in turn.
fn subscribers_reached(engine: &Engine, lowered: &Lowered, object: usize, routine: usize) -> Vec<(usize, usize)> {
    let objs = &lowered.program.objects;
    let mut seen = BTreeSet::new();
    let mut reached = BTreeSet::new();
    let mut stack = vec![(object, routine)];
    while let Some((o, r)) = stack.pop() {
        if !seen.insert((o, r)) {
            continue;
        }
        stack.extend(lowered.calls[o][r].iter().copied());
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

/// Why routine `routine` of `object` cannot run: the first reason found in
/// anything it can reach through static calls (hard rule 4: decided before running).
pub fn blocked_reason(lowered: &Lowered, object: usize, routine: usize) -> Option<String> {
    let mut seen = BTreeSet::new();
    let mut stack = vec![(object, routine)];
    while let Some((o, r)) = stack.pop() {
        if !seen.insert((o, r)) {
            continue;
        }
        if let Some(reason) = lowered.unsupported[o][r].first() {
            let name = &lowered.program.objects[o].routines[r].name;
            let obj = &lowered.program.objects[o].name;
            return Some(format!("{obj}.{name}: {reason}"));
        }
        stack.extend(lowered.calls[o][r].iter().copied());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use al_sema::workspace::AppSource;

    fn outcomes(src: &str) -> Vec<(String, Outcome)> {
        let (ws, l) = al_sema::lower::compile(
            vec![AppSource {
                id: "a".into(),
                name: "a".into(),
                dependencies: vec![],
                files: vec![("f.al".into(), src.into())],
            }],
            &Default::default(),
        );
        assert!(ws.diagnostics.is_empty(), "{:?}", ws.diagnostics);
        let blocked = l
            .unsupported
            .iter()
            .map(|o| o.iter().map(|r| r.first().cloned()).collect())
            .collect();
        let engine = Engine::new(l.program.clone(), blocked);
        let oi = l
            .program
            .objects
            .iter()
            .position(|o| {
                o.properties
                    .iter()
                    .any(|(k, v)| k.eq_ignore_ascii_case("Subtype") && v.eq_ignore_ascii_case("Test"))
            })
            .unwrap();
        run_codeunit(&engine, &l, oi)
    }

    const SRC: &str = "codeunit 60100 T
{
    Subtype = Test;
    var
        Seen: Integer;

    trigger OnRun()
    begin
        Seen := 41;
    end;

    [Test]
    procedure A_Passes()
    begin
        Seen += 1;
        if Seen <> 42 then Error('OnRun did not run first: %1', Seen);
    end;

    [Test]
    procedure B_Fails()
    begin
        Error('expected failure');
    end;

    [Test]
    procedure C_NotRun()
    begin
        Helper();
    end;

    local procedure Helper()
    begin
        Sleep(1);
    end;

    procedure NotATest()
    begin
        Error('must not run');
    end;

    [Test]
    [HandlerFunctions('MsgHandler')]
    procedure D_NeedsHandler()
    begin
    end;

    [MessageHandler]
    procedure MsgHandler(Msg: Text)
    begin
    end;
}";

    #[test]
    fn runs_tests_in_order_after_onrun_on_one_instance() {
        let o = outcomes(SRC);
        let names: Vec<_> = o.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, vec!["A_Passes", "B_Fails", "C_NotRun", "D_NeedsHandler"]);
        assert_eq!(o[0].1, Outcome::Pass);
        assert_eq!(o[1].1, Outcome::Fail("expected failure".into()));
    }

    #[test]
    fn unreachable_code_does_not_block_but_reachable_unsupported_code_does() {
        let o = outcomes(SRC);
        assert!(
            matches!(&o[2].1, Outcome::NotRun(r) if r.contains("Sleep")),
            "{:?}",
            o[2].1
        );
    }

    fn outcomes_of(src: &str, codeunit: &str) -> Vec<(String, Outcome)> {
        let (ws, l) = al_sema::lower::compile(
            vec![AppSource {
                id: "a".into(),
                name: "a".into(),
                dependencies: vec![],
                files: vec![("f.al".into(), src.into())],
            }],
            &Default::default(),
        );
        assert!(ws.diagnostics.is_empty(), "{:?}", ws.diagnostics);
        let blocked = l
            .unsupported
            .iter()
            .map(|o| o.iter().map(|r| r.first().cloned()).collect())
            .collect();
        let engine = Engine::new(l.program.clone(), blocked);
        let oi = l.program.objects.iter().position(|o| o.name == codeunit).unwrap();
        run_codeunit(&engine, &l, oi)
    }

    fn with_rows(body: &str) -> String {
        format!("{ROWS}\n{body}")
    }

    const ROWS: &str = "table 60900 S { fields { field(1; K; Code[10]) { } } }
        codeunit 60910 Inserter { trigger OnRun() var R: Record S; begin R.K := 'RUN'; R.Insert(); end; }";

    // record/TestTransactionModelAutoRollback.al:Test01-Test03: an AutoRollback test's write is
    // rolled back when it finishes; the next default-model test's write survives.
    #[test]
    fn an_auto_rollback_test_is_rolled_back_when_it_finishes() {
        let o = outcomes(&with_rows(
            "            codeunit 60102 T { Subtype = Test;
                [Test] [TransactionModel(TransactionModel::AutoRollback)] procedure T1() var R: Record S; begin
                    R.K := 'A'; R.Insert(); if not R.Get('A') then Error('own write not visible'); end;
                [Test] procedure T2() var R: Record S; begin
                    if R.Get('A') then Error('the AutoRollback write survived');
                    R.K := 'B'; R.Insert(); end;
                [Test] procedure T3() var R: Record S; begin
                    if not R.Get('B') then Error('the default model write did not survive'); end; }",
        ));
        assert_eq!(
            o.iter().map(|(_, x)| x.clone()).collect::<Vec<_>>(),
            vec![Outcome::Pass; 3],
            "{o:?}"
        );
    }

    // record/TestTransactionModelAutoRollback.al:Test04-Test06
    #[test]
    fn auto_rollback_refuses_commit_anywhere_beneath_the_test_and_auto_commit_allows_it() {
        let o = outcomes(&with_rows(
            "            codeunit 60102 T { Subtype = Test;
                [Test] [TransactionModel(TransactionModel::AutoRollback)] procedure Direct() begin
                    asserterror Commit();
                    if GetLastErrorText() <> 'Tests cannot call the Commit function if TransactionModel property is set to AutoRollback.' then
                        Error('wrong text: %1', GetLastErrorText()); end;
                [Test] [TransactionModel(TransactionModel::AutoRollback)] procedure Callee() begin
                    asserterror Down();
                    if StrPos(GetLastErrorText(), 'Tests cannot call the Commit function') = 0 then
                        Error('wrong text: %1', GetLastErrorText()); end;
                [Test] [TransactionModel(TransactionModel::AutoCommit)] procedure AutoCommitAllows() var R: Record S; begin
                    R.K := 'C'; R.Insert(); Commit(); asserterror Error('x');
                    if not R.Get('C') then Error('the commit did not stick'); R.Delete(); Commit(); end;
                local procedure Down() begin Commit(); end; }"
        ));
        assert_eq!(
            o.iter().map(|(_, x)| x.clone()).collect::<Vec<_>>(),
            vec![Outcome::Pass; 3],
            "{o:?}"
        );
    }

    // record/TestTransactionModelAutoRollback.al:Test07 and Test08
    #[test]
    fn commit_behavior_ignore_exempts_the_auto_rollback_refusal_and_error_does_not_outrank_it() {
        let o = outcomes(&with_rows(
            "            codeunit 60102 T { Subtype = Test;
                [Test] [TransactionModel(TransactionModel::AutoRollback)] procedure Ignored() var R: Record S; begin
                    InsertAndCommitIgnoring();
                    asserterror Error('unrelated');
                    if R.Get('I') then Error('an ignored Commit moved the boundary'); end;
                [Test] [TransactionModel(TransactionModel::AutoRollback)] procedure ErrorBehavior() begin
                    asserterror CommitUnderError();
                    if StrPos(GetLastErrorText(), 'Tests cannot call the Commit function') = 0 then
                        Error('wrong text: %1', GetLastErrorText());
                    if StrPos(GetLastErrorText(), 'Commit is prohibited in the current scope') <> 0 then
                        Error('the CommitBehavior text won'); end;
                [CommitBehavior(CommitBehavior::Ignore)]
                local procedure InsertAndCommitIgnoring() var R: Record S; begin R.K := 'I'; R.Insert(); Commit(); end;
                [CommitBehavior(CommitBehavior::Error)]
                local procedure CommitUnderError() begin Commit(); end; }",
        ));
        assert_eq!(
            o.iter().map(|(_, x)| x.clone()).collect::<Vec<_>>(),
            vec![Outcome::Pass; 2],
            "{o:?}"
        );
    }

    // codeunit/TestWriteTxTestBoundary.al:Test02-Test06: no write transaction crosses a test
    // boundary, and a guarded Codeunit.Run is allowed at the start of the next test.
    #[test]
    fn no_write_transaction_survives_a_test_boundary() {
        let o = outcomes(&with_rows(
            "            codeunit 60102 T { Subtype = Test;
                [Test] [TransactionModel(TransactionModel::AutoRollback)] procedure RollbackWrites() var R: Record S; begin
                    R.K := 'A'; R.Insert();
                    if not Database.IsInWriteTransaction() then Error('no write transaction after an insert'); end;
                [Test] [TransactionModel(TransactionModel::AutoRollback)] procedure NextStartsClean() var Ok: Boolean; begin
                    if Database.IsInWriteTransaction() then Error('inherited a write transaction');
                    Ok := Codeunit.Run(Codeunit::Inserter);
                    if not Ok then Error('guarded run refused'); end;
                [Test] procedure DefaultWrites() var R: Record S; begin
                    R.K := 'D'; R.Insert();
                    if not Database.IsInWriteTransaction() then Error('no write transaction after an insert'); end;
                [Test] procedure GuardedRunAfterDefaultWrite() var R: Record S; Ok: Boolean; begin
                    if Database.IsInWriteTransaction() then Error('the default test left a write transaction');
                    Ok := Codeunit.Run(Codeunit::Inserter);
                    if not (Ok and R.Get('D')) then Error('guarded run refused or D lost'); end; }"
        ));
        assert_eq!(
            o.iter().map(|(_, x)| x.clone()).collect::<Vec<_>>(),
            vec![Outcome::Pass; 4],
            "{o:?}"
        );
    }

    // codeunit/TestWriteTxTestBoundary.al:Test08-Test10: a None test starts with no
    // transaction, cannot write directly, and can write inside a Codeunit.Run.
    #[test]
    fn a_none_test_has_no_transaction_until_it_runs_a_codeunit() {
        let o = outcomes(&with_rows(
            "            codeunit 60102 T { Subtype = Test;
                [Test] procedure Default() var R: Record S; begin R.K := 'D'; R.Insert(); end;
                [Test] [TransactionModel(TransactionModel::None)] procedure StartsClean() var R: Record S; Ok: Boolean; begin
                    if Database.IsInWriteTransaction() then Error('a None test started inside a write transaction');
                    if not R.Get('D') then Error('the previous test row is gone');
                    Ok := Codeunit.Run(Codeunit::Inserter);
                    if not Ok then Error('guarded run refused'); end;
                [Test] [TransactionModel(TransactionModel::None)] procedure CannotWrite() var R: Record S; begin
                    R.K := 'N';
                    asserterror R.Insert();
                    if GetLastErrorText() <> 'A transaction must be started before changes can be made to the database.' then
                        Error('wrong text: %1', GetLastErrorText());
                    if Database.IsInWriteTransaction() then Error('a refused write left a transaction open');
                    if R.Get('N') then Error('the refused insert wrote'); end;
                [Test] [TransactionModel(TransactionModel::None)] procedure DeleteAllUnderNone() var R: Record S; begin
                    R.DeleteAll();
                    end; }"
        ));
        assert_eq!(o[0].1, Outcome::Pass, "{o:?}");
        assert_eq!(o[1].1, Outcome::Pass, "{o:?}");
        assert_eq!(o[2].1, Outcome::Pass, "{o:?}");
        // DeleteAll under None: the refusal is not pinned.
        assert!(matches!(&o[3].1, Outcome::NotRun(r) if r.contains("None")), "{o:?}");
    }

    // codeunit/TestWriteTxTestBoundary.al:Test10_ARunCodeunitMayWriteUnderNone
    #[test]
    fn a_codeunit_run_from_a_none_test_may_write_and_its_transaction_ends_with_it() {
        let o = outcomes(&with_rows(
            "            codeunit 60102 T { Subtype = Test;
                [Test] [TransactionModel(TransactionModel::None)] procedure Run() var R: Record S; begin
                    Codeunit.Run(Codeunit::Inserter);
                    if not R.Get('RUN') then Error('the run did not write');
                    if Database.IsInWriteTransaction() then Error('the run left a transaction open'); end; }",
        ));
        assert_eq!(o[0].1, Outcome::Pass, "{o:?}");
    }

    // codeunit/TestCodeunitRunNestedTestCodeunit.al: Codeunit.Run of a test codeunit from a
    // running test raises, guarded or not, in the static and the instance form, and runs nothing.
    #[test]
    fn running_a_test_codeunit_from_a_test_is_refused() {
        let o = outcomes_of(
            "codeunit 60201 Inner { Subtype = Test;
                [Test] procedure InnerTest() begin Error('the inner test ran'); end; }
            codeunit 60202 Plain { trigger OnRun() begin end; }
            codeunit 60203 Outer { Subtype = Test;
                [Test] procedure Unguarded() begin
                    asserterror Codeunit.Run(Codeunit::Inner);
                    if StrPos(GetLastErrorText(), 'You cannot nest the execution of test codeunits.') = 0 then Error('wrong text: %1', GetLastErrorText());
                    if StrPos(GetLastErrorText(), 'Test codeunit 60201 Inner was called from another test codeunit.') = 0 then Error('wrong text: %1', GetLastErrorText()); end;
                [Test] procedure Guarded() var Ok: Boolean; begin
                    Ok := true;
                    asserterror Ok := Codeunit.Run(Codeunit::Inner);
                    if not Ok then Error('a result was assigned');
                    if StrPos(GetLastErrorText(), 'You cannot nest the execution of test codeunits.') = 0 then Error('wrong text: %1', GetLastErrorText()); end;
                [Test] procedure Instance() var I: Codeunit Inner; begin
                    asserterror I.Run();
                    if StrPos(GetLastErrorText(), 'You cannot nest the execution of test codeunits.') = 0 then Error('wrong text: %1', GetLastErrorText()); end;
                [Test] procedure PlainStillRuns() var Ok: Boolean; begin
                    Ok := Codeunit.Run(Codeunit::Plain);
                    if not Ok then Error('plain codeunit failed'); end; }",
            "Outer",
        );
        assert_eq!(
            o.iter().map(|(_, x)| x.clone()).collect::<Vec<_>>(),
            vec![Outcome::Pass; 4],
            "{o:?}"
        );
    }

    // The default TransactionModel commits a test's work at the test boundary (corpus
    // codeunit/TestWriteTxTestBoundary.al:26; record/TestCalcFormulaFlowFieldValueTests.Codeunit.al:182,
    // whose asserterror rolls back to "whatever the previous test committed").
    #[test]
    fn a_passing_test_commits_so_a_later_asserterror_rolls_back_only_to_that_point() {
        let o = outcomes(
            "table 60900 S { fields { field(1; K; Code[10]) { } } }
            codeunit 60102 T { Subtype = Test;
                [Test] procedure A() var R: Record S; begin R.K := 'A'; R.Insert(); end;
                [Test] procedure B() var R: Record S; begin
                    R.K := 'B'; R.Insert();
                    asserterror Error('x');
                    if not R.Get('A') then Error('the first test was not committed');
                    if R.Get('B') then Error('the second tests own write was not rolled back');
                end; }",
        );
        assert_eq!(o[1].1, Outcome::Pass, "{o:?}");
    }

    #[test]
    fn install_codeunits_run_before_the_tests() {
        let o = outcomes("table 60900 S { fields { field(1; K; Code[10]) { } } }
            codeunit 60901 Inst { Subtype = Install;
                trigger OnInstallAppPerCompany() var R: Record S; begin R.K := 'CO'; R.Insert(); end;
                trigger OnInstallAppPerDatabase() var R: Record S; begin R.K := 'DB'; R.Insert(); end; }
            codeunit 60102 T { Subtype = Test;
                [Test] procedure A() var R: Record S; begin if R.Count() <> 2 then Error('install did not run'); end; }");
        assert_eq!(o[0].1, Outcome::Pass, "{o:?}");
    }

    #[test]
    fn a_blocked_installer_also_blocks_tests_on_tables_its_event_subscribers_touch() {
        // install/TestInstallEventDispatch_*.al: the installer raises an event whose
        // subscriber seeds a table the installer never names; skipping the installer
        // must not let tests on that table run against an unseeded store.
        let o = outcomes(
            "table 60900 Seed { fields { field(1; K; Code[10]) { } } }
            codeunit 60904 Pub { [IntegrationEvent(false, false)] procedure OnSeed() begin end; }
            codeunit 60905 Sub {
                [EventSubscriber(ObjectType::Codeunit, Codeunit::Pub, 'OnSeed', '', false, false)]
                local procedure Seed() var R: Record Seed; begin R.Insert(); end; }
            codeunit 60901 Inst { Subtype = Install;
                trigger OnInstallAppPerCompany() var P: Codeunit Pub; begin Sleep(1); P.OnSeed(); end; }
            codeunit 60102 T { Subtype = Test;
                [Test] procedure UsesSeed() var R: Record Seed; begin end; }",
        );
        assert!(matches!(&o[0].1, Outcome::NotRun(r) if r.contains("install")), "{o:?}");
    }

    #[test]
    fn a_blocked_installer_also_blocks_tests_on_tables_written_by_table_event_subscribers() {
        // The installer writes to Src, whose Insert event subscriber seeds Seed: skipping the
        // installer must not let tests on Seed run against an unseeded store.
        let o = outcomes(
            "table 60900 Seed { fields { field(1; K; Code[10]) { } } }
            table 60903 Src { fields { field(1; K; Code[10]) { } } }
            codeunit 60905 Sub {
                [EventSubscriber(ObjectType::Table, Database::Src, 'OnAfterInsertEvent', '', false, false)]
                local procedure Seed(var Rec: Record Src) var R: Record Seed; begin R.K := 'X'; R.Insert(); end; }
            codeunit 60901 Inst { Subtype = Install;
                trigger OnInstallAppPerCompany() var S: Record Src; begin Sleep(1); S.K := 'A'; S.Insert(); end; }
            codeunit 60102 T { Subtype = Test;
                [Test] procedure UsesSeed() var R: Record Seed; begin end; }",
        );
        assert!(matches!(&o[0].1, Outcome::NotRun(r) if r.contains("install")), "{o:?}");
    }

    #[test]
    fn a_blocked_installer_only_blocks_tests_that_touch_its_tables() {
        let o = outcomes(
            "table 60900 S { fields { field(1; K; Code[10]) { } } }
            table 60903 Other { fields { field(1; K; Code[10]) { } } }
            codeunit 60901 Inst { Subtype = Install;
                trigger OnInstallAppPerCompany() var R: Record S; begin Sleep(1); R.Insert(); end; }
            codeunit 60102 T { Subtype = Test;
                [Test] procedure UsesS() var R: Record S; begin end;
                [Test] procedure UsesOther() var R: Record Other; begin end; }",
        );
        assert!(matches!(&o[0].1, Outcome::NotRun(r) if r.contains("install")), "{o:?}");
        assert_eq!(o[1].1, Outcome::Pass, "{o:?}");
    }

    // ---- test handler functions (docs/probes-needed.md lists what is unpinned) ----

    fn outcome_of(o: &[(String, Outcome)], name: &str) -> Outcome {
        o.iter()
            .find(|(n, _)| n == name)
            .unwrap_or_else(|| panic!("{name}: {o:?}"))
            .1
            .clone()
    }

    const UI: &str = "codeunit 60110 UiT { Subtype = Test;
        var Seen: Text; Fired: Integer; Asked: Text;

        // handlers/TestMessageHandler.al:15 Message_Show_HandlerCaptures
        [Test] [HandlerFunctions('Msg')]
        procedure MessageReachesHandler() begin Message('Hello'); if Seen <> 'Hello' then Error('got %1', Seen); end;

        // handlers/TestMessageHandler.al:23 Message_FormattedString_HandlerGetsFormatted
        [Test] [HandlerFunctions('Msg')]
        procedure MessageArgsAreSubstitutedBeforeTheHandler() begin Message('Value: %1', 42); if Seen <> 'Value: 42' then Error('got %1', Seen); end;

        // handlers/TestMessageHandler.al:31,41 Confirm_Question_HandlerRepliesTrue / False
        [Test] [HandlerFunctions('Yes')]
        procedure ConfirmHandlerRepliesTrue() begin if not Confirm('Sure?') then Error('false'); end;
        [Test] [HandlerFunctions('No')]
        procedure ConfirmHandlerRepliesFalse() begin if Confirm('Sure?', true) then Error('true'); end;

        // handlers/TestConfirmFormatArgs.al:Confirm_SingleFormatArg_HandlerReceivesRawUnsubstitutedQuestion
        [Test] [HandlerFunctions('No')]
        procedure ConfirmHandlerGetsTheRawQuestion() begin if Confirm('%1 items. Go?', true, 2) then; if Asked <> '%1 items. Go?' then Error('got %1', Asked); end;

        // handlers/TestConfirmFormatArgs.al:Confirm_DefaultChoiceHonored_WhenHandlerDoesNotOverrideReply
        [Test] [HandlerFunctions('Silent')]
        procedure ConfirmDefaultSurvivesAHandlerThatLeavesReply()
        begin
            if not Confirm('q', true) then Error('default true lost');
            if Confirm('q', false) then Error('default false lost');
        end;

        // handlers/TestConfirmFormatArgs.al: one handler answers two Confirms
        [Test] [HandlerFunctions('No')]
        procedure OneHandlerServesManyCalls() begin Fired := 0; if Confirm('a', true) then; if Confirm('b', true) then; if Fired <> 2 then Error('fired %1', Fired); end;

        // handlers/TestConfirmNoHandler.al:Confirm_NoHandler_TestBody_IsRefused
        [Test]
        procedure ConfirmWithoutHandlerIsRefused()
        var Reply: Boolean;
        begin
            Reply := false;
            asserterror Reply := Confirm('CNH question', true);
            if StrPos(GetLastErrorText(), 'Unhandled UI') = 0 then Error('no Unhandled UI: %1', GetLastErrorText());
            if StrPos(GetLastErrorText(), 'Confirm') = 0 then Error('no Confirm: %1', GetLastErrorText());
            if StrPos(GetLastErrorText(), 'CNH question') = 0 then Error('no question: %1', GetLastErrorText());
            if Reply then Error('default was assigned');
        end;

        // handlers/TestPageQueryCloseError_Tests.al:75 (Unhandled UI ... Message)
        [Test]
        procedure MessageWithoutHandlerIsRefused()
        begin
            asserterror Message('lost text');
            if StrPos(GetLastErrorText(), 'Unhandled UI: Message') = 0 then Error('got %1', GetLastErrorText());
            if StrPos(GetLastErrorText(), 'lost text') = 0 then Error('text missing: %1', GetLastErrorText());
        end;

        // a Message handler does not answer a Confirm
        [Test] [HandlerFunctions('Msg')]
        procedure HandlerOfAnotherKindDoesNotAnswer()
        begin
            asserterror if Confirm('q') then;
            if StrPos(GetLastErrorText(), 'Unhandled UI: Confirm') = 0 then Error('got %1', GetLastErrorText());
            Message('now used');
        end;

        // handlers/TestPageQueryCloseMessageHandler_Tests.al:51 (UI handlers were not executed)
        [Test] [HandlerFunctions('Msg')]
        procedure DeclaredHandlerThatNeverFiresFailsTheTest() begin end;

        // an error in the test wins over the unexecuted handler
        [Test] [HandlerFunctions('Msg')]
        procedure TestErrorIsReportedBeforeUnexecutedHandlers() begin Error('own error'); end;

        // an error raised in a handler comes out of the UI call
        [Test] [HandlerFunctions('Boom')]
        procedure HandlerErrorPropagates() begin Message('x'); end;

        // handlers of the previous test are not bound any more
        [Test]
        procedure HandlersDoNotLeakIntoTheNextTest()
        begin
            asserterror Message('leak?');
            if StrPos(GetLastErrorText(), 'Unhandled UI') = 0 then Error('got %1', GetLastErrorText());
        end;

        // network/TestHttpClient.al:36 [StrMenuHandler] AllowOnceHandler
        [Test] [HandlerFunctions('Menu')]
        procedure StrMenuHandlerChoosesAnOption()
        var C: Integer;
        begin
            C := StrMenu('One,Two,Three', 1, 'Pick');
            if C <> 2 then Error('choice %1', C);
            if Asked <> 'One,Two,Three|Pick' then Error('asked %1', Asked);
        end;

        [Test]
        procedure StrMenuWithoutHandlerIsRefused()
        begin
            asserterror if StrMenu('A,B') = 0 then;
            if StrPos(GetLastErrorText(), 'Unhandled UI: StrMenu') = 0 then Error('got %1', GetLastErrorText());
        end;

        [Test] [HandlerFunctions('Nope')]
        procedure UnknownHandlerNameIsRefused() begin end;

        [Test] [HandlerFunctions('Helper')]
        procedure NamingARoutineThatIsNoHandlerIsRefused() begin end;

        [Test] [HandlerFunctions('Blocked')]
        procedure HandlerWithUnsupportedCodeIsRefused() begin Message('x'); end;

        [Test] [HandlerFunctions('Msg,Msg2')]
        procedure TwoHandlersOfOneKindAreRefused() begin end;

        [MessageHandler] procedure Msg(M: Text[1024]) begin Seen := M; end;
        [MessageHandler] procedure Msg2(M: Text[1024]) begin end;
        [MessageHandler] procedure Boom(M: Text[1024]) begin Error('boom in handler'); end;
        [MessageHandler] procedure Blocked(M: Text[1024]) begin Sleep(1); end;
        [ConfirmHandler] procedure Yes(Q: Text[1024]; var Reply: Boolean) begin Reply := true; end;
        [ConfirmHandler] procedure No(Q: Text[1024]; var Reply: Boolean) begin Fired += 1; Asked := Q; Reply := false; end;
        [ConfirmHandler] procedure Silent(Q: Text[1024]; var Reply: Boolean) begin end;
        [StrMenuHandler] procedure Menu(Options: Text; var Choice: Integer; Instruction: Text) begin Asked := Options + '|' + Instruction; Choice := 2; end;
        procedure Helper() begin end;
    }";

    fn ui(name: &str) -> Outcome {
        outcome_of(&outcomes(UI), name)
    }

    #[test]
    fn message_handler_receives_the_text_and_substituted_arguments() {
        assert_eq!(ui("MessageReachesHandler"), Outcome::Pass);
        assert_eq!(ui("MessageArgsAreSubstitutedBeforeTheHandler"), Outcome::Pass);
    }

    #[test]
    fn confirm_handler_replies_and_receives_the_raw_question() {
        assert_eq!(ui("ConfirmHandlerRepliesTrue"), Outcome::Pass);
        assert_eq!(ui("ConfirmHandlerRepliesFalse"), Outcome::Pass);
        assert_eq!(ui("ConfirmHandlerGetsTheRawQuestion"), Outcome::Pass);
        assert_eq!(ui("ConfirmDefaultSurvivesAHandlerThatLeavesReply"), Outcome::Pass);
        assert_eq!(ui("OneHandlerServesManyCalls"), Outcome::Pass);
    }

    #[test]
    fn ui_without_a_bound_handler_of_its_kind_is_refused_with_unhandled_ui() {
        assert_eq!(ui("ConfirmWithoutHandlerIsRefused"), Outcome::Pass);
        assert_eq!(ui("MessageWithoutHandlerIsRefused"), Outcome::Pass);
        assert_eq!(ui("HandlerOfAnotherKindDoesNotAnswer"), Outcome::Pass);
        assert_eq!(ui("StrMenuWithoutHandlerIsRefused"), Outcome::Pass);
        assert_eq!(ui("HandlersDoNotLeakIntoTheNextTest"), Outcome::Pass);
    }

    #[test]
    fn a_declared_handler_that_never_fires_fails_the_test() {
        assert!(matches!(ui("DeclaredHandlerThatNeverFiresFailsTheTest"),
                Outcome::Fail(m) if m.starts_with("The following UI handlers were not executed") && m.contains("Msg")),);
        assert_eq!(
            ui("TestErrorIsReportedBeforeUnexecutedHandlers"),
            Outcome::Fail("own error".into())
        );
    }

    #[test]
    fn an_error_in_a_handler_comes_out_of_the_ui_call() {
        assert_eq!(ui("HandlerErrorPropagates"), Outcome::Fail("boom in handler".into()));
    }

    #[test]
    fn str_menu_handler_chooses() {
        assert_eq!(ui("StrMenuHandlerChoosesAnOption"), Outcome::Pass);
    }

    #[test]
    fn handlers_the_runtime_cannot_run_are_refused_explicitly() {
        let nr = |n: &str| match ui(n) {
            Outcome::NotRun(r) => r,
            other => panic!("{n}: {other:?}"),
        };
        assert!(nr("UnknownHandlerNameIsRefused").contains("Nope"));
        assert!(nr("NamingARoutineThatIsNoHandlerIsRefused").contains("Helper"));
        assert!(nr("HandlerWithUnsupportedCodeIsRefused").contains("Sleep"));
        assert!(nr("TwoHandlersOfOneKindAreRefused").contains("Msg2"));
    }

    #[test]
    fn page_report_and_other_handlers_are_refused_with_their_kind() {
        let o = outcomes(
            "page 60120 Pg { PageType = Card; }
            report 60123 Rp { }
            codeunit 60121 T { Subtype = Test;
                [Test] [HandlerFunctions('Modal')] procedure A() begin end;
                [Test] [HandlerFunctions('Req')] procedure B() begin end;
                [ModalPageHandler] procedure Modal(var P: TestPage Pg) begin end;
                [RequestPageHandler] procedure Req(var P: TestRequestPage Rp) begin end; }",
        );
        // A modal handler is supported now; one the test never triggers is reported as BC does.
        assert!(
            matches!(&o[0].1, Outcome::Fail(r) if r.contains("not executed")),
            "{o:?}"
        );
        assert!(
            matches!(&o[1].1, Outcome::NotRun(r) if r.contains("RequestPageHandler")),
            "{o:?}"
        );
    }

    #[test]
    fn ui_outside_a_test_method_is_not_run() {
        // OnRun is not a test method: what a UI call does there is not pinned.
        let o = outcomes(
            "codeunit 60122 T { Subtype = Test;
                trigger OnRun() begin Message('x'); end;
                [Test] procedure A() begin end; }",
        );
        assert!(
            matches!(&o[0].1, Outcome::NotRun(r) if r.contains("OnRun") && r.contains("outside a test method")),
            "{o:?}"
        );
    }

    // ---- page handlers (ADR-012) ----

    const PAGES: &str = r#"
table 60700 "PH Row" { fields { field(1; "No."; Code[20]) { } field(2; Descr; Text[50]) { } } keys { key(PK; "No.") { } } }
page 60701 "PH Card" { PageType = Card; SourceTable = "PH Row"; layout { area(Content) { field(NoCtl; Rec."No.") { } field(DescrCtl; Rec.Descr) { } } } }
page 60702 "PH Host"
{
    PageType = Card; SourceTable = "PH Row";
    layout { area(Content) { field(NoCtl; Rec."No.") { } } }
    actions { area(Processing) { action(Pick) { trigger OnAction() var M: Page "PH Card"; R: Record "PH Row"; Res: Action; begin
        R.Init(); R."No." := 'RESULT'; Res := M.RunModal(); R.Descr := Format(Res); R.Insert(); end; } } }
}
table 60706 "PH Nr" { fields { field(1; Id; Guid) { } field(2; Name; Code[50]) { } } keys { key(PK; Id) { } } }
page 60707 "PH NrCard"
{
    PageType = Card; SourceTable = "PH Nr"; DelayedInsert = true;
    layout { area(Content) {
        field(IdField; Rec.Id) { Visible = false; }
        field(NameField; Rec.Name) { trigger OnValidate() begin CurrPage.Update(); end; } } }
    trigger OnInsertRecord(BelowxRec: Boolean): Boolean begin Rec.Id := CreateGuid(); end;
    trigger OnAfterGetRecord() var T: Codeunit PhTrace; begin T.Note('AGR:' + Rec.Name); end;
    trigger OnAfterGetCurrRecord() var T: Codeunit PhTrace; begin T.Note('AGCR'); CurrPage.Update(false); end;
}
codeunit 60708 PhTrace { SingleInstance = true; var Log: Text; procedure Note(T: Text) begin Log += T + ';'; end; procedure Get(): Text begin exit(Log); end; procedure Reset() begin Log := ''; end; }
page 60709 "PH Ro" { PageType = Card; SourceTable = "PH Row"; layout { area(Content) { field(NoCtl; Rec."No.") { } field(NoteCtl; Note) { } } } trigger OnOpenPage() begin CurrPage.Editable(false); end; var Note: Text[20]; }
page 60710 "PH Desc" { PageType = List; SourceTable = "PH Row"; SourceTableView = sorting("No.") order(descending); layout { area(Content) { repeater(R) { field(NoCtl; Rec."No.") { } field(DescrCtl; Rec.Descr) { } } } } }
table 60704 "PH Lk" { LookupPageId = "PH Card"; fields { field(1; "No."; Code[20]) { } field(2; Descr; Text[50]) { } } keys { key(PK; "No.") { } } }
page 60705 "PH Saved" { PageType = StandardDialog; SaveValues = true; layout { area(Content) { field(Rem; RemTxt) { } } } var RemTxt: Text[30]; }
codeunit 60703 PhT
{
    Subtype = Test;
    var Seen: Text;

    // handlers/TestPageModalHandler_Tests.al:43,83 ModalPageHandlerOkReachesTheCallingAl / CancelReaches (lookup)
    [Test] [HandlerFunctions('OkModal')]
    procedure OkReachesTheCaller()
    var R: Record "PH Row"; H: TestPage "PH Host";
    begin
        R."No." := 'A'; R.Insert(); H.OpenEdit(); H.First(); H.Pick.Invoke(); H.Close();
        R.Get('RESULT'); if R.Descr <> 'OK' then Error('got %1', R.Descr);
    end;

    // handlers/TestPageModalHandler_Tests.al:105 ModalPageWithoutAHandlerIsRefused
    [Test]
    procedure NoHandlerIsRefused()
    var R: Record "PH Row"; H: TestPage "PH Host";
    begin
        R."No." := 'N1'; R.Insert(); Commit(); H.OpenEdit(); H.First();
        asserterror H.Pick.Invoke();
        if StrPos(GetLastErrorText(), 'Unhandled UI: ModalPage') = 0 then Error('got %1', GetLastErrorText());
    end;

    // handlers/TestPageRunHandler_Tests.al:60-90: the handler is handed the record Page.Run was given
    [Test] [HandlerFunctions('SeePage')]
    procedure PageRunHandsTheRecord()
    var R: Record "PH Row";
    begin
        R."No." := 'P1'; R.Descr := 'Alpha'; R.Insert(); R."No." := 'P2'; R.Descr := 'Beta'; R.Insert();
        R.Get('P2');
        Page.Run(Page::"PH Card", R);
        if Seen <> 'Beta' then Error('got %1', Seen);
    end;

    // testpageclose + handlers/TestPageModalQueryClose_Tests.al (b): a handler that invokes nothing leaves a plain modal reporting OK
    [Test] [HandlerFunctions('Nothing')]
    procedure UnattendedPlainModalReportsOk()
    var M: Page "PH Card"; Res: Action;
    begin
        Res := M.RunModal();
        if Format(Res) <> 'OK' then Error('got %1', Format(Res));
    end;

    // TestPageModalQueryClose_Tests.al (d)/(e): lookup mode cancels
    [Test] [HandlerFunctions('CancelIt')]
    procedure LookupCancelReportsLookupCancel()
    var M: Page "PH Card"; Res: Action;
    begin
        M.LookupMode(true);
        Res := M.RunModal();
        if Format(Res) <> 'LookupCancel' then Error('got %1', Format(Res));
    end;

    // TestPageModalQueryClose_Tests.al (b2): a plain modal has no built-in Cancel
    [Test] [HandlerFunctions('CancelIt')]
    procedure PlainModalHasNoCancel()
    var M: Page "PH Card"; Res: Action;
    begin
        asserterror Res := M.RunModal();
        if StrPos(GetLastErrorText(), 'The built-in action = Cancel is not found on the page') = 0 then Error('got %1', GetLastErrorText());
    end;

    // handlers/TestPageRunModalLookupPageId_Tests.al: id 0 with a record resolves LookupPageId
    [Test] [HandlerFunctions('OkModal')]
    procedure IdZeroUsesLookupPageId()
    var K: Record "PH Lk"; R: Record "PH Row"; Res: Action;
    begin
        Res := Page.RunModal(0, K);
        if Format(Res) <> 'LookupOK' then Error('got %1', Format(Res));
        asserterror Page.RunModal(0, R);
        if StrPos(GetLastErrorText(), 'You tried to invoke the Page object with the ID 0') = 0 then Error('got %1', GetLastErrorText());
    end;

    // handlers/TestPageSaveValues.al: OK stores, the next open shows it; Cancel stores nothing
    [Test] [HandlerFunctions('SaveRem')]
    procedure SaveValuesKeepsWhatOkStored()
    begin
        Seen := 'x'; Page.RunModal(Page::"PH Saved");
        Seen := ''; Page.RunModal(Page::"PH Saved");
        if Seen <> 'kept' then Error('got %1', Seen);
    end;

    // handlers/TestPageTrapSharedScope_Tests.al:87: a page still held by the caller keeps the trap
    [Test]
    procedure TrapSurvivesWhileAnotherVariableHoldsThePage()
    var T: TestPage "PH Card"; R: Record "PH Row";
    begin
        R."No." := 'TS'; R.Insert();
        TrapAndAssign(T);
        R.Get('TS');
        Page.Run(Page::"PH Card", R);
        if T.NoCtl.Value() <> 'TS' then Error('got %1', T.NoCtl.Value());
        T.Close();
    end;

    local procedure TrapAndAssign(var Out: TestPage "PH Card")
    var A: TestPage "PH Card"; B: TestPage "PH Card";
    begin
        A.Trap(); B := A; Out := B;
    end;

    // pageupdate/TestPageUpdateNewRow.al: Update() in OnValidate saves a DelayedInsert new row, then the refresh runs AGR for it
    [Test]
    procedure UpdateSavesTheNewRowAndRefreshesIt()
    var C: TestPage "PH NrCard"; R: Record "PH Nr"; T: Codeunit PhTrace;
    begin
        T.Reset(); C.OpenNew();
        if T.Get() <> 'AGCR;' then Error('open: %1', T.Get());
        T.Reset(); C.NameField.SetValue('Nx');
        if StrPos(T.Get(), 'AGR:NX;') = 0 then Error('set: %1', T.Get());
        C.Close();
        R.SetRange(Name, 'Nx'); if R.Count() <> 1 then Error('rows %1', R.Count());
    end;

    // handlers/TestPageControlEditableRoutes.al:548: CurrPage.Editable(false) in OnOpenPage narrows every control
    [Test]
    procedure CurrPageEditableFalseInOnOpenPageNarrowsEverything()
    var C: TestPage "PH Ro"; R: Record "PH Row";
    begin
        R."No." := 'E1'; R.Insert();
        C.OpenEdit();
        if C.Editable() or C.NoCtl.Editable() or C.NoteCtl.Editable() then Error('still editable');
        C.Close();
    end;

    // pagefindrecord/TestPageFindRecord.al:192: SourceTableView sorting descending; Find*Field walks the view
    [Test]
    procedure SourceTableViewOrdersTheRowsAndFindFieldWalksThem()
    var L: TestPage "PH Desc"; R: Record "PH Row";
    begin
        R."No." := 'ZD1'; R.Insert(); R."No." := 'ZD2'; R.Insert(); R."No." := 'ZD3'; R.Insert();
        L.OpenView(); L.First();
        if L.NoCtl.Value() <> 'ZD3' then Error('first %1', L.NoCtl.Value());
        if not L.FindNextField(L.NoCtl, 'ZD1') then Error('next');
        if L.NoCtl.Value() <> 'ZD1' then Error('at %1', L.NoCtl.Value());
        if L.FindFirstField(L.NoCtl, 5) then Error('an Integer must not match a Code control');
        if L.NoCtl.Value() <> 'ZD1' then Error('a miss keeps the row, at %1', L.NoCtl.Value());
        L.Close();
    end;

    [ModalPageHandler] procedure SaveRem(var P: TestPage "PH Saved")
    begin
        if Seen = 'x' then begin P.Rem.SetValue('kept'); P.OK().Invoke(); end
        else begin Seen := P.Rem.Value(); P.Cancel().Invoke(); end;
    end;
    [ModalPageHandler] procedure OkModal(var P: TestPage "PH Card") begin P.OK().Invoke(); end;
    [ModalPageHandler] procedure Nothing(var P: TestPage "PH Card") begin end;
    [ModalPageHandler] procedure CancelIt(var P: TestPage "PH Card") begin P.Cancel().Invoke(); end;
    [PageHandler] procedure SeePage(var P: TestPage "PH Card") begin Seen := P.DescrCtl.Value(); end;
}
"#;

    fn ph(name: &str) -> Outcome {
        outcome_of(&outcomes(PAGES), name)
    }

    #[test]
    fn modal_and_page_handlers_answer_page_opens() {
        for n in [
            "OkReachesTheCaller",
            "NoHandlerIsRefused",
            "PageRunHandsTheRecord",
            "UnattendedPlainModalReportsOk",
            "LookupCancelReportsLookupCancel",
            "PlainModalHasNoCancel",
            "IdZeroUsesLookupPageId",
            "SaveValuesKeepsWhatOkStored",
            "TrapSurvivesWhileAnotherVariableHoldsThePage",
            "UpdateSavesTheNewRowAndRefreshesIt",
            "CurrPageEditableFalseInOnOpenPageNarrowsEverything",
            "SourceTableViewOrdersTheRowsAndFindFieldWalksThem",
        ] {
            assert_eq!(ph(n), Outcome::Pass, "{n}");
        }
    }
}
