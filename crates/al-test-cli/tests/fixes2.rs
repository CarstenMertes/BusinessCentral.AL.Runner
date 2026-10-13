//! Gaps closed on branch fixes-2: a refused close of a modal page, and the transaction of a page
//! background worker. AL source → compile → run as a test codeunit.
//! Each test names the corpus test it mirrors.

use al_sema::workspace::AppSource;
use al_test_cli::report::Outcome;
use al_test_cli::runtime::run_codeunit;
use al_vm::Engine;

const OBJECTS: &str = r#"
table 63000 "F2 Row"
{
    fields { field(1; "No."; Code[20]) { } field(2; Val; Integer) { } }
    keys { key(PK; "No.") { } }
}
codeunit 63002 "F2 Probe"
{
    SingleInstance = true;
    var Seen: Integer; Last: Text;
    procedure Reset() begin Seen := 0; Last := ''; end;
    procedure Note(T: Text) begin Seen += 1; Last := T; end;
    procedure Count(): Integer begin exit(Seen); end;
    procedure LastText(): Text begin exit(Last); end;
}
page 63010 "F2 Veto Modal"
{
    PageType = Worksheet;
    layout { area(Content) { field(Marker; MarkerVar) { } } }
    var MarkerVar: Text[10];
    trigger OnOpenPage() var Row: Record "F2 Row"; begin Row."No." := 'OPENED'; Row.Val := 42; Row.Insert(); end;
    trigger OnQueryClosePage(CloseAction: Action): Boolean begin Error('close refused'); end;
}

codeunit 63020 "F2 Noop" { trigger OnRun() begin end; }
codeunit 63021 "F2 Failing" { trigger OnRun() begin Error('guarded failure'); end; }
codeunit 63022 "F2 Tx Worker"
{
    trigger OnRun()
    var Params: Dictionary of [Text, Text]; R: Dictionary of [Text, Text]; Mode: Text; Ok: Boolean;
    begin
        Params := Page.GetBackgroundParameters();
        if not Params.Get('Mode', Mode) then Mode := '';
        R.Add('InWriteTx', BoolText(Database.IsInWriteTransaction()));
        if Mode = 'unguarded' then Codeunit.Run(Codeunit::"F2 Noop")
        else if Mode = 'failing' then R.Add('Guarded', BoolText(Codeunit.Run(Codeunit::"F2 Failing")))
        else R.Add('Guarded', BoolText(Codeunit.Run(Codeunit::"F2 Noop")));
        R.Add('InWriteTxAfter', BoolText(Database.IsInWriteTransaction()));
        Page.SetBackgroundTaskResult(R);
    end;
    local procedure BoolText(B: Boolean): Text begin if B then exit('true'); exit('false'); end;
}
page 63011 "F2 Card"
{
    PageType = Card; SourceTable = "F2 Row";
    layout { area(Content) { field("No."; Rec."No.") { } } }
}
"#;

fn outcomes(tests: &str) -> Vec<(String, Outcome)> {
    let src = format!("{OBJECTS}\ncodeunit 63100 T {{ Subtype = Test; {tests} }}");
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

fn passes(tests: &str) {
    let o = outcomes(tests);
    assert!(!o.is_empty());
    for (n, r) in &o {
        assert_eq!(r, &Outcome::Pass, "{n}");
    }
}

const HELPERS: &str = "procedure Check(Expected: Text; Actual: Text) begin if Expected <> Actual then Error('expected <%1> got <%2>', Expected, Actual); end;";

// handlers/TestPageQueryCloseMessageHandler_Tests.al: ErrorInQueryClosePage_MessageHandlerReceivesTheTriggersText,
// _MessageConsumed_CallerRegainsControl, _MessageConsumed_SettlesThePagesUncommittedWrite.
// The modal handler's OK closes the page once (message 1, close refused); the platform closes the
// page again after the handler returns (message 2, refused again). RunModal reports None and the
// page's uncommitted write survives.
#[test]
fn a_modal_whose_query_close_page_errors_delivers_the_message_twice_and_reports_none() {
    passes(&format!(
        "{HELPERS}
        [Test] [HandlerFunctions('OkHandler,MsgHandler')]
        procedure Refused() var Row: Record \"F2 Row\"; P: Codeunit \"F2 Probe\"; M: Page \"F2 Veto Modal\"; R: Action; begin
            P.Reset(); R := M.RunModal();
            Check('2', Format(P.Count())); Check('close refused', P.LastText());
            Check(Format(Action::None), Format(R));
            if not Row.Get('OPENED') then Error('the page write must survive'); Check('42', Format(Row.Val)); end;
        [ModalPageHandler] procedure OkHandler(var T: TestPage \"F2 Veto Modal\") begin T.OK().Invoke(); end;
        [MessageHandler] procedure MsgHandler(Msg: Text[1024]) var P: Codeunit \"F2 Probe\"; begin P.Note(Msg); end;"
    ));
}

const TX_HELPERS: &str = "procedure Work(Mode: Text): Dictionary of [Text, Text] var C: TestPage \"F2 Card\"; P: Dictionary of [Text, Text]; R: Dictionary of [Text, Text]; begin
    P.Add('Mode', Mode); C.OpenView(); R := C.RunPageBackgroundTask(Codeunit::\"F2 Tx Worker\", P, false); C.Close(); exit(R); end;
procedure Seed(No: Code[20]) var Row: Record \"F2 Row\"; begin Row.DeleteAll(); Commit(); Row.\"No.\" := No; Row.Insert(); end;";

// handlers/TestPageBgTaskTx_Tests.al: RunPageBackgroundTask_WorkerIsOutsideCallersWriteTransaction
// (the worker reports no write transaction, a guarded Codeunit.Run inside it runs, the caller's
// write transaction is still open afterwards) and ..._WorkerCommitLeavesCallersRowsUncommitted
// (the caller's row is still rolled back by a later error).
#[test]
fn a_worker_is_outside_the_callers_write_transaction_and_its_guarded_run_leaves_it_alone() {
    passes(&format!(
        "{HELPERS} {TX_HELPERS}
        [Test]
        procedure Outside() var R: Dictionary of [Text, Text]; V: Text; Row: Record \"F2 Row\"; begin
            Seed('TX-1');
            if not Database.IsInWriteTransaction() then Error('precondition');
            R := Work('');
            R.Get('InWriteTx', V); Check('false', V);
            R.Get('Guarded', V); Check('true', V);
            R.Get('InWriteTxAfter', V); Check('false', V);
            if not Database.IsInWriteTransaction() then Error('the callers write transaction must stay open');
            asserterror Error('probe');
            if Row.Get('TX-1') then Error('the callers row must have been rolled back'); end;"
    ));
}

// The same separation for a guarded run that fails inside the worker: it undoes the worker's own
// writes (none), not the caller's (handlers/TestPageBgTaskTx_Tests.al:
// RunPageBackgroundTask_WorkerCommitLeavesCallersRowsUncommitted gives the separation).
#[test]
fn a_failing_guarded_run_in_a_worker_does_not_roll_back_the_callers_row() {
    passes(&format!(
        "{HELPERS} {TX_HELPERS}
        [Test]
        procedure Failing() var R: Dictionary of [Text, Text]; V: Text; Row: Record \"F2 Row\"; begin
            Seed('TX-2');
            R := Work('failing');
            R.Get('Guarded', V); Check('false', V);
            if not Row.Get('TX-2') then Error('the callers row must survive'); 
            if not Database.IsInWriteTransaction() then Error('still open'); end;"
    ));
}

// An unguarded Codeunit.Run in a worker is not pinned: refused.
#[test]
fn an_unguarded_codeunit_run_in_a_worker_is_refused() {
    let o = outcomes(&format!(
        "{HELPERS} {TX_HELPERS}
        [Test]
        procedure Unguarded() var R: Dictionary of [Text, Text]; begin R := Work('unguarded'); end;"
    ));
    assert!(
        matches!(&o[0].1, Outcome::NotRun(why) if why.contains("Codeunit.Run in a page background task")),
        "{o:?}"
    );
}

// session/TestSessionSettings.al:464-545: RequestSessionUpdate(false) is a UI call routed to the
// [SessionSettingsHandler]; the handler gets the assigned settings, the instance and the running
// session's language stay as they were.
#[test]
fn request_session_update_goes_to_the_session_settings_handler() {
    passes(&format!(
        "{HELPERS}
        var Calls: Integer; Saw: Text;
        [Test] [HandlerFunctions('SettingsHandler')]
        procedure Routed() var S: SessionSettings; Before: Integer; begin
            Calls := 0; Before := GlobalLanguage();
            S.TimeZone('Pacific Standard Time'); S.LanguageId(1030);
            S.RequestSessionUpdate(false);
            Check('1', Format(Calls)); Check('Pacific Standard Time', Saw);
            Check('1030', Format(S.LanguageId())); Check(Format(Before), Format(GlobalLanguage())); end;
        [SessionSettingsHandler] procedure SettingsHandler(var TheSettings: SessionSettings): Boolean begin
            Calls += 1; Saw := TheSettings.TimeZone(); exit(true); end;"
    ));
}

// RequestSessionUpdate(true) writes User Personalization (the corpus does not test it): refused.
#[test]
fn request_session_update_that_persists_is_refused() {
    let o = outcomes(&format!(
        "{HELPERS}
        [Test] [HandlerFunctions('SettingsHandler')]
        procedure Persist() var S: SessionSettings; begin S.RequestSessionUpdate(true); end;
        [SessionSettingsHandler] procedure SettingsHandler(var TheSettings: SessionSettings): Boolean begin exit(true); end;"
    ));
    assert!(
        matches!(&o[0].1, Outcome::NotRun(why) if why.contains("RequestSessionUpdate(true)")),
        "{o:?}"
    );
}
