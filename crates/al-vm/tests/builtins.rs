//! End to end: single builtins that need the compiled program (enum metadata) or session state.
//! Each test names the corpus test that pins the behaviour it asserts.

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
    let blocked = l
        .unsupported
        .iter()
        .map(|o| o.iter().map(|r| r.first().cloned()).collect())
        .collect();
    let oi = l
        .program
        .objects
        .iter()
        .position(|o| o.routines.iter().any(|r| r.name == routine))
        .expect("object");
    let ri = l.program.objects[oi]
        .routines
        .iter()
        .position(|r| r.name == routine)
        .unwrap();
    let engine = Engine::new(l.program, blocked);
    let mut s = Session::new(engine);
    let inst = s.instance(oi);
    s.call(oi, ri, inst, vec![]).map(|(v, _)| v)
}

const STATUS: &str = "enum 50101 \"ALT Status\" { value(0; \" \") { Caption = ' '; } value(1; Draft) { Caption = 'Draft'; } value(5; Closed) { Caption = 'Closed'; } }\n";

/// Runs `procedure F(): Integer var <vars> begin <body> end;` next to the enum.
fn int_of(vars: &str, body: &str) -> Result<i32, Signal> {
    let src = format!("{STATUS}codeunit 50100 C {{ procedure F(): Integer var {vars} begin {body} end; }}");
    match run(&src, "F")? {
        Value::Integer(i) => Ok(i),
        other => panic!("{other:?}"),
    }
}

// ---------------------------------------------------------------- Enum

#[test]
fn enum_names_lists_every_declared_name() {
    // types/TestEnumNamesContracts.al:19-60: `"ALT Status".Names()` holds the names, blank ordinal included.
    let v = "N: List of [Text];";
    assert_eq!(int_of(v, "N := \"ALT Status\".Names(); exit(N.Count());").unwrap(), 3);
    assert_eq!(
        int_of(
            v,
            "N := \"ALT Status\".Names(); if N.Contains('Draft') and N.Contains('Closed') then exit(1); exit(0);"
        )
        .unwrap(),
        1
    );
}

#[test]
fn enum_names_works_on_a_variable() {
    // types/TestEnumNamesContracts.al:77-85 (`S.Names()`).
    let v = "S: Enum \"ALT Status\"; N: List of [Text];";
    assert_eq!(
        int_of(v, "S := \"ALT Status\"::Draft; N := S.Names(); exit(N.Count());").unwrap(),
        3
    );
}

#[test]
fn enum_ordinals_lists_every_declared_value() {
    // session/TestSystemExtended.al:381-390 and types/TestLanguageEdgeCases.al:91-105.
    let v = "O: List of [Integer];";
    assert_eq!(
        int_of(v, "O := \"ALT Status\".Ordinals(); exit(O.Count());").unwrap(),
        3
    );
    assert_eq!(int_of(v, "O := \"ALT Status\".Ordinals(); if O.Contains(0) and O.Contains(1) and O.Contains(5) then exit(1); exit(0);").unwrap(), 1);
}

#[test]
fn names_and_ordinals_line_up() {
    // types/TestEnumNamesContracts.al:62-70: equal counts; the Nth name belongs to the Nth ordinal.
    let v = "N: List of [Text]; O: List of [Integer];";
    assert_eq!(
        int_of(v, "N := \"ALT Status\".Names(); O := \"ALT Status\".Ordinals(); if (N.Get(2) = 'Draft') and (O.Get(2) = 1) then exit(1); exit(0);").unwrap(),
        1
    );
}

#[test]
fn enum_as_integer_returns_the_ordinal() {
    // types/TestEnum.al:27-40.
    let v = "S: Enum \"ALT Status\";";
    assert_eq!(
        int_of(v, "S := \"ALT Status\"::Closed; exit(S.AsInteger());").unwrap(),
        5
    );
    assert_eq!(int_of(v, "exit(S.AsInteger());").unwrap(), 0);
}

#[test]
fn enum_from_integer_carries_any_ordinal() {
    // types/TestEnum.al:15-22 (a declared ordinal), :83-91 and types/TestEnumDefaultImplTests.Codeunit.al:70
    // (an undeclared one keeps its number); types/TestEnumShippedOrdinalTests.Codeunit.al:91 spells the
    // receiver `Enum::"Name"`.
    let v = "S: Enum \"ALT Status\";";
    assert_eq!(
        int_of(
            v,
            "S := \"ALT Status\".FromInteger(5); if S = \"ALT Status\"::Closed then exit(1); exit(0);"
        )
        .unwrap(),
        1
    );
    assert_eq!(
        int_of(v, "S := \"ALT Status\".FromInteger(99); exit(S.AsInteger());").unwrap(),
        99
    );
    assert_eq!(
        int_of(v, "S := Enum::\"ALT Status\".FromInteger(1); exit(S.AsInteger());").unwrap(),
        1
    );
}

// ---------------------------------------------------------------- NumberSequence

fn text_of(vars: &str, body: &str) -> Result<String, Signal> {
    let src = format!("codeunit 50100 C {{ procedure F(): Text var {vars} begin {body} end; }}");
    match run(&src, "F")? {
        Value::Text(s) | Value::Code(s) => Ok(s),
        other => panic!("{other:?}"),
    }
}

#[test]
fn number_sequence_honours_seed_and_increment_per_scope() {
    // session/TestNumberSequence.al:15-60: the default scope is company specific, a global one is
    // independent, and Next starts at the seed and advances by the increment.
    let r = text_of(
        "a: Text;",
        "NumberSequence.Insert('S', 5, 2);
         a := Format(NumberSequence.Exists('S', true)) + Format(NumberSequence.Exists('S', false));
         a += '|' + Format(NumberSequence.Next('S')) + ',' + Format(NumberSequence.Next('S'));
         NumberSequence.Insert('S', 100, 10, false);
         a += '|' + Format(NumberSequence.Next('S', false)) + ',' + Format(NumberSequence.Next('S', true));
         exit(a);",
    );
    assert_eq!(r.unwrap(), "YesNo|5,7|100,9");
}

#[test]
fn number_sequence_current_restart_range_and_delete() {
    // session/TestNumberSequence.al:63-170.
    let r = text_of(
        "a: Text; i: BigInteger; first: BigInteger;",
        "NumberSequence.Insert('S', 10, 3, false);
         a := Format(NumberSequence.Current('S', false));
         first := NumberSequence.Range('S', 4, false);
         a += '|' + Format(first) + ',' + Format(NumberSequence.Current('S', false)) + ',' + Format(NumberSequence.Next('S', false));
         NumberSequence.Restart('S', 50, false);
         a += '|' + Format(NumberSequence.Next('S', false)) + ',' + Format(NumberSequence.Next('S', false));
         first := NumberSequence.Range('S', 2, i, false);
         a += '|' + Format(first) + ',' + Format(i);
         NumberSequence.Delete('S', false);
         exit(a + '|' + Format(NumberSequence.Exists('S', false)));",
    );
    assert_eq!(r.unwrap(), "10|10,19,22|50,53|56,3|No");
}

#[test]
fn number_sequence_errors_are_catchable_and_a_missing_delete_is_not_one() {
    // session/TestNumberSequence.al:173-205 and error-handling/TestNumberSequenceTrappableErrors.al.
    let r = text_of(
        "a: Text; v: BigInteger;",
        "NumberSequence.Insert('S', 1, 1, false);
         asserterror NumberSequence.Insert('S', 100, 10, false);
         a := GetLastErrorText();
         NumberSequence.Delete('S', false);
         NumberSequence.Delete('S', false);
         asserterror v := NumberSequence.Next('S', false);
         a += '|' + GetLastErrorText();
         asserterror v := NumberSequence.Current('S', false);
         a += '|' + GetLastErrorText();
         asserterror NumberSequence.Restart('S', 1, false);
         a += '|' + GetLastErrorText();
         asserterror v := NumberSequence.Range('S', 3, false);
         a += '|' + GetLastErrorText();
         exit(a + '|' + Format(NumberSequence.Exists('S', false)));",
    );
    let r = r.unwrap();
    let parts: Vec<&str> = r.split('|').collect();
    assert!(parts[0].contains("already exists"), "{r}");
    assert!(parts[1..=4].iter().all(|p| p.contains("does not exist")), "{r}");
    assert_eq!(parts[5], "No");
}

#[test]
fn number_sequence_survives_an_asserterror_rollback() {
    // session/TestNumberSequence.al:173-186 and :188-201: a failed call leaves the sequence as it
    // was; error-handling/TestNumberSequenceTrappableErrors.al:134 ("Number sequences are
    // database objects, not rows"): a sequence outlives the rollback an asserterror triggers.
    let r = text_of(
        "a: Text;",
        "NumberSequence.Insert('S', 1, 1, false);
         asserterror Error('x');
         exit(Format(NumberSequence.Exists('S', false)));",
    );
    assert_eq!(r.unwrap(), "Yes");
}

#[test]
fn number_sequence_forms_the_corpus_does_not_pin_are_refused() {
    let r = text_of("", "NumberSequence.Insert('S'); exit('');");
    assert!(matches!(r, Err(Signal::Unsupported(_))), "{r:?}");
    let r = text_of(
        "",
        "NumberSequence.Insert('S', 1, 1); NumberSequence.Restart('S'); exit('');",
    );
    assert!(matches!(r, Err(Signal::Unsupported(_))), "{r:?}");
}

// ---------------------------------------------------------------- Database.IsInWriteTransaction

const ROWS: &str = "table 50900 \"ALT Row\" { fields { field(1; Id; Integer) { } } keys { key(PK; Id) { } } }\n";

fn text_with_table(body: &str) -> Result<String, Signal> {
    let src = format!("{ROWS}codeunit 50100 C {{ procedure F(): Text var R: Record \"ALT Row\"; begin {body} end; }}");
    match run(&src, "F")? {
        Value::Text(s) => Ok(s),
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_write_opens_a_write_transaction_until_the_commit() {
    // codeunit/TestWriteTxTestBoundary.al:93,103,142: an uncommitted Insert leaves one open; a
    // Commit (and the test boundary) closes it.
    let r = text_with_table(
        "exit(Format(Database.IsInWriteTransaction()));
         ",
    );
    assert_eq!(r.unwrap(), "No");
    let r = text_with_table(
        "R.Id := 1; R.Insert();
         exit(Format(Database.IsInWriteTransaction()));",
    );
    assert_eq!(r.unwrap(), "Yes");
    let r = text_with_table(
        "R.Id := 1; R.Insert(); Commit();
         exit(Format(Database.IsInWriteTransaction()));",
    );
    assert_eq!(r.unwrap(), "No");
}

// ---------------------------------------------------------------- WebServiceActionContext

fn wsac(body: &str) -> Result<String, Signal> {
    text_of(
        "c: WebServiceActionContext; d: WebServiceActionContext; a: Text; b: Boolean; v: Variant;",
        body,
    )
}

#[test]
fn wsac_fresh_instance_and_round_trips() {
    // session/TestWebServiceActionContext.al:180-250 (defaults) and :256-345 (round trips).
    let r = wsac(
        "a := Format(c.GetObjectId()) + '|' + Format(c.GetResultCode());
         c.SetObjectId(50123); c.SetObjectType(ObjectType::Page); c.SetResultCode(WebServiceActionResultCode::Created);
         a += '|' + Format(c.GetObjectId()) + ',' + Format(c.GetResultCode());
         if c.GetObjectType() = ObjectType::Page then a += ',page';
         c.SetObjectId(-7);
         exit(a + '|' + Format(c.GetObjectId()));",
    );
    assert_eq!(r.unwrap(), "0|None|50123,Created,page|-7");
}

#[test]
fn wsac_a_fresh_object_type_is_none_of_the_seven_members() {
    // session/TestWebServiceActionContext.al:187-200 and :411-428 (Clear returns it to the pristine value).
    let r = wsac(
        "if (c.GetObjectType() <> ObjectType::Table) and (c.GetObjectType() <> ObjectType::Page) and (c.GetObjectType() <> ObjectType::Report)
            and (c.GetObjectType() <> ObjectType::Codeunit) and (c.GetObjectType() <> ObjectType::XmlPort)
            and (c.GetObjectType() <> ObjectType::Query) and (c.GetObjectType() <> ObjectType::MenuSuite) then a := 'none';
         c.SetObjectType(ObjectType::Page);
         Clear(c);
         if c.GetObjectType() = d.GetObjectType() then a += ',pristine';
         exit(a);",
    );
    assert_eq!(r.unwrap(), "none,pristine");
}

#[test]
fn wsac_updated_reads_back_as_get() {
    // session/TestWebServiceActionContext.al:352-395: Get and Updated share the value 200.
    let r = wsac(
        "c.SetResultCode(WebServiceActionResultCode::Updated); a := Format(c.GetResultCode());
         c.SetResultCode(WebServiceActionResultCode::Get); a += ',' + Format(c.GetResultCode());
         c.SetResultCode(WebServiceActionResultCode::Deleted); a += ',' + Format(c.GetResultCode());
         c.SetResultCode(WebServiceActionResultCode::None); a += ',' + Format(c.GetResultCode());
         exit(a);",
    );
    assert_eq!(r.unwrap(), "Get,Get,Deleted,None");
}

#[test]
fn wsac_entity_keys_are_a_map_by_field_id() {
    // session/TestWebServiceActionContext.al:437-520: a duplicate field id returns false when the
    // result is used and raises (naming the type) when it is not; Clear empties the map.
    let r = wsac(
        "a := Format(c.AddEntityKey(1, 'x')) + Format(c.AddEntityKey(2, 42)) + Format(c.AddEntityKey(1, 'y')) + Format(c.AddEntityKey(1, 'x'));
         asserterror c.AddEntityKey(1, 'z');
         a += '|' + Format(StrPos(GetLastErrorText(), 'WebServiceActionContext') > 0);
         Clear(c);
         exit(a + '|' + Format(c.AddEntityKey(1, 'again')));",
    );
    assert_eq!(r.unwrap(), "YesYesNoNo|Yes|Yes");
}

#[test]
fn wsac_assignment_shares_the_context_and_clear_replaces_it() {
    // session/TestWebServiceActionContext.al:552-590: `Copy := Original` shares; the keys too.
    let r = wsac(
        "c.SetObjectId(50123); d := c; d.SetObjectId(50124); a := Format(c.GetObjectId());
         b := c.AddEntityKey(1, 'k');
         a += ',' + Format(d.AddEntityKey(1, 'k'));
         Clear(d);
         exit(a + ',' + Format(c.GetObjectId()) + ',' + Format(d.GetObjectId()));",
    );
    assert_eq!(r.unwrap(), "50124,No,50124,0");
}

#[test]
fn wsac_in_a_variant_has_an_empty_text() {
    // session/TestWebServiceActionContext.al:615-650.
    let r = wsac(
        "c.SetObjectId(50123); v := c;
         exit(Format(v.IsInteger()) + Format(v.IsText()) + Format(v.IsRecord()) + '|' + Format(v));",
    );
    assert_eq!(r.unwrap(), "NoNoNo|");
}

// ---------------------------------------------------------------- SessionSettings

fn settings(body: &str) -> Result<String, Signal> {
    text_of(
        "s: SessionSettings; t: SessionSettings; p: SessionSettings; a: Text; g: Guid; v: Variant;",
        body,
    )
}

#[test]
fn session_settings_properties_round_trip_and_are_independent() {
    // session/TestSessionSettings.al:25-165.
    let r = settings(
        "s.LanguageId(1030); s.LocaleId(2057); s.TimeZone('Pacific Standard Time'); s.ProfileId('ALT PROFILE');
         s.Company('ALT Company'); g := CreateGuid(); s.ProfileAppId(g);
         s.TimeZone('UTC');
         exit(Format(s.LanguageId()) + '|' + Format(s.LocaleId()) + '|' + s.TimeZone() + '|' + s.ProfileId() + '|' + s.Company() + '|' + Format(s.ProfileAppId() = g));",
    );
    assert_eq!(r.unwrap(), "1030|2057|UTC|ALT PROFILE|ALT Company|Yes");
}

#[test]
fn session_settings_profile_system_scope_ignores_its_argument() {
    // session/TestSessionSettings.al:172-215.
    let r = settings(
        "a := Format(s.ProfileSystemScope()); s.ProfileSystemScope(true); a += Format(s.ProfileSystemScope());
         s.ProfileSystemScope(false); exit(a + Format(s.ProfileSystemScope()));",
    );
    assert_eq!(r.unwrap(), "NoNoNo");
}

#[test]
fn session_settings_assignment_copies_by_value() {
    // session/TestSessionSettings.al:222-262.
    let r = settings(
        "s.Company('Source'); s.LanguageId(1030); t := s; t.Company('Target'); t.LanguageId(2057);
         exit(s.Company() + Format(s.LanguageId()) + '|' + t.Company() + Format(t.LanguageId()));",
    );
    assert_eq!(r.unwrap(), "Source1030|Target2057");
}

#[test]
fn session_settings_compare_by_value_and_format_shows_the_contents() {
    // session/TestSessionSettings.al:478-560.
    let r = settings(
        "s.Company('C'); s.LanguageId(1030); t.Company('C'); t.LanguageId(1030);
         a := Format(s = t);
         t.LanguageId(2057); a += Format(s = t);
         s.TimeZone('Pacific Standard Time'); t.TimeZone('UTC');
         a += Format(StrPos(Format(s), 'Pacific Standard Time') > 0) + Format(Format(s) = Format(t));
         v := s; 
         exit(a);",
    );
    assert_eq!(r.unwrap(), "YesNoYesNo");
}

#[test]
fn session_settings_clear_returns_to_a_pristine_instance() {
    // session/TestSessionSettings.al:565-595.
    let r = settings("s.Company('C'); s.LanguageId(1030); Clear(s); a := Format(s = p) + s.Company(); exit(a);");
    assert_eq!(r.unwrap(), "Yes");
}

#[test]
fn session_settings_forms_that_need_the_running_session_are_refused() {
    // Init() reads language, locale and time zone of the session; RequestSessionUpdate goes to a
    // handler; an unset language id is 0 or 1033 and the corpus does not say which.
    for body in [
        "s.Init();",
        "s.RequestSessionUpdate(false);",
        "if s.LanguageId() = 0 then;",
        "if s.TimeZone() = '' then;",
    ] {
        let r = settings(&format!("{body} exit('');"));
        assert!(matches!(r, Err(Signal::Unsupported(_))), "{body}: {r:?}");
    }
}

// ---------------------------------------------------------------- Notification

fn notification(body: &str) -> Result<String, Signal> {
    text_of("n: Notification; a: Text; v: Variant;", body)
}

#[test]
fn notification_message_and_data() {
    // handlers/TestNotificationHandler.al:28-35, session/TestMiscTypes.al:155-184,
    // session/TestCalcDateNotificationContracts.al:130-180.
    let r = notification(
        "n.Message('Hello'); a := n.Message();
         a += '|' + Format(n.HasData('k1'));
         n.SetData('k1', 'v1'); n.SetData('k2', 'v2');
         a += Format(n.HasData('k1')) + n.GetData('k1') + n.GetData('k2');
         n.SetData('k1', 'again');
         exit(a + n.GetData('k1'));",
    );
    assert_eq!(r.unwrap(), "Hello|NoYesv1v2again");
}

#[test]
fn notification_scope_and_actions_are_accepted() {
    // session/TestCalcDateNotificationContracts.al:182-188, session/TestSystemExtended.al:553-575,
    // session/TestMiscComplete.al:106-119: they must not throw.
    let r = notification(
        "n.Message('Test'); n.Scope(NotificationScope::LocalScope);
         n.AddAction('Click me', 60019, 'Initialize'); n.AddAction('Click Me', 60145, 'Initialize', 'Action description');
         v := n;
         exit(Format(v.IsNotification()));",
    );
    assert_eq!(r.unwrap(), "Yes");
}

#[test]
fn notification_reads_that_the_corpus_does_not_pin_are_refused() {
    for body in ["if n.GetData('missing') = '' then;", "if n.Message() = '' then;"] {
        let r = notification(&format!("{body} exit('');"));
        assert!(matches!(r, Err(Signal::Unsupported(_))), "{body}: {r:?}");
    }
}

// ---------------------------------------------------------------- ErrorInfo

const EI_ROW: &str = "table 50901 \"EI Row\" { fields { field(1; Id; Integer) { } field(2; Payload; Text[30]) { } } keys { key(PK; Id) { } } }\n";

fn error_info(body: &str) -> Result<String, Signal> {
    let src = format!(
        "{EI_ROW}codeunit 50100 C {{ procedure F(): Text var e: ErrorInfo; f: ErrorInfo; r1: Record \"EI Row\"; r2: Record \"EI Row\"; a: Text; d: Dictionary of [Text, Text]; back: Dictionary of [Text, Text]; begin {body} end; }}"
    );
    match run(&src, "F")? {
        Value::Text(s) => Ok(s),
        other => panic!("{other:?}"),
    }
}

#[test]
fn error_info_create_defaults() {
    // errorinfo/TestErrorInfoType.al:21-60: the zero-argument Create is collectible, Create(Message)
    // is not; Verbosity::Error and DataClassification::CustomerContent are the defaults.
    let r = error_info(
        "e := ErrorInfo.Create();
         a := Format(e.Verbosity()) + '|' + Format(e.DataClassification()) + '|' + Format(e.Collectible()) + '|' + e.Message() + e.Title() + e.DetailedMessage();
         f := ErrorInfo.Create('the message');
         exit(a + '|' + f.Message() + '|' + Format(f.Collectible()) + '|' + Format(f.Verbosity()) + '|' + Format(e.TableId()) + Format(e.FieldNo()) + Format(e.PageNo()));",
    );
    assert_eq!(r.unwrap(), "Error|CustomerContent|Yes||the message|No|Error|000");
}

#[test]
fn error_info_accessors_round_trip_independently() {
    // errorinfo/TestErrorInfoType.al:64-160 and :330-440.
    let r = error_info(
        "e := ErrorInfo.Create();
         e.Message('MSG'); e.Title('TITLE'); e.DetailedMessage('DETAIL'); e.ControlName('CTRL');
         e.Verbosity(Verbosity::Warning); e.DataClassification(DataClassification::SystemMetadata);
         e.PageNo(42); e.FieldNo(7); e.Collectible(false);
         e.ErrorType(ErrorType::Internal);
         a := e.Message() + e.Title() + e.DetailedMessage() + e.ControlName() + Format(e.Verbosity()) + Format(e.DataClassification());
         exit(a + Format(e.PageNo()) + Format(e.FieldNo()) + Format(e.Collectible()) + Format(e.ErrorType()));",
    );
    assert_eq!(r.unwrap(), "MSGTITLEDETAILCTRLWarningSystemMetadata427NoInternal");
}

#[test]
fn error_info_create_with_a_record_sets_table_id_record_id_and_field() {
    // errorinfo/TestErrorInfoType.al:243-326.
    let r = error_info(
        "r1.Id := 1; r1.Insert(); r2.Id := 2; r2.Insert();
         e := ErrorInfo.Create('m', true, r1, r1.FieldNo(Payload));
         a := Format(e.TableId() = Database::\"EI Row\") + Format(Format(e.RecordId()) = Format(r1.RecordId())) + Format(Format(e.RecordId()) = Format(r2.RecordId()));
         exit(a + Format(e.FieldNo()) + Format(e.PageNo()));",
    );
    assert_eq!(r.unwrap(), "YesYesNo20");
}

#[test]
fn error_info_custom_dimensions_round_trip() {
    // errorinfo/TestErrorInfoType.al:313-345.
    let r = error_info(
        "e := ErrorInfo.Create(); a := Format(e.CustomDimensions().Count());
         d.Add('alpha', 'one'); d.Add('beta', 'two'); e.CustomDimensions(d);
         back := e.CustomDimensions();
         exit(a + Format(back.Count()) + back.Get('alpha') + back.Get('beta'));",
    );
    assert_eq!(r.unwrap(), "02onetwo");
}

#[test]
fn error_info_actions_do_not_disturb_the_state() {
    // errorinfo/TestErrorInfoType.al:523-557.
    let r = error_info(
        "e := ErrorInfo.Create('an error', true); e.Title('a title');
         e.AddAction('Fix it', 50100, 'F'); e.AddNavigationAction('Show me');
         exit(e.Message() + e.Title() + Format(e.Collectible()));",
    );
    assert_eq!(r.unwrap(), "an errora titleYes");
}

// ---------------------------------------------------------------- small builtins

#[test]
fn a_progress_dialog_is_a_headless_no_op() {
    // handlers/TestHandlersDialogProgressNoOp.al:24-110: Open, Update (all forms) and Close run and
    // change nothing.
    let src = "codeunit 50100 C { var Window: Dialog; procedure F(): Integer var i: Integer; total: Integer; begin
        Window.Open('Processing record #1######');
        for i := 1 to 5 do begin Window.Update(1, i); Window.Update(); total += i; end;
        Window.Close();
        exit(total);
    end; }";
    assert!(matches!(run(src, "F"), Ok(Value::Integer(15))));
}

#[test]
fn select_latest_version_does_nothing() {
    // session/TestBCDatabaseContracts.al:170-187, session/TestMiscComplete.al:36-55.
    let r = text_of("", "SelectLatestVersion(); SelectLatestVersion(60000); exit('ok');");
    assert_eq!(r.unwrap(), "ok");
}

#[test]
fn company_name_is_the_company_of_the_session() {
    // session/TestSessionFunctions.al:25 (not empty), session/TestDatabase.al:69-75 (same as CurrentCompany).
    let src = "table 50900 \"ALT Row\" { fields { field(1; Id; Integer) { } } keys { key(PK; Id) { } } }
        codeunit 50100 C { procedure F(): Text var R: Record \"ALT Row\"; begin exit(Format(CompanyName() = R.CurrentCompany()) + Format(CompanyName() <> '')); end; }";
    assert!(matches!(run(src, "F"), Ok(Value::Text(ref s)) if s == "YesYes"));
}

#[test]
fn change_company_to_the_session_company_succeeds_and_other_companies_are_refused() {
    // record/TestSecurityFilteringContracts.al:108-138, record/TestRecordChangeCompanyRows.al:115:
    // `Rec.ChangeCompany(CompanyName())` is true and the rows stay visible. Another company needs
    // the Company table (outside the workspace).
    let src = "table 50900 \"ALT Row\" { fields { field(1; Id; Integer) { } } keys { key(PK; Id) { } } }
        codeunit 50100 C { procedure F(): Text var R: Record \"ALT Row\"; B: Record \"ALT Row\"; ok: Boolean; begin
            R.Id := 1; R.Insert();
            ok := B.ChangeCompany(CompanyName());
            exit(Format(ok) + Format(B.Count()) + B.CurrentCompany());
        end; procedure G() var R: Record \"ALT Row\"; ok: Boolean; begin ok := R.ChangeCompany('Some Other Company'); end; }";
    assert!(matches!(run(src, "F"), Ok(Value::Text(ref s)) if s.starts_with("Yes1")));
    assert!(matches!(run(src, "G"), Err(Signal::Unsupported(_))));
}

#[test]
fn a_sequential_guid_is_not_null_and_not_repeated() {
    // session/TestSystemExtended.al:306-328.
    let r = text_of(
        "a: Guid; b: Guid;",
        "a := Guid.CreateSequentialGuid(); b := Guid.CreateSequentialGuid(); exit(Format(IsNullGuid(a)) + Format(a = b));",
    );
    assert_eq!(r.unwrap(), "NoNo");
}

#[test]
fn text_builder_max_capacity_is_large() {
    // session/TestSystemExtended.al:450-473: positive and at least a million.
    let r = text_of("tb: TextBuilder;", "exit(Format(tb.MaxCapacity() >= 1000000));");
    assert_eq!(r.unwrap(), "Yes");
}

#[test]
fn a_version_is_read_by_evaluate_and_split_in_parts() {
    // session/TestMiscTypes.al:44-80.
    let r = text_of(
        "v: Version; w: Version;",
        "Evaluate(v, '1.2.3.4'); Evaluate(w, '5.6.7.8');
         exit(Format(v.Major()) + Format(v.Minor()) + Format(v.Build()) + Format(v.Revision()) + '|' + Format(w.Revision()));",
    );
    assert_eq!(r.unwrap(), "1234|8");
}

// ---------------------------------------------------------------- DataTransfer, CodeCoverageLog

fn transfer(body: &str) -> Result<String, Signal> {
    let src = format!(
        "table 50900 \"DT Src\" {{ fields {{ field(1; Id; Integer) {{ }} }} keys {{ key(PK; Id) {{ }} }} }}
         table 50901 \"DT Dst\" {{ fields {{ field(1; Id; Integer) {{ }} }} keys {{ key(PK; Id) {{ }} }} }}
         codeunit 50100 C {{ procedure F(): Text var DT: DataTransfer; a: Text; begin {body} end; }}"
    );
    match run(&src, "F")? {
        Value::Text(s) => Ok(s),
        other => panic!("{other:?}"),
    }
}

#[test]
fn data_transfer_outside_upgrade_says_which_of_the_two_things_is_wrong() {
    // install/TestDataTransferContracts.al:44-160: a call before SetTables asks for SetTables; after it,
    // every staging or copying call says the transfer is for upgrade and install code only.
    let r = transfer(
        "asserterror DT.AddFieldValue(1, 1); a := GetLastErrorText();
         asserterror DT.CopyRows(); a += '|' + GetLastErrorText();
         DT.SetTables(Database::\"DT Src\", Database::\"DT Dst\");
         asserterror DT.AddFieldValue(1, 1); a += '|' + GetLastErrorText();
         asserterror DT.AddConstantValue('x', 1); a += '|' + GetLastErrorText();
         asserterror DT.AddSourceFilter(1, '=%1', 5); a += '|' + GetLastErrorText();
         asserterror DT.AddDestinationFilter(1, '=%1', 5); a += '|' + GetLastErrorText();
         asserterror DT.AddJoin(1, 1); a += '|' + GetLastErrorText();
         asserterror DT.CopyFields(); a += '|' + GetLastErrorText();
         asserterror DT.CopyRows(); a += '|' + GetLastErrorText();
         exit(a);",
    );
    let first = "SetTables must first be called before calling other methods on DataTransfer.";
    let later = "DataTransfer is only usable during upgrade and installation code.";
    let want = format!("{first}|{first}|{later}|{later}|{later}|{later}|{later}|{later}|{later}");
    assert_eq!(r.unwrap(), want);
}

#[test]
fn data_transfer_with_a_table_outside_the_workspace_is_refused() {
    let r = transfer("DT.SetTables(2000000001, Database::\"DT Dst\"); exit('');");
    assert!(matches!(r, Err(Signal::Unsupported(_))), "{r:?}");
}

#[test]
fn code_coverage_log_reports_the_requested_state() {
    // codeunit/TestCodeCoverageStartStop.al:30-102.
    let r = text_of(
        "a: Text; b: Boolean;",
        "a := Format(CodeCoverageLog());
         b := CodeCoverageLog(false, false); a += Format(b) + Format(CodeCoverageLog());
         b := CodeCoverageLog(true, false); a += Format(b) + Format(CodeCoverageLog());
         b := CodeCoverageLog(false, false); a += Format(b) + Format(CodeCoverageLog());
         b := CodeCoverageLog(true); a += Format(b) + Format(CodeCoverageLog());
         CodeCoverageLog(false);
         exit(a + Format(CodeCoverageLog()));",
    );
    assert_eq!(r.unwrap(), "NoNoNoYesYesNoNoYesYesNo");
}

// ---------------------------------------------------------------- Clear, conversions

#[test]
fn clear_empties_lists_and_dictionaries_also_through_a_var_parameter() {
    // types/TestClearContracts.al:130-150, collections/TestListByRefContracts.al:155,
    // collections/TestCollectionsDictionary.al:108.
    let src = "codeunit 50100 C {
        procedure F(): Text var l: List of [Integer]; d: Dictionary of [Text, Integer]; a: Text; begin
            l.Add(1); l.Add(2); d.Add('a', 1);
            Clear(l); Clear(d);
            a := Format(l.Count()) + Format(d.Count());
            l.Add(5); d.Add('a', 7);
            ClearViaVar(l);
            exit(a + Format(l.Count()) + Format(d.Count()));
        end;
        local procedure ClearViaVar(var l: List of [Integer]) begin Clear(l); end; }";
    assert!(matches!(run(src, "F"), Ok(Value::Text(ref s)) if s == "0001"));
}

#[test]
fn a_text_assigns_into_a_guid() {
    // record/TestRecordGetBySystemId.al ZeroGuid, install and system-id fixtures.
    let r = text_of(
        "g: Guid;",
        "g := '00000000-0000-0000-0000-000000000000'; exit(Format(IsNullGuid(g)) + Format(g));",
    );
    assert_eq!(r.unwrap(), "Yes{00000000-0000-0000-0000-000000000000}");
}

#[test]
fn datetime_plus_and_minus_durations() {
    // types/TestDurationContracts.al:44-171.
    let r = text_of(
        "a: DateTime; b: DateTime; d: Duration;",
        "a := CreateDateTime(20240115D, 100000T); d := 7200000; b := a + d;
         exit(Format(DT2Time(b) = 120000T) + Format((b - a) = d) + Format(DT2Time(b - d) = 100000T));",
    );
    assert_eq!(r.unwrap(), "YesYesYes");
}

// ---------------------------------------------------------------- IsolatedStorage

fn storage(body: &str) -> Result<String, Signal> {
    text_of(
        "v: Text; w: Text; s: SecretText; a: Text; secret: Boolean; ok: Boolean;",
        body,
    )
}

#[test]
fn isolated_storage_round_trips_and_deletes() {
    // session/TestIsolatedStorage.al:30-65 and :95-125.
    let r = storage(
        "a := Format(IsolatedStorage.Set('k', 'value')) + Format(IsolatedStorage.Contains('k'));
         a += Format(IsolatedStorage.Get('k', v)) + v;
         a += '|' + Format(IsolatedStorage.Delete('k')) + Format(IsolatedStorage.Contains('k'));
         a += Format(IsolatedStorage.Get('absent', w)) + '[' + w + ']';
         exit(a);",
    );
    assert_eq!(r.unwrap(), "YesYesYesvalue|YesNoNo[]");
}

#[test]
fn isolated_storage_scopes_are_apart_and_encrypted_entries_read_back_as_text() {
    // session/TestIsolatedStorage.al:97-110, session/TestIsolatedStoragePerApp.al:100-110 (the Company scope).
    let r = storage(
        "IsolatedStorage.Set('k', 'module'); IsolatedStorage.Set('k', 'company', DataScope::Company);
         ok := IsolatedStorage.Get('k', v); a := v;
         ok := IsolatedStorage.Get('k', DataScope::Company, v); a += ',' + v;
         ok := IsolatedStorage.SetEncrypted('e', 'secret value');
         ok := IsolatedStorage.Contains('e', DataScope::Module, secret);
         ok := IsolatedStorage.Get('e', v);
         exit(a + ',' + v + Format(secret));",
    );
    assert_eq!(r.unwrap(), "module,company,secret valueYes");
}

#[test]
fn isolated_storage_takes_secret_text_and_refuses_the_user_scopes() {
    let r = storage("s := 'hidden'; IsolatedStorage.Set('k', s); ok := IsolatedStorage.Get('k', s); exit(Format(ok));");
    assert_eq!(r.unwrap(), "Yes");
    let r = storage("IsolatedStorage.Set('k', 'x', DataScope::User); exit('');");
    assert!(matches!(r, Err(Signal::Unsupported(_))), "{r:?}");
}

// ---------------------------------------------------------------- NavApp module info

const DEP_ID: &str = "f1e2d3c4-b5a6-7890-fedc-ba9876543210";
const MAIN_ID: &str = "a1b2c3d4-e5f6-7890-abcd-ef1234567890";

/// Two apps: `dep` (a callee codeunit) and `main` (depends on it, holds the test code).
fn run_two_apps(dep_src: &str, main_src: &str, routine: &str) -> Result<Value, Signal> {
    let (ws, mut l) = al_sema::lower::compile(
        vec![
            AppSource {
                id: DEP_ID.into(),
                name: "Dependency".into(),
                dependencies: vec![],
                files: vec![("d.al".into(), dep_src.into())],
            },
            AppSource {
                id: MAIN_ID.into(),
                name: "Main".into(),
                dependencies: vec![DEP_ID.into(), "63ca2fa4-4f03-4f2b-a480-172fef340d3f".into()],
                files: vec![("m.al".into(), main_src.into())],
            },
        ],
        &Default::default(),
    );
    assert!(ws.diagnostics.is_empty(), "{:?}", ws.diagnostics);
    // The runner patches what only the manifests know.
    l.program.apps[0].publisher = "Dep Publisher".into();
    l.program.apps[0].version = "2.3.4.5".into();
    l.program.apps[1].publisher = "Main Publisher".into();
    l.program.apps[1].version = "1.0.0.0".into();
    l.program.apps[1].resources = vec![
        ("greeting.txt".into(), b"Hello from packaged resource".to_vec()),
        ("sub/nested.txt".into(), b"nested-resource-content".to_vec()),
        ("data.json".into(), b"{\"a\": 1}".to_vec()),
    ];
    let blocked = l
        .unsupported
        .iter()
        .map(|o| o.iter().map(|r| r.first().cloned()).collect())
        .collect();
    let oi = l
        .program
        .objects
        .iter()
        .position(|o| o.routines.iter().any(|r| r.name == routine))
        .expect("object");
    let ri = l.program.objects[oi]
        .routines
        .iter()
        .position(|r| r.name == routine)
        .unwrap();
    let engine = Engine::new(l.program, blocked);
    let mut s = Session::new(engine);
    let inst = s.instance(oi);
    s.call(oi, ri, inst, vec![]).map(|(v, _)| v)
}

const CALLEE: &str = "codeunit 60384 Callee {
    procedure OwnName(): Text var i: ModuleInfo; begin NavApp.GetCurrentModuleInfo(i); exit(i.Name()); end;
    procedure OwnId(): Guid var i: ModuleInfo; begin NavApp.GetCurrentModuleInfo(i); exit(i.Id()); end;
    procedure CallerName(): Text var i: ModuleInfo; begin NavApp.GetCallerModuleInfo(i); exit(i.Name()); end;
    procedure CallerInfos(): List of [ModuleInfo] begin exit(NavApp.GetCallerCallstackModuleInfos()); end;
    procedure CallerInfosThroughOwnFrame(): List of [ModuleInfo] begin exit(Inner()); end;
    local procedure Inner(): List of [ModuleInfo] begin exit(NavApp.GetCallerCallstackModuleInfos()); end;
}";

fn module_text(body: &str) -> Result<String, Signal> {
    let main = format!(
        "codeunit 50100 T {{ procedure F(): Text var c: Codeunit Callee; i: ModuleInfo; j: ModuleInfo; v: Version; a: Text; t2: Text; ids: List of [ModuleInfo]; g: Guid; ins: InStream; o: JsonObject; names: List of [Text]; begin {body} end; }}"
    );
    match run_two_apps(CALLEE, &main, "F")? {
        Value::Text(s) => Ok(s),
        other => panic!("{other:?}"),
    }
}

#[test]
fn module_info_reports_the_manifest_of_the_running_app() {
    // session/TestBCPlatformContracts.al:100-130 (Id, Name, AppVersion match app.json),
    // session/TestMiscTypes.al:100-150 (AppVersion, PackageId, DataVersion), TestNavAppExtended.al.
    let r = module_text(
        "NavApp.GetCurrentModuleInfo(i);
         v := i.AppVersion();
         Evaluate(g, '{a1b2c3d4-e5f6-7890-abcd-ef1234567890}');
         exit(i.Name() + '|' + i.Publisher() + '|' + Format(i.Id() = g) + '|'
              + Format(v.Major()) + Format(v.Minor()) + Format(v) + '|' + Format(IsNullGuid(i.PackageId())) + Format(i.PackageId() = i.Id())
              + '|' + Format(i.Dependencies().Count()));",
    );
    assert_eq!(r.unwrap(), "Main|Main Publisher|Yes|101.0.0.0|NoNo|2");
}

#[test]
fn get_module_info_finds_installed_apps_and_names_an_unknown_id() {
    // session/TestNavAppExtended.al:94-150.
    let r = module_text(
        "NavApp.GetCurrentModuleInfo(i);
         a := Format(NavApp.GetModuleInfo(i.Id(), j)) + Format(j.Name() = i.Name());
         a += Format(NavApp.GetModuleInfo('00000000-dead-beef-0000-000000000004', j));
         ClearLastError();
         asserterror NavApp.GetModuleInfo('00000000-dead-beef-0000-000000000002', j);
         exit(a + '|' + GetLastErrorText());",
    );
    let r = r.unwrap();
    assert!(r.starts_with("YesYesNo|"), "{r}");
    assert!(r.contains("No installed extension was found with ID"), "{r}");
    assert!(r.contains("00000000-dead-beef-0000-000000000002"), "{r}");
}

#[test]
fn a_manifest_dependency_outside_the_workspace_is_refused_not_called_missing() {
    let r = module_text("NavApp.GetModuleInfo('63ca2fa4-4f03-4f2b-a480-172fef340d3f', j); exit('');");
    assert!(matches!(r, Err(Signal::Unsupported(_))), "{r:?}");
}

#[test]
fn the_callee_in_another_app_sees_its_own_module_and_its_caller() {
    // session/TestNavAppModuleInfoCallerIdentity.al:35-100 and TestNavAppCallerCallstackModuleInfos.al:25-100.
    let r = module_text(
        "NavApp.GetCurrentModuleInfo(i);
         a := c.OwnName() + ',' + c.CallerName() + '|';
         ids := c.CallerInfos();
         j := ids.Get(1);
         a += Format(ids.Count() >= 1) + Format(j.Id() = i.Id()) + '|';
         ids := c.CallerInfosThroughOwnFrame();
         a += Format(ids.Count()) + ',' + Format(ids.Get(1).Id() = c.OwnId()) + Format(ids.Get(2).Id() = i.Id());
         exit(a);",
    );
    assert_eq!(r.unwrap(), "Dependency,Main|YesYes|2,YesYes");
}

#[test]
fn is_installing_is_false_in_test_code() {
    // session/TestNavAppExtended.al, session/TestBCPlatformContracts.al:IsInstalling.
    let r = module_text("exit(Format(NavApp.IsInstalling()));");
    assert_eq!(r.unwrap(), "No");
}

#[test]
fn packaged_resources_read_back_exactly() {
    // session/TestNavAppGetResource.al:25-60.
    let r = module_text(
        "NavApp.GetResource('greeting.txt', ins, TextEncoding::UTF8); ins.ReadText(a);
         NavApp.GetResource('sub/nested.txt', ins, TextEncoding::UTF8); ins.ReadText(t2);
         exit(a + '|' + t2 + '|' + NavApp.GetResourceAsText('greeting.txt', TextEncoding::UTF8) + '|' + NavApp.GetResourceAsText('greeting.txt'));",
    );
    assert_eq!(
        r.unwrap(),
        "Hello from packaged resource|nested-resource-content|Hello from packaged resource|Hello from packaged resource"
    );
}

#[test]
fn a_missing_resource_is_named_in_the_error() {
    // session/TestNavAppGetResource.al:62-85: "A resource matching 'missing.txt' could not be found in app".
    let r = module_text(
        "asserterror NavApp.GetResource('missing.txt', ins); a := GetLastErrorText();
         asserterror t2 := NavApp.GetResourceAsText('missing.txt'); a += '|' + GetLastErrorText();
         asserterror o := NavApp.GetResourceAsJson('nonexistent.json'); a += '|' + GetLastErrorText();
         exit(a);",
    );
    let r = r.unwrap();
    for part in r.split('|') {
        assert!(part.starts_with("A resource matching '"), "{r}");
        assert!(part.contains("' could not be found in app"), "{r}");
    }
}

#[test]
fn resources_are_listed_with_an_optional_wildcard_filter() {
    // session/TestMiscComplete.al:139-165 (they must succeed); the filter is the documented wildcard.
    let r = module_text(
        "names := NavApp.ListResources(); a := Format(names.Count());
         names := NavApp.ListResources('*.json'); a += Format(names.Count()) + names.Get(1);
         names := NavApp.ListResources('*.xml');
         exit(a + Format(names.Count()));",
    );
    assert_eq!(r.unwrap(), "31data.json0");
}

// ---------------------------------------------------------------- Error(ErrorInfo), ExecutionContext

#[test]
fn error_with_an_error_info_raises_its_message() {
    // errorinfo/TestErrorInfoErrorType.al:50-65 (a client error reports the message verbatim).
    let r = text_of(
        "e: ErrorInfo; a: Text;",
        "e := ErrorInfo.Create('the client visible text'); e.ErrorType(ErrorType::Client);
         asserterror Error(e); a := GetLastErrorText();
         e.ErrorType(ErrorType::Internal); asserterror Error(e);
         exit(a);",
    );
    assert!(matches!(r, Err(Signal::Unsupported(_))), "{r:?}");
    let r = text_of(
        "e: ErrorInfo;",
        "e := ErrorInfo.Create('the client visible text'); e.ErrorType(ErrorType::Client);
         asserterror Error(e); exit(GetLastErrorText());",
    );
    assert_eq!(r.unwrap(), "the client visible text");
}

#[test]
fn the_execution_context_is_normal_in_a_test() {
    // install/TestInstallEnvVisible_Tests.al:120-127, session/TestBCPlatformContracts.al:46-60.
    let r = text_of(
        "ec: ExecutionContext;",
        "ec := Session.GetExecutionContext();
         exit(Format(ec) + Format(Session.GetExecutionContext() = ExecutionContext::Normal)
              + Format(Session.GetCurrentModuleExecutionContext() = ExecutionContext::Normal)
              + Format(Session.GetModuleExecutionContext() = ExecutionContext::Normal));",
    );
    assert_eq!(r.unwrap(), "NormalYesYesYes");
}

// ---------------------------------------------------------------- collection errors

#[test]
fn list_get_outside_the_list_raises_the_invalid_argument_error() {
    // collections/TestListTrappableError.al:21-45: both forms of Get raise it.
    let r = text_of(
        "l: List of [Integer]; v: Integer; a: Text;",
        "asserterror l.Get(5, v); a := GetLastErrorText(); asserterror v := l.Get(5); exit(a + '|' + GetLastErrorText());",
    );
    let want = "An invalid argument was passed to a 'List' data type method.";
    assert_eq!(r.unwrap(), format!("{want}|{want}"));
}

#[test]
fn list_get_outside_the_list_with_the_result_used_still_answers_false() {
    let r = text_of("l: List of [Integer]; v: Integer;", "exit(Format(l.Get(5, v)));");
    assert_eq!(r.unwrap(), "No");
}

#[test]
fn dictionary_get_of_a_missing_key_raises_the_key_not_found_error() {
    // collections/TestCollectionsDictionary.al:42-55 pins the statement form; the value form
    // (collections/TestCollectionContracts.al:188) is pinned as an error with a text (probes-needed).
    let r = text_of(
        "d: Dictionary of [Text, Integer]; v: Integer; a: Text;",
        "d.Add('alpha', 11); asserterror d.Get('missing', v); a := GetLastErrorText(); asserterror v := d.Get('missing'); exit(a + '|' + GetLastErrorText());",
    );
    let want = "The given key was not present in the dictionary.";
    assert_eq!(r.unwrap(), format!("{want}|{want}"));
}

#[test]
fn other_list_index_errors_are_still_refused() {
    // Only Get is pinned: RemoveAt, Set, Insert and GetRange outside the list stay refused.
    for body in ["l.RemoveAt(3);", "l.Set(3, 1);", "l.Insert(4, 1);", "l.Get(0);"] {
        let r = text_of("l: List of [Integer];", &format!("{body} exit('');"));
        assert!(matches!(r, Err(Signal::Unsupported(_))), "{body}: {r:?}");
    }
}

// ---------------------------------------------------------------- platform context

#[test]
fn a_test_runs_with_a_gui_on_a_service_tier() {
    // session/TestBCPlatformContracts.al:26-43 (GuiAllowed and IsServiceTier are true in a test);
    // handlers/TestConfirmNoHandler.al:28 (GuiAllowed is Yes inside a page trigger).
    let r = text_of(
        "",
        "exit(Format(GuiAllowed()) + Format(System.GuiAllowed()) + Format(IsServiceTier()));",
    );
    assert_eq!(r.unwrap(), "YesYesYes");
}

#[test]
fn the_last_used_row_version_is_positive_and_does_not_fall_after_an_insert() {
    // session/TestSystemExtended.al:635-659, session/TestBCDatabaseContracts.al:100-117.
    let src = "table 50902 \"RV Row\" { fields { field(1; Id; Integer) { } } keys { key(PK; Id) { } } }
        codeunit 50100 C { procedure F(): Text var r: Record \"RV Row\"; a: BigInteger; b: BigInteger;
        begin a := Database.LastUsedRowVersion(); r.Id := 1; r.Insert(); b := Database.LastUsedRowVersion();
              exit(Format(a > 0) + Format(b >= a) + Format(b = r.\"SystemRowVersion\")); end; }";
    match run(src, "F") {
        Ok(Value::Text(t)) => assert_eq!(t, "YesYesYes"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn the_lock_timeout_can_be_read_and_set() {
    // session/TestSystemExtended.al:608-630: callable in every form. The default is the documented one.
    let r = text_of(
        "a: Text;",
        "a := Format(Database.LockTimeout()); Database.LockTimeout(false); a += Format(Database.LockTimeout());
         Database.LockTimeout(true); exit(a + Format(Database.LockTimeout()));",
    );
    assert_eq!(r.unwrap(), "YesNoYes");
}

// ---------------------------------------------------------------- enum InitValue

#[test]
fn an_enum_field_starts_at_its_init_value_blank_or_named() {
    // types/TestBlankEnumInitTests.Codeunit.al:52-86 (InTestInsertInitializesBlankEnumToOrdinalZero,
    // MixedEnumInitValue...): a blank member as InitValue is ordinal 0, a named one its true ordinal.
    let src = "enum 50910 \"IV Level\" { value(0; \" \") { } value(5; Verbose) { } value(9; Quiet) { } }
        table 50911 \"IV Row\" { fields {
            field(1; K; Code[10]) { }
            field(2; Blank; Enum \"IV Level\") { InitValue = \" \"; }
            field(3; Named; Enum \"IV Level\") { InitValue = Verbose; }
            field(4; Opt; Option) { OptionMembers = \" \",Alpha; InitValue = \" \"; }
        } keys { key(PK; K) { } } }
        codeunit 50100 C { procedure F(): Text var r: Record \"IV Row\"; a: Text; begin
            r.Init(); r.K := 'M1'; r.Insert(false); r.Get('M1');
            exit(Format(r.Blank.AsInteger()) + '|' + Format(r.Named.AsInteger()) + '|' + Format(r.Opt = r.Opt::\" \")); end; }";
    match run(src, "F") {
        Ok(Value::Text(t)) => assert_eq!(t, "0|5|Yes"),
        other => panic!("{other:?}"),
    }
}
