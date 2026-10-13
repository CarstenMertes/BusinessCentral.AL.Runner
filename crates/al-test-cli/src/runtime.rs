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
                    let result = session.call(object, r, instance.clone(), Vec::new());
                    let unexecuted = session.end_ui_scope();
                    match result {
                        Ok(_) if unexecuted.is_empty() => Outcome::Pass,
                        // corpus handlers/TestPageQueryCloseMessageHandler_Tests.al:51: a declared
                        // handler that never fires is a test failure. The text after the prefix
                        // is not pinned (docs/probes-needed.md).
                        Ok(_) => Outcome::Fail(format!(
                            "The following UI handlers were not executed: {}",
                            unexecuted.join(", ")
                        )),
                        Err(Signal::Error(e)) => Outcome::Fail(e.message),
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
                    "pagehandler" | "modalpagehandler" | "requestpagehandler" | "filterpagehandler" => {
                        "page handlers need TestPage support"
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
            if let Some((_, other)) = out.iter().find(|(k, _)| *k == kind) {
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
                al_data::Store::rollback(session.store_mut());
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
        assert!(
            matches!(&o[0].1, Outcome::NotRun(r) if r.contains("ModalPageHandler")),
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
}
