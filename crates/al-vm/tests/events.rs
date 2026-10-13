//! Event publishers and subscribers end to end: AL source → compile → run.
//!
//! Semantics are pinned by corpus tests, cited per test; subscriber order
//! among several subscribers is NOT pinned there, so no test depends on it.

use al_builtins::native::Signal;
use al_builtins::value::Value;
use al_sema::workspace::AppSource;
use al_vm::{Engine, Session};

fn run(src: &str, routine: &str) -> Result<Value, Signal> {
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
    let blocked: Vec<Vec<Option<String>>> = l
        .unsupported
        .iter()
        .map(|o| o.iter().map(|r| r.first().cloned()).collect())
        .collect();
    let oi = l.program.objects.iter().position(|o| o.name == "C").unwrap();
    let ri = l.program.objects[oi]
        .routines
        .iter()
        .position(|r| r.name == routine)
        .unwrap();
    if let Some(Some(r)) = blocked.get(oi).and_then(|o| o.get(ri)) {
        return Err(Signal::Unsupported(r.clone()));
    }
    let engine = Engine::new(l.program, blocked);
    let mut s = Session::new(engine);
    let inst = s.instance(oi);
    s.call(oi, ri, inst, vec![]).map(|(v, _)| v)
}

fn text(r: Result<Value, Signal>) -> String {
    match r {
        Ok(Value::Text(s)) | Ok(Value::Code(s)) => s,
        other => panic!("{other:?}"),
    }
}

fn int(r: Result<Value, Signal>) -> i32 {
    match r {
        Ok(Value::Integer(i)) => i,
        other => panic!("{other:?}"),
    }
}

fn boolean(r: Result<Value, Signal>) -> bool {
    match r {
        Ok(Value::Boolean(b)) => b,
        other => panic!("{other:?}"),
    }
}

fn error_message(r: Result<Value, Signal>) -> String {
    match r {
        Err(Signal::Error(e)) => e.message,
        other => panic!("expected an error, got {other:?}"),
    }
}

const PUB: &str = r#"
codeunit 50200 Pub
{
    [IntegrationEvent(false, false)]
    procedure OnWork(Input: Integer; var Total: Integer; var Log: Text) begin end;

    procedure Raise(Input: Integer; var Log: Text): Integer
    var Total: Integer;
    begin
        Total := 100;
        OnWork(Input, Total, Log);
        exit(Total);
    end;
}
"#;

// codeunit/TestEventContracts.al:12 (var parameters are modifiable and visible to the caller),
// codeunit/TestCodeunitEventSubscriberMutation / TestTableEventByRefMutation.al:23.
#[test]
fn a_static_subscriber_runs_and_its_var_parameters_write_back() {
    let src = format!(
        "{PUB}
codeunit 50201 Sub
{{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::Pub, 'OnWork', '', false, false)]
    local procedure H(Input: Integer; var Total: Integer; var Log: Text)
    begin
        Total += Input;
        Log += 'S;';
    end;
}}
codeunit 50100 C
{{
    procedure F(): Text
    var P: Codeunit Pub; Log: Text; T: Integer;
    begin
        T := P.Raise(7, Log);
        exit(Format(T) + '|' + Log);
    end;
}}"
    );
    assert_eq!(text(run(&src, "F")), "107|S;");
}

// codeunit/TestEventContracts.al:12 -- with no subscriber the publisher is a no-op.
#[test]
fn an_event_without_subscribers_does_nothing() {
    let src = format!(
        "{PUB}
codeunit 50100 C
{{
    procedure F(): Text
    var P: Codeunit Pub; Log: Text; T: Integer;
    begin
        T := P.Raise(7, Log);
        exit(Format(T) + '|' + Log);
    end;
}}"
    );
    assert_eq!(text(run(&src, "F")), "100|");
}

// codeunit/TestIntegrationEventFanout.al:16 -- every subscriber receives each publish.
// Order among subscribers is not pinned, so the effects below commute.
#[test]
fn every_subscriber_runs_once_per_publish() {
    let src = format!(
        "{PUB}
codeunit 50201 SubA
{{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::Pub, 'OnWork', '', false, false)]
    local procedure H(Input: Integer; var Total: Integer) begin Total += 1000; end;
}}
codeunit 50202 SubB
{{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::Pub, 'onwork', '', false, false)]
    local procedure H(var Total: Integer) begin Total += 1; end;
}}
codeunit 50100 C
{{
    procedure F(): Integer
    var P: Codeunit Pub; Log: Text;
    begin
        P.Raise(1, Log);
        exit(P.Raise(1, Log));
    end;
}}"
    );
    // A subscriber may declare any subset of the publisher's parameters (SenderPosition.al:5).
    assert_eq!(int(run(&src, "F")), 1101);
}

// codeunit/TestEventUnderscoreName.al:UnderscoredEvent_SubscriberError_Propagates
#[test]
fn a_subscriber_error_reaches_the_publishers_caller() {
    let src = format!(
        "{PUB}
codeunit 50201 Sub
{{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::Pub, 'OnWork', '', false, false)]
    local procedure H(Input: Integer) begin Error('Strict subscriber rejected %1', Input); end;
}}
codeunit 50100 C
{{
    procedure F()
    var P: Codeunit Pub; Log: Text;
    begin
        P.Raise(5, Log);
    end;
}}"
    );
    assert_eq!(error_message(run(&src, "F")), "Strict subscriber rejected 5");
}

// codeunit/TestEventUnderscoreName.al:UnderscoredEvent_RaisedFromSubscriber_ReachesItsSubscriber
#[test]
fn a_subscriber_may_raise_another_event() {
    let src = r#"
codeunit 50200 Pub
{
    [IntegrationEvent(false, false)] procedure OnOuter(var Log: Text) begin end;
    [IntegrationEvent(false, false)] procedure OnInner(var Log: Text) begin end;
    procedure Go(var Log: Text) begin OnOuter(Log); end;
    procedure GoInner(var Log: Text) begin OnInner(Log); end;
}
codeunit 50201 Sub
{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::Pub, 'OnOuter', '', false, false)]
    local procedure Outer(var Log: Text) var P: Codeunit Pub; begin Log += 'Outer;'; P.GoInner(Log); end;
    [EventSubscriber(ObjectType::Codeunit, Codeunit::Pub, 'OnInner', '', false, false)]
    local procedure Inner(var Log: Text) begin Log += 'Inner;'; end;
}
codeunit 50100 C
{
    procedure F(): Text var P: Codeunit Pub; Log: Text; begin P.Go(Log); exit(Log); end;
}"#;
    assert_eq!(text(run(src, "F")), "Outer;Inner;");
}

// Quoted event names: codeunit/TestEventQuotedName.al.
#[test]
fn quoted_event_and_procedure_names_match() {
    let src = r#"
codeunit 50200 Pub
{
    [IntegrationEvent(false, false)] local procedure "On-Check.Value (Qty) & Amt"(var Log: Text) begin end;
    procedure Go(var Log: Text) begin "On-Check.Value (Qty) & Amt"(Log); end;
}
codeunit 50201 Sub
{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::Pub, 'On-Check.Value (Qty) & Amt', '', false, false)]
    local procedure H(var Log: Text) begin Log += 'Q;'; end;
}
codeunit 50100 C
{
    procedure F(): Text var P: Codeunit Pub; Log: Text; begin P.Go(Log); exit(Log); end;
}"#;
    assert_eq!(text(run(src, "F")), "Q;");
}

const SENDER: &str = r#"
codeunit 50200 Pub
{
    var Marker: Text;
    procedure Seed(): Integer begin exit(100); end;
    procedure SetMarker(M: Text) begin Marker := M; end;
    procedure GetMarker(): Text begin exit(Marker); end;
    [IntegrationEvent(true, false)] local procedure OnFirst(var Value: Integer; Tag: Text) begin end;
    [IntegrationEvent(true, false)] local procedure OnMiddle(var Value: Integer; Tag: Text) begin end;
    [IntegrationEvent(true, false)] local procedure OnLastOmit(var Value: Integer; Tag: Text) begin end;
    procedure Go(Which: Integer; Tag: Text) Value: Integer
    begin
        Value := 10;
        case Which of
            1: OnFirst(Value, Tag);
            2: OnMiddle(Value, Tag);
            3: OnLastOmit(Value, Tag);
        end;
    end;
}
codeunit 50201 Sub
{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::Pub, 'OnFirst', '', false, false)]
    local procedure F1(var Sender: Codeunit Pub; var Value: Integer; Tag: Text) begin Value += Sender.Seed(); Sender.SetMarker(Tag); end;
    [EventSubscriber(ObjectType::Codeunit, Codeunit::Pub, 'OnMiddle', '', false, false)]
    local procedure F2(var Value: Integer; var Sender: Codeunit Pub; Tag: Text) begin Value += Sender.Seed(); Sender.SetMarker(Tag); end;
    [EventSubscriber(ObjectType::Codeunit, Codeunit::Pub, 'OnLastOmit', '', false, false)]
    local procedure F3(var Value: Integer; var Sender: Codeunit Pub) begin Value += Sender.Seed(); Sender.SetMarker('omitted'); end;
}
codeunit 50100 C
{
    procedure F(Which: Integer): Text
    var P: Codeunit Pub; V: Integer;
    begin
        V := P.Go(Which, 'T');
        exit(Format(V) + P.GetMarker());
    end;
    procedure F1(): Text begin exit(F(1)); end;
    procedure F2(): Text begin exit(F(2)); end;
    procedure F3(): Text begin exit(F(3)); end;
}"#;

// codeunit/TestCodeunitEventSenderPosition.al: the sender binds wherever it is declared,
// even when a trailing publisher parameter is omitted, and is the publishing instance.
#[test]
fn include_sender_binds_the_publisher_instance_at_any_position() {
    assert_eq!(text(run(SENDER, "F1")), "110T");
    assert_eq!(text(run(SENDER, "F2")), "110T");
    assert_eq!(text(run(SENDER, "F3")), "110omitted");
}

const ISO: &str = r#"
table 50300 Row { fields { field(1; "No."; Integer) { } field(2; Source; Text[30]) { } } keys { key(PK; "No.") { } } }
codeunit 50200 Pub
{
    [IntegrationEvent(false, false, true)] local procedure OnIsolated() begin end;
    [IntegrationEvent(false, false)] local procedure OnShared() begin end;
    procedure RaiseIsolated() begin OnIsolated(); end;
    procedure RaiseShared() begin OnShared(); end;
}
codeunit 50201 Subs
{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::Pub, 'OnIsolated', '', false, false)]
    local procedure FailsAfterWriting() var R: Record Row; begin R."No." := 1; R.Insert(); Error('ISO-SUBSCRIBER-FAILED'); end;
    [EventSubscriber(ObjectType::Codeunit, Codeunit::Pub, 'OnIsolated', '', false, false)]
    local procedure Writes() var R: Record Row; begin R."No." := 2; R.Insert(); end;
    [EventSubscriber(ObjectType::Codeunit, Codeunit::Pub, 'OnShared', '', false, false)]
    local procedure SharedFails() var R: Record Row; begin R."No." := 3; R.Insert(); Error('SHARED-SUBSCRIBER-FAILED'); end;
}
codeunit 50100 C
{
    procedure Clean() var R: Record Row; begin R.DeleteAll(); Commit(); end;
    procedure NoPendingWrite(): Text
    var P: Codeunit Pub; R: Record Row;
    begin
        Clean();
        P.RaiseIsolated();
        exit(Format(R.Get(1)) + Format(R.Get(2)) + Format(R.Count()));
    end;
    procedure PendingWrite(): Text
    var P: Codeunit Pub; R: Record Row;
    begin
        Clean();
        R."No." := 10; R.Insert();
        asserterror P.RaiseIsolated();
        exit(GetLastErrorText());
    end;
    procedure Shared(): Text
    var P: Codeunit Pub;
    begin
        Clean();
        asserterror P.RaiseShared();
        exit(GetLastErrorText());
    end;
}"#;

// codeunit/TestIsolatedEventSubscriberError.al: with nothing pending, an isolated subscriber's
// error is swallowed and its writes are rolled back, the other subscribers still run and
// keep theirs.
#[test]
fn an_isolated_subscriber_error_is_swallowed_and_only_its_writes_are_rolled_back() {
    assert_eq!(text(run(ISO, "NoPendingWrite")), "NoYes1");
}

// IsolatedEvent_WithCallerWritePending_SubscriberErrorPropagates, and SharedEvent_... (control).
#[test]
fn with_a_write_pending_or_without_isolation_the_subscriber_error_propagates() {
    assert_eq!(text(run(ISO, "PendingWrite")), "ISO-SUBSCRIBER-FAILED");
    assert_eq!(text(run(ISO, "Shared")), "SHARED-SUBSCRIBER-FAILED");
}

const MANUAL: &str = r#"
codeunit 50200 Pub
{
    [IntegrationEvent(false, false)] procedure OnBefore(EntryNo: Integer; var Handled: Boolean) begin end;
    procedure Fire(EntryNo: Integer): Boolean var Handled: Boolean; begin OnBefore(EntryNo, Handled); exit(Handled); end;
}
codeunit 50201 Manual
{
    EventSubscriberInstance = Manual;
    var FireCount: Integer; LastEntryNo: Integer;
    procedure Count(): Integer begin exit(FireCount); end;
    procedure Last(): Integer begin exit(LastEntryNo); end;
    [EventSubscriber(ObjectType::Codeunit, Codeunit::Pub, 'OnBefore', '', false, false)]
    local procedure H(EntryNo: Integer; var Handled: Boolean) begin FireCount += 1; LastEntryNo := EntryNo; Handled := true; end;
}
codeunit 50202 StaticSub
{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::Pub, 'OnBefore', '', false, false)]
    local procedure H(EntryNo: Integer) begin end;
}
codeunit 50100 C
{
    var Leaked: Codeunit Manual;
    procedure Unbound(): Boolean var P: Codeunit Pub; M: Codeunit Manual; begin exit(P.Fire(1) or (M.Count() <> 0)); end;
    procedure Bound(): Text
    var P: Codeunit Pub; M: Codeunit Manual; B: Boolean; H: Boolean;
    begin
        B := BindSubscription(M);
        H := P.Fire(2);
        exit(Format(B) + Format(H) + Format(M.Count()) + Format(M.Last()));
    end;
    procedure Unbind(): Text
    var P: Codeunit Pub; M: Codeunit Manual; U: Boolean; H: Boolean;
    begin
        BindSubscription(M);
        P.Fire(3);
        U := UnbindSubscription(M);
        H := P.Fire(4);
        exit(Format(U) + Format(H) + Format(M.Count()) + Format(M.Last()));
    end;
    procedure TwiceExpression(): Text
    var M: Codeunit Manual; A: Boolean; B: Boolean;
    begin
        A := BindSubscription(M);
        B := BindSubscription(M);
        exit(Format(A) + Format(B));
    end;
    procedure TwiceStatement() var M: Codeunit Manual; begin BindSubscription(M); BindSubscription(M); end;
    procedure BindStatic(): Boolean var S: Codeunit StaticSub; begin exit(BindSubscription(S)); end;
    procedure UnbindNeverBound(): Boolean var M: Codeunit Manual; begin exit(UnbindSubscription(M)); end;
    procedure TwoInstances(): Text
    var P: Codeunit Pub; M1: Codeunit Manual; M2: Codeunit Manual;
    begin
        BindSubscription(M1);
        BindSubscription(M2);
        P.Fire(5);
        exit(Format(M1.Count()) + Format(M2.Count()));
    end;
    procedure BindAndForgetLocal() var M: Codeunit Manual; begin BindSubscription(M); end;
    procedure FiresAfterLocalWasDropped(): Boolean var P: Codeunit Pub; begin BindAndForgetLocal(); exit(P.Fire(6)); end;
    procedure BindGlobalForget() begin BindSubscription(Leaked); end;
    procedure FiresWhileGlobalHoldsIt(): Boolean var P: Codeunit Pub; begin BindGlobalForget(); exit(P.Fire(7)); end;
    procedure PassedByValue(): Text
    var P: Codeunit Pub; M: Codeunit Manual;
    begin
        BindSubscription(M);
        exit(Format(ReadCount(M)) + Format(P.Fire(8)) + Format(ReadCount(M)));
    end;
    local procedure ReadCount(M: Codeunit Manual): Integer begin exit(M.Count()); end;
    procedure BindInCalleeLocalCopiedToCaller(): Text
    var P: Codeunit Pub; M: Codeunit Manual;
    begin
        BindAndGive(M);
        exit(Format(P.Fire(9)) + Format(M.Count()));
    end;
    local procedure BindAndGive(var Out: Codeunit Manual) var L: Codeunit Manual; begin BindSubscription(L); Out := L; end;
    procedure TwoLocalsSharingOneInstance(): Boolean var P: Codeunit Pub; begin BindShared(); exit(P.Fire(10)); end;
    local procedure BindShared() var A: Codeunit Manual; B: Codeunit Manual; begin B := A; BindSubscription(A); end;
}"#;

// codeunit/TestEventManualBinding.al Contract 1.
#[test]
fn an_unbound_manual_subscriber_never_fires() {
    assert!(!boolean(run(MANUAL, "Unbound")));
}

// Contracts 2 and 3: fires on the bound instance, with its state, until unbound.
#[test]
fn a_bound_manual_subscriber_fires_on_the_bound_instance_until_unbound() {
    assert_eq!(text(run(MANUAL, "Bound")), "YesYes12");
    assert_eq!(text(run(MANUAL, "Unbind")), "YesNo13");
}

// Contract 4 (Boolean form returns false) and Contract 8 (statement form raises).
#[test]
fn binding_the_same_instance_twice_is_false_as_a_value_and_an_error_as_a_statement() {
    assert_eq!(text(run(MANUAL, "TwiceExpression")), "YesNo");
    let m = error_message(run(MANUAL, "TwiceStatement"));
    assert!(m.contains("already been bound"), "{m}");
}

// Contract 5 and 6.
#[test]
fn binding_a_static_subscriber_is_false_and_unbinding_a_never_bound_manual_one_is_true() {
    assert!(!boolean(run(MANUAL, "BindStatic")));
    assert!(boolean(run(MANUAL, "UnbindNeverBound")));
}

// Contract 7: every bound instance fires once.
#[test]
fn each_bound_instance_of_a_manual_subscriber_fires_once() {
    assert_eq!(text(run(MANUAL, "TwoInstances")), "11");
}

// Contract 10 and TestEventManualBindingByValue.al: a binding ends with the last
// reference to the instance; a global variable keeps it.
#[test]
fn a_binding_ends_when_the_last_reference_to_the_instance_is_gone() {
    assert!(!boolean(run(MANUAL, "FiresAfterLocalWasDropped")));
    assert!(boolean(run(MANUAL, "FiresWhileGlobalHoldsIt")));
    assert_eq!(text(run(MANUAL, "PassedByValue")), "0Yes1");
    assert_eq!(text(run(MANUAL, "BindInCalleeLocalCopiedToCaller")), "Yes1");
    assert!(!boolean(run(MANUAL, "TwoLocalsSharingOneInstance")));
}

// TestEventManualBindingByValue.al:SingleInstanceLocalBoundInCallee_StillFiresAfterReturn
#[test]
fn a_single_instance_manual_subscriber_stays_bound_and_unbinds_through_any_variable() {
    let src = r#"
codeunit 50200 Pub
{
    [IntegrationEvent(false, false)] procedure OnBefore(var Handled: Boolean) begin end;
    procedure Fire(): Boolean var Handled: Boolean; begin OnBefore(Handled); exit(Handled); end;
}
codeunit 50201 Single
{
    SingleInstance = true;
    EventSubscriberInstance = Manual;
    [EventSubscriber(ObjectType::Codeunit, Codeunit::Pub, 'OnBefore', '', false, false)]
    local procedure H(var Handled: Boolean) begin Handled := true; end;
}
codeunit 50100 C
{
    procedure F(): Text
    var P: Codeunit Pub; S: Codeunit Single; First: Boolean;
    begin
        BindInCallee();
        First := P.Fire();
        UnbindSubscription(S);
        exit(Format(First) + Format(P.Fire()));
    end;
    local procedure BindInCallee() var L: Codeunit Single; begin BindSubscription(L); end;
}"#;
    assert_eq!(text(run(src, "F")), "YesNo");
}

// TestEventManualBinding.al Contract 9: a binding made through a codeunit's global
// variable survives the procedure, and a second bind in the next procedure on the same
// instance is the "already bound" error.
#[test]
fn a_binding_through_a_global_variable_survives_into_the_next_procedure_of_the_instance() {
    let src = r#"
codeunit 50201 Manual
{
    EventSubscriberInstance = Manual;
}
codeunit 50100 C
{
    var Leaked: Codeunit Manual;
    procedure A(): Boolean begin exit(BindSubscription(Leaked)); end;
    procedure B() begin BindSubscription(Leaked); end;
}"#;
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
    let blocked: Vec<Vec<Option<String>>> = l
        .unsupported
        .iter()
        .map(|o| o.iter().map(|r| r.first().cloned()).collect())
        .collect();
    let oi = l.program.objects.iter().position(|o| o.name == "C").unwrap();
    let ri = |n: &str| l.program.objects[oi].routines.iter().position(|r| r.name == n).unwrap();
    let (a, b) = (ri("A"), ri("B"));
    let engine = Engine::new(l.program, blocked);
    let mut s = Session::new(engine);
    let inst = s.instance(oi);
    assert!(matches!(
        s.call(oi, a, inst.clone(), vec![]),
        Ok((Value::Boolean(true), _))
    ));
    let r = s.call(oi, b, inst, vec![]);
    assert!(
        matches!(&r, Err(Signal::Error(e)) if e.message.contains("already been bound")),
        "{r:?}"
    );
}

// ---- table events -------------------------------------------------------------------

const TABLE: &str = r#"
codeunit 50300 Trace
{
    SingleInstance = true;
    var Log: Text;
    procedure Add(S: Text) begin if Log <> '' then Log += ','; Log += S; end;
    procedure Get(): Text begin exit(Log); end;
    procedure Clear() begin Log := ''; end;
}
table 50301 ET
{
    fields
    {
        field(1; "No."; Integer) { }
        field(2; Name; Text[30]) { }
        field(3; Watched; Text[30]) { trigger OnValidate() var T: Codeunit Trace; begin T.Add('OnValidate'); end; }
        field(4; Other; Text[30]) { }
    }
    keys { key(PK; "No.") { } }
    trigger OnInsert() var T: Codeunit Trace; begin T.Add('OnInsert'); end;
    trigger OnModify() var T: Codeunit Trace; begin T.Add('OnModify'); end;
    trigger OnDelete() var T: Codeunit Trace; begin T.Add('OnDelete'); end;
    trigger OnRename() var T: Codeunit Trace; begin T.Add('OnRename'); end;
}
table 50302 Quiet
{
    fields { field(1; "No."; Integer) { } field(2; Name; Text[30]) { } }
    keys { key(PK; "No.") { } }
}
codeunit 50310 Subs
{
    [EventSubscriber(ObjectType::Table, Database::ET, 'OnBeforeInsertEvent', '', false, false)]
    local procedure BI(var Rec: Record ET; RunTrigger: Boolean) var T: Codeunit Trace; begin T.Add('BI' + Format(RunTrigger)); end;
    [EventSubscriber(ObjectType::Table, Database::ET, 'OnAfterInsertEvent', '', false, false)]
    local procedure AI(var Rec: Record ET; RunTrigger: Boolean) var T: Codeunit Trace; begin T.Add('AI' + Format(Rec."No.")); end;
    [EventSubscriber(ObjectType::Table, Database::ET, 'OnBeforeModifyEvent', '', false, false)]
    local procedure BM(var Rec: Record ET; var xRec: Record ET; RunTrigger: Boolean) var T: Codeunit Trace; begin T.Add('BM' + xRec.Name + '>' + Rec.Name); end;
    [EventSubscriber(ObjectType::Table, Database::ET, 'OnAfterModifyEvent', '', false, false)]
    local procedure AM(var Rec: Record ET; var xRec: Record ET; RunTrigger: Boolean) var T: Codeunit Trace; begin T.Add('AM' + xRec.Name + '>' + Rec.Name); end;
    [EventSubscriber(ObjectType::Table, Database::ET, 'OnBeforeDeleteEvent', '', false, false)]
    local procedure BD(var Rec: Record ET; RunTrigger: Boolean) var T: Codeunit Trace; begin T.Add('BD' + Format(Rec."No.")); end;
    [EventSubscriber(ObjectType::Table, Database::ET, 'OnAfterDeleteEvent', '', false, false)]
    local procedure AD(var Rec: Record ET; RunTrigger: Boolean) var T: Codeunit Trace; begin T.Add('AD' + Format(Rec."No.")); end;
    [EventSubscriber(ObjectType::Table, Database::ET, 'OnBeforeRenameEvent', '', false, false)]
    local procedure BR(var Rec: Record ET; var xRec: Record ET; RunTrigger: Boolean) var T: Codeunit Trace; begin T.Add('BR' + Format(xRec."No.") + '>' + Format(Rec."No.")); end;
    [EventSubscriber(ObjectType::Table, Database::ET, 'OnAfterRenameEvent', '', false, false)]
    local procedure AR(var Rec: Record ET; var xRec: Record ET; RunTrigger: Boolean) var T: Codeunit Trace; begin T.Add('AR' + Format(xRec."No.") + '>' + Format(Rec."No.")); end;
    [EventSubscriber(ObjectType::Table, Database::ET, 'OnBeforeValidateEvent', 'Watched', false, false)]
    local procedure BV(var Rec: Record ET; var xRec: Record ET; CurrFieldNo: Integer) var T: Codeunit Trace; begin T.Add('BV' + xRec.Watched + '>' + Rec.Watched); end;
    [EventSubscriber(ObjectType::Table, Database::ET, 'OnAfterValidateEvent', 'Watched', false, false)]
    local procedure AV(var Rec: Record ET; var xRec: Record ET; CurrFieldNo: Integer) var T: Codeunit Trace; begin T.Add('AV'); end;
}
"#;

fn trace(body: &str) -> Result<Value, Signal> {
    let src = format!(
        "{TABLE}
codeunit 50100 C
{{
    procedure F(): Text
    var R: Record ET; T: Codeunit Trace;
    begin
        {body}
        exit(T.Get());
    end;
}}"
    );
    run(&src, "F")
}

// record/TestTriggerDispatchOrder.al:TriggerOrder_Insert_RecordPipeline_IsStable -- the
// Before event, then OnInsert, then the After event; record/TestTableEventDispatch.al:TableEvent_OnAfterInsert_Subscriber_Fires.
#[test]
fn insert_raises_the_before_event_then_the_trigger_then_the_after_event() {
    assert_eq!(text(trace("R.\"No.\" := 1; R.Insert(true);")), "BIYes,OnInsert,AI1");
}

// record/TestStorageLayerFieldWriteRoutes.al:StorageWrite_InsertFalse_RunsNoTrigger_ButStillWritesKeyAndStamps
// -- Insert(false) runs no trigger, but the event subscribers still run.
#[test]
fn table_events_fire_even_when_the_trigger_does_not_run() {
    assert_eq!(text(trace("R.\"No.\" := 1; R.Insert(false);")), "BINo,AI1");
}

// TriggerOrder_Modify_RecordPipeline_IsStable and TableEvent_OnAfterModify_Subscriber_Fires:
// xRec holds the stored row in both events.
#[test]
fn modify_raises_before_trigger_after_with_the_stored_row_as_xrec() {
    assert_eq!(
        text(trace(
            "R.\"No.\" := 1; R.Name := 'old'; R.Insert(false); T.Clear();
             R.Name := 'new'; R.Modify(true);"
        )),
        "BMold>new,OnModify,AMold>new"
    );
}

// TriggerOrder_Delete_RecordPipeline_IsStable
#[test]
fn delete_raises_before_trigger_after() {
    assert_eq!(
        text(trace("R.\"No.\" := 4; R.Insert(false); T.Clear(); R.Delete(true);")),
        "BD4,OnDelete,AD4"
    );
}

// TriggerOrder_Rename_RecordPipeline_IsStable and TableEvent_OnAfterRename_Subscriber_FiresWithOldAndNewKeys:
// Rec carries the new key and xRec the old one.
#[test]
fn rename_raises_before_trigger_after_with_old_and_new_keys() {
    assert_eq!(
        text(trace("R.\"No.\" := 5; R.Insert(false); T.Clear(); R.Rename(55);")),
        "BR5>55,OnRename,AR5>55"
    );
}

// TriggerOrder_Validate_FieldPipeline_IsStable (without the table-extension steps):
// OnBeforeValidateEvent, the field's OnValidate, OnAfterValidateEvent. Only the
// subscribed field raises it (record/TestNoSubscriberEvents.al, per (table, event)).
#[test]
fn validate_raises_before_on_validate_after_for_the_subscribed_field_only() {
    assert_eq!(
        text(trace("R.Validate(Watched, 'v'); R.Validate(Other, 'o');")),
        "BV>v,OnValidate,AV"
    );
}

// record/TestNoSubscriberEvents.al: a table nobody subscribes to raises nothing; with a
// subscriber on another event of the same table, an unsubscribed event raises nothing.
#[test]
fn unsubscribed_tables_raise_nothing() {
    let src = format!(
        "{TABLE}
codeunit 50100 C
{{
    procedure F(): Text
    var Q: Record Quiet; T: Codeunit Trace;
    begin
        Q.\"No.\" := 1; Q.Insert(true); Q.Name := 'x'; Q.Modify(true); Q.Delete(true);
        exit('ok' + T.Get());
    end;
}}"
    );
    assert_eq!(text(run(&src, "F")), "ok");
}

// record/TestTableEventByRefMutation.al: a Before subscriber's change to Rec is stored;
// an After subscriber's change is visible to the caller's variable but not stored.
#[test]
fn rec_is_passed_by_reference_to_table_event_subscribers() {
    let src = r#"
table 50301 ET
{
    fields { field(1; "No."; Integer) { } field(2; Name; Text[30]) { } field(3; Note; Text[30]) { } }
    keys { key(PK; "No.") { } }
}
codeunit 50310 Subs
{
    [EventSubscriber(ObjectType::Table, Database::ET, 'OnBeforeInsertEvent', '', false, false)]
    local procedure BI(var Rec: Record ET) begin Rec.Name := 'before'; end;
    [EventSubscriber(ObjectType::Table, Database::ET, 'OnAfterInsertEvent', '', false, false)]
    local procedure AI(var Rec: Record ET) begin Rec.Note := 'after'; end;
    [EventSubscriber(ObjectType::Table, Database::ET, 'OnBeforeModifyEvent', '', false, false)]
    local procedure BM(var Rec: Record ET; var xRec: Record ET) begin xRec.Note := 'xrec-mutated'; end;
    [EventSubscriber(ObjectType::Table, Database::ET, 'OnAfterModifyEvent', '', false, false)]
    local procedure AM(var Rec: Record ET; var xRec: Record ET) begin Rec.Note := xRec.Note; end;
}
codeunit 50100 C
{
    procedure F(): Text
    var R: Record ET; S: Record ET;
    begin
        R."No." := 1; R.Insert(true);
        S.Get(1);
        if (R.Note <> 'after') or (S.Note <> '') then exit('after-not-by-ref');
        R.Modify(true);
        exit(S.Name + '|' + R.Note);
    end;
}"#;
    // After Insert: the caller's R carries the After subscriber's Note, the stored row does not;
    // the xRec the Before-Modify subscriber mutated is the one the After-Modify subscriber sees.
    assert_eq!(text(run(src, "F")), "before|xrec-mutated");
}

// record/TestTableEventAsyncSubscriberError.al: an error in a Before subscriber refuses the write.
#[test]
fn an_error_in_a_before_event_subscriber_refuses_the_write() {
    let src = r#"
table 50301 ET
{
    fields { field(1; "No."; Integer) { } }
    keys { key(PK; "No.") { } }
}
codeunit 50310 Subs
{
    [EventSubscriber(ObjectType::Table, Database::ET, 'OnBeforeInsertEvent', '', false, false)]
    local procedure BI(var Rec: Record ET) begin Error('refused %1', Rec."No."); end;
}
codeunit 50100 C
{
    procedure F(): Text
    var R: Record ET; Msg: Text;
    begin
        R."No." := 3;
        asserterror R.Insert(true);
        Msg := GetLastErrorText();
        exit(Msg + '|' + Format(R.Count()));
    end;
}"#;
    assert_eq!(text(run(src, "F")), "refused 3|0");
}

// record/TestTriggerContracts.al:DeleteAll_WithTrigger_FiresForEachFilteredRecord -- DeleteAll(true)
// is one Delete(true) per matching row; that the events fire per row too is
// docs/probes-needed.md. Only the filtered rows go.
#[test]
fn delete_all_true_deletes_row_by_row_with_trigger_and_events() {
    assert_eq!(
        text(trace(
            "R.\"No.\" := 1; R.Insert(false); R.\"No.\" := 2; R.Insert(false); R.\"No.\" := 3; R.Insert(false);
             T.Clear(); R.SetRange(\"No.\", 1, 2); R.DeleteAll(true);
             R.Reset(); T.Add(Format(R.Count()));"
        )),
        "BD1,OnDelete,AD1,BD2,OnDelete,AD2,1"
    );
}

// DeleteAll(false) raises no trigger. Whether it raises the delete events is not pinned, so
// it is refused when rows would be deleted from a table with delete subscribers, and runs
// (it has nothing to do) on an empty one -- that is what the fixture cleanup relies on.
#[test]
fn delete_all_false_is_refused_only_when_it_would_delete_rows_that_have_delete_subscribers() {
    assert_eq!(text(trace("R.DeleteAll(false);")), "");
    assert!(matches!(
        trace("R.\"No.\" := 1; R.Insert(false); R.DeleteAll(false);"),
        Err(Signal::Unsupported(r)) if r.contains("DeleteAll(false)")
    ));
    let src = format!(
        "{TABLE}
codeunit 50100 C
{{
    procedure F(): Integer
    var Q: Record Quiet;
    begin
        Q.\"No.\" := 1; Q.Insert(false); Q.DeleteAll(false);
        exit(Q.Count());
    end;
}}"
    );
    assert_eq!(int(run(&src, "F")), 0);
}

// Whether table events fire for temporary records is not pinned (the one corpus case,
// record/TestTableEventAsyncSubscriberError.al, is Base App): refused, not guessed.
#[test]
fn table_events_on_temporary_records_are_refused() {
    assert!(matches!(
        trace_with("T2: Record ET temporary;", "T2.\"No.\" := 1; T2.Insert(true);"),
        Err(Signal::Unsupported(r)) if r.contains("temporary")
    ));
}

fn trace_with(vars: &str, body: &str) -> Result<Value, Signal> {
    let src = format!(
        "{TABLE}
codeunit 50100 C
{{
    procedure F(): Text
    var R: Record ET; T: Codeunit Trace; {vars}
    begin
        {body}
        exit(T.Get());
    end;
}}"
    );
    run(&src, "F")
}

// record/TestTableEventManualBinding.al Contracts 1-3 on a table event: a Manual subscriber
// to a table event fires only while bound, on the bound instance.
#[test]
fn a_manual_subscriber_to_a_table_event_fires_only_while_bound() {
    let src = r#"
table 50301 ET
{
    fields { field(1; "No."; Integer) { } }
    keys { key(PK; "No.") { } }
}
codeunit 50310 ManualSub
{
    EventSubscriberInstance = Manual;
    var Fired: Integer; LastNo: Integer;
    procedure Count(): Integer begin exit(Fired); end;
    procedure Last(): Integer begin exit(LastNo); end;
    [EventSubscriber(ObjectType::Table, Database::ET, 'OnAfterInsertEvent', '', false, false)]
    local procedure AI(var Rec: Record ET; RunTrigger: Boolean) begin Fired += 1; LastNo := Rec."No."; end;
}
codeunit 50100 C
{
    procedure F(): Text
    var R: Record ET; M: Codeunit ManualSub; Before: Integer;
    begin
        R."No." := 1; R.Insert(true);
        Before := M.Count();
        BindSubscription(M);
        R."No." := 2; R.Insert(true);
        UnbindSubscription(M);
        R."No." := 3; R.Insert(true);
        exit(Format(Before) + '|' + Format(M.Count()) + '|' + Format(M.Last()));
    end;
}"#;
    assert_eq!(text(run(src, "F")), "0|1|2");
}

// record/TestTriggerDispatchOrder.al: subscribers to the Base App's Global Triggers codeunit
// log from OnDatabaseInsert etc. in BC; that codeunit is not here, so a write to the table
// they name is refused, while tables they do not name, and a DeleteAll with nothing to
// delete, are not.
#[test]
fn writes_to_tables_watched_by_global_triggers_subscribers_are_refused() {
    let src = r#"
table 50301 Watched
{
    fields { field(1; "No."; Integer) { } }
    keys { key(PK; "No.") { } }
}
table 50302 Free
{
    fields { field(1; "No."; Integer) { } }
    keys { key(PK; "No.") { } }
}
codeunit 50310 GT
{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::"Global Triggers", 'GetDatabaseTableTriggerSetup', '', false, false)]
    local procedure Setup(TableId: Integer; var OnDatabaseInsert: Boolean; var OnDatabaseModify: Boolean; var OnDatabaseDelete: Boolean; var OnDatabaseRename: Boolean)
    begin
        if TableId <> Database::Watched then exit;
        OnDatabaseInsert := true;
    end;
}
codeunit 50100 C
{
    procedure Free(): Integer var R: Record Free; begin R."No." := 1; R.Insert(); exit(R.Count()); end;
    procedure Empty() var R: Record Watched; begin R.DeleteAll(false); end;
    procedure Write() var R: Record Watched; begin R."No." := 1; R.Insert(); end;
}"#;
    assert_eq!(int(run(src, "Free")), 1);
    assert!(run(src, "Empty").is_ok());
    assert!(matches!(run(src, "Write"), Err(Signal::Unsupported(r)) if r.contains("Global Triggers")));
}

// record/TestManualTableIntegrationEvent.al and record/TestManualTableIncludeSenderEvent.al: an
// [IntegrationEvent] declared on a table is raised from the table's own code and reaches
// its subscribers, with Rec or as Sender.
#[test]
fn events_declared_on_a_table_reach_their_subscribers() {
    let src = r#"
table 50301 Pub
{
    fields { field(1; "No."; Integer) { } field(2; Name; Text[30]) { } }
    keys { key(PK; "No.") { } }
    trigger OnDelete() begin OnAfterDeleteHook(Rec); end;
    procedure Discover() begin OnDiscover(); end;
    [IntegrationEvent(false, false)] local procedure OnAfterDeleteHook(var Rec: Record Pub) begin end;
    [IntegrationEvent(true, false)] local procedure OnDiscover() begin end;
}
tableextension 50302 PubExt extends Pub
{
    procedure RaiseExt(V: Integer) begin OnExt(V); end;
    [IntegrationEvent(false, false)] local procedure OnExt(V: Integer) begin end;
}
codeunit 50300 Trace
{
    SingleInstance = true;
    var Log: Text;
    procedure Add(S: Text) begin Log += S + ';'; end;
    procedure Get(): Text begin exit(Log); end;
}
codeunit 50310 Subs
{
    [EventSubscriber(ObjectType::Table, Database::Pub, 'OnAfterDeleteHook', '', false, false)]
    local procedure H(var Rec: Record Pub) var T: Codeunit Trace; begin T.Add('del' + Format(Rec."No.")); end;
    [EventSubscriber(ObjectType::Table, Database::Pub, 'OnDiscover', '', false, false)]
    local procedure D(var Sender: Record Pub) var T: Codeunit Trace; begin T.Add('sender' + Sender.Name); end;
    [EventSubscriber(ObjectType::Table, Database::Pub, 'OnExt', '', false, false)]
    local procedure E(V: Integer) var T: Codeunit Trace; begin T.Add('ext' + Format(V)); end;
}
codeunit 50100 C
{
    procedure F(): Text
    var R: Record Pub; T: Codeunit Trace;
    begin
        R."No." := 7; R.Name := 'N'; R.Insert(false);
        R.Delete(true);
        R.Discover();
        R.RaiseExt(5);
        exit(T.Get());
    end;
}"#;
    assert_eq!(text(run(src, "F")), "del7;senderN;ext5;");
}

// Regression guard (corpus 60150 TempTable_*, 60943 TempVar_*): a Manual subscriber nobody
// has bound does not listen, so it neither fires nor blocks writes to temporary records.
#[test]
fn an_unbound_manual_subscriber_does_not_block_temporary_records_or_duplicate_keys() {
    let src = r#"
table 50301 ET
{
    fields { field(1; "No."; Integer) { } }
    keys { key(PK; "No.") { } }
}
codeunit 50310 ManualSub
{
    EventSubscriberInstance = Manual;
    [EventSubscriber(ObjectType::Table, Database::ET, 'OnAfterInsertEvent', '', false, false)]
    local procedure AI(var Rec: Record ET; RunTrigger: Boolean) begin end;
}
codeunit 50100 C
{
    procedure F(): Text
    var T: Record ET temporary; R: Record ET;
    begin
        T."No." := 1; T.Insert(true);
        R."No." := 1; R.Insert(true);
        Commit();
        asserterror R.Insert(true);
        exit(Format(T.Count()) + Format(R.Count()) + GetLastErrorText());
    end;
}"#;
    let r = text(run(src, "F"));
    assert!(r.starts_with("11") && r.contains("already exists"), "{r}");
}

// record/TestTriggerRollback.al:OnValidate_Throws_FieldValueRolledBack -- when the field's
// OnValidate raises, the field is back at its old value.
#[test]
fn an_error_in_on_validate_puts_the_field_back() {
    let src = r#"
table 50301 ET
{
    fields
    {
        field(1; "No."; Integer) { }
        field(2; Value; Integer) { trigger OnValidate() begin Error('validate refused %1', Value); end; }
    }
    keys { key(PK; "No.") { } }
}
codeunit 50100 C
{
    procedure F(): Text
    var R: Record ET;
    begin
        R.Value := 10;
        asserterror R.Validate(Value, 99);
        exit(GetLastErrorText() + '|' + Format(R.Value));
    end;
}"#;
    assert_eq!(text(run(src, "F")), "validate refused 99|10");
}

// ---- table extension triggers -------------------------------------------------------

const EXT: &str = r#"
codeunit 50300 Trace
{
    SingleInstance = true;
    var Log: Text;
    procedure Add(S: Text) begin if Log <> '' then Log += ','; Log += S; end;
    procedure Get(): Text begin exit(Log); end;
    procedure Clear() begin Log := ''; end;
}
table 50301 ET
{
    fields { field(1; "No."; Integer) { } field(2; Name; Text[30]) { } }
    keys { key(PK; "No.") { } }
    trigger OnInsert() var T: Codeunit Trace; begin T.Add('BaseOnInsert'); end;
}
tableextension 50302 ETExt extends ET
{
    trigger OnBeforeInsert() var T: Codeunit Trace; begin T.Add('ExtBeforeInsert'); end;
    trigger OnAfterInsert() var T: Codeunit Trace; begin T.Add('ExtAfterInsert'); end;
    trigger OnBeforeModify() var T: Codeunit Trace; begin T.Add('ExtBeforeModify' + xRec.Name + '>' + Rec.Name); end;
    trigger OnAfterModify() var T: Codeunit Trace; begin T.Add('ExtAfterModify'); end;
    trigger OnBeforeDelete() var T: Codeunit Trace; begin T.Add('ExtBeforeDelete'); end;
    trigger OnAfterDelete() var T: Codeunit Trace; begin T.Add('ExtAfterDelete'); end;
    trigger OnBeforeRename() var T: Codeunit Trace; begin T.Add('ExtBeforeRename' + Format(xRec."No.") + '>' + Format(Rec."No.")); end;
    trigger OnAfterRename() var T: Codeunit Trace; begin T.Add('ExtAfterRename'); end;
}
codeunit 50310 Subs
{
    [EventSubscriber(ObjectType::Table, Database::ET, 'OnBeforeInsertEvent', '', false, false)]
    local procedure BI(var Rec: Record ET) var T: Codeunit Trace; begin T.Add('EvBeforeInsert'); end;
    [EventSubscriber(ObjectType::Table, Database::ET, 'OnAfterInsertEvent', '', false, false)]
    local procedure AI(var Rec: Record ET) var T: Codeunit Trace; begin T.Add('EvAfterInsert'); end;
}
"#;

fn ext_trace(body: &str) -> Result<Value, Signal> {
    let src = format!(
        "{EXT}
codeunit 50100 C
{{
    procedure F(): Text
    var R: Record ET; T: Codeunit Trace;
    begin
        {body}
        exit(T.Get());
    end;
}}"
    );
    run(&src, "F")
}

// tableextensiontrigger/TestTableExtensionTableTriggers_Tests.al and
// record/TestTriggerDispatchOrder.al: the extension's OnBefore/OnAfter triggers bracket the
// base table's own trigger, inside the Before and After events; Insert(false) runs none.
#[test]
fn table_extension_insert_triggers_bracket_the_base_trigger_inside_the_events() {
    assert_eq!(
        text(ext_trace("R.\"No.\" := 1; R.Insert(true);")),
        "EvBeforeInsert,ExtBeforeInsert,BaseOnInsert,ExtAfterInsert,EvAfterInsert"
    );
    assert_eq!(
        text(ext_trace("R.\"No.\" := 1; R.Insert(false);")),
        "EvBeforeInsert,EvAfterInsert"
    );
}

#[test]
fn table_extension_modify_delete_rename_triggers_run_with_xrec() {
    assert_eq!(
        text(ext_trace(
            "R.\"No.\" := 1; R.Name := 'a'; R.Insert(false); T.Clear(); R.Name := 'b'; R.Modify(true);
             R.Modify(false);"
        )),
        "ExtBeforeModifya>b,ExtAfterModify"
    );
    assert_eq!(
        text(ext_trace(
            "R.\"No.\" := 1; R.Insert(false); T.Clear(); R.Delete(true); R.\"No.\" := 2; R.Insert(false); R.Delete(false);"
        )),
        "ExtBeforeDelete,ExtAfterDelete,EvBeforeInsert,EvAfterInsert"
    );
    // Rename takes no RunTrigger flag and always runs the triggers.
    assert_eq!(
        text(ext_trace("R.\"No.\" := 1; R.Insert(false); T.Clear(); R.Rename(5);")),
        "ExtBeforeRename1>5,ExtAfterRename"
    );
}

#[test]
fn delete_all_true_runs_the_extension_triggers_per_row_and_false_runs_none() {
    assert_eq!(
        text(ext_trace(
            "R.\"No.\" := 1; R.Insert(false); R.\"No.\" := 2; R.Insert(false); T.Clear();
             R.DeleteAll(false); R.\"No.\" := 3; R.Insert(false); T.Clear(); R.DeleteAll(true);"
        )),
        "ExtBeforeDelete,ExtAfterDelete"
    );
}

// The order of two extensions' triggers is not pinned, so two declaring the same one is refused.
#[test]
fn two_extensions_declaring_the_same_trigger_are_refused() {
    let src = format!(
        "{EXT}
tableextension 50303 ETExt2 extends ET
{{
    trigger OnBeforeInsert() begin end;
}}
codeunit 50100 C
{{
    procedure F() var R: Record ET; begin R.\"No.\" := 1; R.Insert(true); end;
}}"
    );
    assert!(matches!(run(&src, "F"), Err(Signal::Unsupported(r)) if r.contains("extension")));
}

// Field triggers an extension attaches (modify(Field) { trigger OnBeforeValidate }) are still refused.
#[test]
fn table_extension_validate_triggers_are_still_refused() {
    let src = r#"
table 50301 ET { fields { field(1; "No."; Integer) { } field(2; Name; Text[30]) { } } keys { key(PK; "No.") { } } }
tableextension 50302 ETExt extends ET
{
    fields { modify(Name) { trigger OnBeforeValidate() begin end; } }
}
codeunit 50100 C { procedure F() var R: Record ET; begin R.Validate(Name, 'x'); end; }"#;
    assert!(matches!(run(src, "F"), Err(Signal::Unsupported(r)) if r.contains("extension")));
}

// record/TestXRecContracts.al:OnValidate_Nested_InnerTrigger_SeesPreOuterValidateBeforeImage -- a
// Validate nested in another Validate's trigger on the same record keeps the outer call's
// xRec (the record as it was before the outer call), and a Validate afterwards starts afresh.
#[test]
fn a_nested_validate_keeps_the_outer_calls_xrec() {
    let src = r#"
codeunit 50300 Trace
{
    SingleInstance = true;
    var Log: Text;
    procedure Add(S: Text) begin Log += S + ';'; end;
    procedure Get(): Text begin exit(Log); end;
}
table 50301 ET
{
    fields
    {
        field(1; "No."; Integer) { }
        field(2; Kind; Integer) { trigger OnValidate() begin Validate(Ref, 0); end; }
        field(3; Ref; Integer) { trigger OnValidate() var T: Codeunit Trace; begin T.Add('xKind=' + Format(xRec.Kind) + ',xRef=' + Format(xRec.Ref) + ',Ref=' + Format(Rec.Ref)); end; }
    }
    keys { key(PK; "No.") { } }
}
codeunit 50100 C
{
    procedure F(): Text
    var R: Record ET; T: Codeunit Trace;
    begin
        R.Kind := 1; R.Ref := 5;
        R.Validate(Kind, 2);
        R.Validate(Ref, 9);
        exit(T.Get());
    end;
}"#;
    // Nested: xRec.Kind is the value before the outer call (1), not its new value (2);
    // the later plain Validate(Ref) sees the record as it was then (Kind 2, Ref 0).
    assert_eq!(text(run(src, "F")), "xKind=1,xRef=5,Ref=0;xKind=2,xRef=0,Ref=9;");
}

// Mutation-testing additions (docs/status.md debt): these pin behaviour the corpus tests above
// reach only by accident. Each names the code it guards.

// A subscriber parameter that matches no event parameter is refused, with or without
// IncludeSender. Without a sender there is nothing to bind to the unmatched parameter.
#[test]
fn a_subscriber_parameter_matching_no_event_parameter_is_refused() {
    let src = format!(
        "{PUB}
codeunit 50201 Sub
{{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::Pub, 'OnWork', '', false, false)]
    local procedure H(Input: Integer; Stranger: Integer) begin end;
}}
codeunit 50100 C
{{
    procedure F(): Integer var P: Codeunit Pub; Log: Text; begin exit(P.Raise(1, Log)); end;
}}"
    );
    match run(&src, "F") {
        Err(Signal::Unsupported(m)) => assert!(m.contains("`Stranger` matches no parameter"), "{m}"),
        other => panic!("expected a refusal, got {other:?}"),
    }
}

// Only one unmatched parameter can take the sender; a second one is refused.
#[test]
fn a_second_unmatched_subscriber_parameter_is_refused_even_with_a_sender() {
    let src = r#"
codeunit 50200 Pub
{
    [IntegrationEvent(true, false)] procedure OnWork(Input: Integer) begin end;
    procedure Raise() begin OnWork(1); end;
}
codeunit 50201 Sub
{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::Pub, 'OnWork', '', false, false)]
    local procedure H(Input: Integer; S: Codeunit Pub; Extra: Integer) begin end;
}
codeunit 50100 C
{
    procedure F() var P: Codeunit Pub; begin P.Raise(); end;
}"#;
    match run(src, "F") {
        Err(Signal::Unsupported(m)) => assert!(m.contains("`Extra` matches no parameter"), "{m}"),
        other => panic!("expected a refusal, got {other:?}"),
    }
}

const STATEFUL_SUB: &str = r#"
codeunit 50200 Pub
{
    [IntegrationEvent(false, false)] procedure OnTick() begin end;
    procedure Tick() begin OnTick(); end;
}
codeunit 50201 Sub
{
    SingleInstance = true;
    var Count: Integer;
    [EventSubscriber(ObjectType::Codeunit, Codeunit::Pub, 'OnTick', '', false, false)]
    local procedure H() begin Count += 1; end;
    procedure Seen(): Integer begin exit(Count); end;
}
codeunit 50100 C
{
    procedure F(): Integer var P: Codeunit Pub; S: Codeunit Sub; begin P.Tick(); P.Tick(); exit(S.Seen()); end;
}"#;

// A static subscriber on a SingleInstance codeunit runs on that one instance: its global
// state survives from one event to the next and is what the codeunit's callers see.
// (Corpus: codeunit/TestEventContracts.al uses SingleInstance trace codeunits the same way.)
#[test]
fn a_single_instance_static_subscriber_keeps_its_globals_between_events() {
    assert_eq!(int(run(STATEFUL_SUB, "F")), 2);
}

// A non-single-instance static subscriber that keeps globals is refused, because its instance
// lifetime is not pinned. `SingleInstance = false` is not SingleInstance.
#[test]
fn a_static_subscriber_with_globals_and_single_instance_false_is_refused() {
    let src = STATEFUL_SUB.replace("SingleInstance = true;", "SingleInstance = false;");
    match run(&src, "F") {
        Err(Signal::Unsupported(m)) => assert!(m.contains("keeps global variables"), "{m}"),
        other => panic!("expected a refusal, got {other:?}"),
    }
}

// Only a *Manual* EventSubscriberInstance is manual. Naming the default, StaticAutomatic,
// still fires the subscriber without a binding, and BindSubscription on it is false.
// (codeunit/TestEventManualBinding.al Contract 5 covers the property being absent.)
#[test]
fn an_explicit_static_automatic_subscriber_fires_unbound_and_cannot_be_bound() {
    let src = r#"
codeunit 50200 Pub
{
    [IntegrationEvent(false, false)] procedure OnBefore(var Handled: Boolean) begin end;
    procedure Fire(): Boolean var Handled: Boolean; begin OnBefore(Handled); exit(Handled); end;
}
codeunit 50201 Auto
{
    EventSubscriberInstance = StaticAutomatic;
    [EventSubscriber(ObjectType::Codeunit, Codeunit::Pub, 'OnBefore', '', false, false)]
    local procedure H(var Handled: Boolean) begin Handled := true; end;
}
codeunit 50100 C
{
    procedure Fires(): Boolean var P: Codeunit Pub; begin exit(P.Fire()); end;
    procedure Bind(): Boolean var A: Codeunit Auto; begin exit(BindSubscription(A)); end;
}"#;
    assert!(boolean(run(src, "Fires")));
    assert!(!boolean(run(src, "Bind")));
}

// Unbinding a static subscriber raises nothing; as a value it is false (only "no error" is
// pinned by codeunit/TestEventContracts.al Contract 6, the value is in docs/probes-needed.md).
#[test]
fn unbinding_a_static_subscriber_does_not_raise_and_is_false_as_a_value() {
    let src = r#"
codeunit 50201 Auto { }
codeunit 50100 C
{
    procedure Stmt(): Boolean var A: Codeunit Auto; begin UnbindSubscription(A); exit(true); end;
    procedure Value(): Boolean var A: Codeunit Auto; begin exit(UnbindSubscription(A)); end;
}"#;
    assert!(boolean(run(src, "Stmt")));
    assert!(!boolean(run(src, "Value")));
}
