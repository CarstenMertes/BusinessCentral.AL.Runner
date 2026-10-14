//! Gaps found by running Microsoft's BaseApp test buckets (branch bucket-misc): notifications,
//! and the refusals that stay. AL source → compile → run as a test codeunit. Each test names the
//! corpus test it mirrors.

use al_sema::workspace::AppSource;
use al_test_cli::report::Outcome;
use al_test_cli::runtime::run_codeunit;
use al_vm::Engine;

fn outcomes_with(objects: &str, tests: &str) -> Vec<(String, Outcome)> {
    let src = format!("{objects}\ncodeunit 63100 T {{ Subtype = Test; {tests} }}");
    let (ws, l) = al_sema::lower::compile(
        vec![AppSource {
            id: "a".into(),
            name: "a".into(),
            dependencies: vec![],
            files: vec![("f.al".into(), src)],
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
    let oi = l.program.objects.iter().position(|o| o.name == "T").unwrap();
    run_codeunit(&engine, &l, oi)
}

fn outcomes(tests: &str) -> Vec<(String, Outcome)> {
    outcomes_with("", tests)
}

fn passes(tests: &str) {
    let o = outcomes(tests);
    assert!(!o.is_empty());
    for (n, r) in &o {
        assert_eq!(r, &Outcome::Pass, "{n}");
    }
}

fn not_run(tests: &str, needle: &str) {
    let o = outcomes(tests);
    assert!(matches!(&o[0].1, Outcome::NotRun(why) if why.contains(needle)), "{o:?}");
}

const HELPERS: &str = "procedure Check(Expected: Text; Actual: Text) begin if Expected <> Actual then Error('expected <%1> got <%2>', Expected, Actual); end;";

// ---------------------------------------------------------------- Notification

// handlers/TestNotificationHandler.al:15-26: Send goes to the [SendNotificationHandler], which
// gets the notification with its message; the handler answers true.
#[test]
fn send_goes_to_the_send_notification_handler() {
    passes(&format!(
        "{HELPERS}
        var Calls: Integer; Saw: Text;
        [Test] [HandlerFunctions('Handler')]
        procedure Sent() var N: Notification; begin
            Calls := 0; N.Id := CreateGuid(); N.Message('Test notification'); N.Scope(NotificationScope::LocalScope);
            N.Send(); Check('1', Format(Calls)); Check('Test notification', Saw); end;
        [SendNotificationHandler] procedure Handler(var Notif: Notification): Boolean begin
            Calls += 1; Saw := Notif.Message(); exit(true); end;"
    ));
}

// handlers/TestNotificationHandler.al:60-72: Recall after a handled Send does not throw, with
// only a send handler bound.
#[test]
fn recall_after_a_handled_send_does_not_throw() {
    passes(&format!(
        "{HELPERS}
        [Test] [HandlerFunctions('Handler')]
        procedure Recalled() var N: Notification; begin
            N.Id := CreateGuid(); N.Message('Message to recall'); N.Scope(NotificationScope::LocalScope);
            N.Send(); N.Recall(); end;
        [SendNotificationHandler] procedure Handler(var Notif: Notification): Boolean begin exit(true); end;"
    ));
}

// A declared handler that never fired fails the test (handlers/TestPageQueryCloseMessageHandler_Tests.al:51).
#[test]
fn a_send_notification_handler_that_never_fires_fails_the_test() {
    let o = outcomes(&format!(
        "{HELPERS}
        [Test] [HandlerFunctions('Handler')]
        procedure Quiet() begin end;
        [SendNotificationHandler] procedure Handler(var Notif: Notification): Boolean begin exit(true); end;"
    ));
    assert!(matches!(&o[0].1, Outcome::Fail(_)), "{o:?}");
}

// No corpus test sends without a handler, recalls a notification nobody sent, or has a handler
// answer false: refused.
#[test]
fn send_without_a_handler_is_refused() {
    not_run(
        "[Test] procedure NoHandler() var N: Notification; begin N.Message('x'); N.Send(); end;",
        "Notification.Send without a [SendNotificationHandler]",
    );
}

#[test]
fn recall_of_a_notification_nobody_sent_is_refused() {
    not_run(
        "[Test] procedure Unsent() var N: Notification; begin N.Id := CreateGuid(); N.Recall(); end;",
        "Notification.Recall of a notification that was not sent",
    );
}

#[test]
fn a_handler_that_answers_false_is_refused() {
    not_run(
        "[Test] [HandlerFunctions('Handler')]
        procedure False1() var N: Notification; begin N.Message('x'); N.Send(); end;
        [SendNotificationHandler] procedure Handler(var Notif: Notification): Boolean begin exit(false); end;",
        "only true is pinned",
    );
}
