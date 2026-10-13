//! Runs test codeunits the way BC's test runner does, as far as the corpus
//! pins it: one instance per test codeunit, `OnRun` once before the tests
//! (codeunit/TestCodeunitOnRun.al), tests in declaration order.

use crate::report::Outcome;
use al_builtins::native::Signal;
use al_sema::lower::Lowered;
use al_vm::{Engine, Session};
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
        let (_ws, lowered) = al_sema::lower::compile(apps, symbols);
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
    // Test-runner transaction modes are not implemented; tests in such a
    // codeunit depend on each other, so none of them runs.
    if let Some(a) = tests.iter().flat_map(|&r| &obj.routines[r].attributes).find(|a| {
        ["transactionmodel", "commitbehavior", "errorbehavior"].contains(&a.name.to_ascii_lowercase().as_str())
    }) {
        return all(Outcome::NotRun(format!("[{}] is not implemented yet", a.name)));
    }
    // What publishing the app does first: run its install codeunits, then commit.
    let skipped_installs = run_install_codeunits(lowered, &mut session);
    if let Some(on_run) = (0..obj.routines.len())
        .find(|&r| obj.routines[r].is_trigger && obj.routines[r].name.eq_ignore_ascii_case("OnRun"))
    {
        if let Some(reason) = blocked_reason(lowered, object, on_run) {
            return all(Outcome::NotRun(format!("OnRun: {reason}")));
        }
        match session.call(object, on_run, instance.clone(), Vec::new()) {
            // NavTestCodeunit.DoRunAsync commits after a declared OnRun
            // (codeunit/TestTestCodeunitOnRun.al).
            Ok(_) => al_builtins::native::Ctx::commit(&mut session).expect("commit"),
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
        let outcome = if routine
            .attributes
            .iter()
            .any(|a| a.name.eq_ignore_ascii_case("HandlerFunctions"))
        {
            Outcome::NotRun("handler functions are not supported yet".into())
        } else if let Some(reason) = blocked_reason(lowered, object, r) {
            Outcome::NotRun(reason)
        } else if let Some(reason) = skipped_installs
            .iter()
            .find(|(tables, _)| !tables.is_disjoint(&tables_touched(lowered, object, r)))
            .map(|(_, why)| why)
        {
            Outcome::NotRun(reason.clone())
        } else {
            session.last_error = Default::default();
            match session.call(object, r, instance.clone(), Vec::new()) {
                // The default TransactionModel commits at the test boundary; a failed test's
                // transaction is rolled back (corpus codeunit/TestWriteTxTestBoundary.al:26).
                Ok(_) => {
                    al_builtins::native::Ctx::commit(&mut session).expect("commit");
                    Outcome::Pass
                }
                Err(Signal::Error(e)) => {
                    session.rollback_transaction();
                    Outcome::Fail(e.message)
                }
                Err(Signal::Unsupported(r)) => Outcome::NotRun(r),
                Err(other) => Outcome::Fail(format!("unexpected signal {other:?}")),
            }
        };
        out.push((routine.name.clone(), outcome));
    }
    out
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
                for (so, sr) in subscribers_reached(lowered, oi, ri) {
                    touched.extend(tables_touched(lowered, so, sr));
                }
                skipped.push((touched, format!("install codeunit {} could not run: {why}", o.name)));
            }
        }
        al_builtins::native::Ctx::commit(session).expect("commit");
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

/// Subscriber routines (object, routine) bound to an event publisher that
/// `routine` of `object` can reach through static calls.
fn subscribers_reached(lowered: &Lowered, object: usize, routine: usize) -> Vec<(usize, usize)> {
    const EVENTS: [&str; 3] = ["IntegrationEvent", "BusinessEvent", "InternalEvent"];
    let objs = &lowered.program.objects;
    let mut published = BTreeSet::new();
    let mut seen = BTreeSet::new();
    let mut stack = vec![(object, routine)];
    while let Some((o, r)) = stack.pop() {
        if !seen.insert((o, r)) {
            continue;
        }
        let rt = &objs[o].routines[r];
        if rt
            .attributes
            .iter()
            .any(|a| EVENTS.iter().any(|e| a.name.eq_ignore_ascii_case(e)))
        {
            published.insert((o, rt.name.to_lowercase()));
        }
        stack.extend(lowered.calls[o][r].iter().copied());
    }
    let mut out = Vec::new();
    for (so, obj) in objs.iter().enumerate() {
        for (sr, r) in obj.routines.iter().enumerate() {
            let hit = r
                .attributes
                .iter()
                .filter(|a| a.name.eq_ignore_ascii_case("EventSubscriber"))
                .any(|a| {
                    let Some((target, event)) = subscriber_target(&a.text) else {
                        return false;
                    };
                    published.iter().any(|(po, ev)| {
                        let p = &objs[*po];
                        *ev == event
                            && (p.name.eq_ignore_ascii_case(&target) || p.id.is_some_and(|id| id.to_string() == target))
                    })
                });
            if hit {
                out.push((so, sr));
            }
        }
    }
    out
}

/// (publisher object name or id, lower-case event name) from
/// `[EventSubscriber(ObjectType::Codeunit, Codeunit::"Name", 'Event', ...)]`.
fn subscriber_target(text: &str) -> Option<(String, String)> {
    let args = text.split_once('(')?.1;
    let mut parts = args.split(',');
    parts.next()?;
    let object = parts.next()?.rsplit("::").next()?.trim().trim_matches('"').to_string();
    let event = parts.next()?.trim().trim_matches('\'').to_lowercase();
    Some((object, event))
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

    #[test]
    fn transaction_models_make_the_codeunit_not_run() {
        let o = outcomes(
            "codeunit 60101 T { Subtype = Test;
            [Test] [TransactionModel(TransactionModel::AutoRollback)] procedure A() begin end;
            [Test] procedure B() begin end; }",
        );
        assert!(
            o.iter()
                .all(|(_, x)| matches!(x, Outcome::NotRun(r) if r.contains("TransactionModel"))),
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
                trigger OnInstallAppPerCompany() var P: Codeunit Pub; begin P.OnSeed(); end; }
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

    #[test]
    fn handler_functions_are_not_supported_yet() {
        let o = outcomes(SRC);
        assert!(
            matches!(&o[3].1, Outcome::NotRun(r) if r.contains("handler")),
            "{:?}",
            o[3].1
        );
    }
}
