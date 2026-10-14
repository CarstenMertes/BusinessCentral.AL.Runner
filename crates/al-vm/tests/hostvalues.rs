//! Host values end to end: the clock, the identity and the system field stamps (ADR-006, ADR-013).
//! Each test names the corpus test that pins the behaviour it asserts.

use al_builtins::native::Signal;
use al_builtins::value::Value;
use al_sema::workspace::AppSource;
use al_vm::{Engine, Session};

const TABLE: &str = r#"
table 50900 "ALT T"
{
    fields
    {
        field(1; "No."; Integer) { }
        field(2; Amount; Integer) { }
        field(3; Note; Text[30]) { }
    }
    keys { key(PK; "No.") { } }
}
table 50901 "ALT Temp" { TableType = Temporary; fields { field(1; K; Integer) { } } keys { key(PK; K) { } } }
"#;

fn compile(body: &str) -> (std::sync::Arc<Engine>, usize) {
    let src = format!("{TABLE}\ncodeunit 50100 C {{ {body} }}");
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
    let blocked: Vec<Vec<Option<String>>> = l
        .unsupported
        .iter()
        .map(|o| o.iter().map(|r| r.first().cloned()).collect())
        .collect();
    let oi = l.program.objects.iter().position(|o| o.name == "C").unwrap();
    for (ri, r) in l.program.objects[oi].routines.iter().enumerate() {
        if let Some(Some(why)) = blocked.get(oi).and_then(|o| o.get(ri)) {
            panic!("{} is blocked: {why}", r.name);
        }
    }
    (Engine::new(l.program, blocked), oi)
}

fn call(s: &mut Session, oi: usize, routine: &str) -> Result<Value, Signal> {
    let ri = s.engine.program.objects[oi]
        .routines
        .iter()
        .position(|r| r.name == routine)
        .unwrap();
    let inst = s.instance(oi);
    s.call(oi, ri, inst, vec![]).map(|(v, _)| v)
}

fn run(body: &str) -> Result<Value, Signal> {
    let (engine, oi) = compile(body);
    call(&mut Session::new(engine), oi, "F")
}

/// `procedure F(): Text var <vars> begin <body> end;`
fn text(vars: &str, body: &str) -> String {
    match run(&format!("procedure F(): Text var {vars} begin {body} end;")) {
        Ok(Value::Text(t)) => t,
        other => panic!("{other:?}"),
    }
}

fn truth(vars: &str, body: &str) -> bool {
    text(vars, &format!("exit(Format({body}));")) == "Yes"
}

// ---------------------------------------------------------------- the clock

#[test]
fn today_time_and_current_datetime_come_from_one_instant() {
    // session/TestSessionEvaluateRegionalPatterns.al:52-67, session/TestDateTimeContracts.al:19-21:
    // the three views of the clock agree.
    assert!(truth("D: DateTime;", "DT2Date(CurrentDateTime()) = Today()"));
    assert!(truth("", "CurrentDateTime() <> 0DT"));
    assert!(truth("", "Today() <> 0D"));
    assert!(truth("", "Time() <> 0T"));
}

#[test]
fn the_default_host_clock_is_frozen_at_a_known_instant() {
    // ADR-006: tests are deterministic by default. The instant is al-host's default.
    assert_eq!(text("", "exit(Format(Today(), 0, 9));"), "2024-01-15");
    assert_eq!(
        text("", "exit(Format(CurrentDateTime(), 0, 9));"),
        "2024-01-15T12:00:00.000Z"
    );
}

#[test]
fn sleep_moves_the_clock_by_the_time_waited() {
    // record/TestRecordSystemId.al:588 and others call Sleep(200) between two writes and expect
    // the second stamp to be later.
    assert_eq!(
        text(
            "A: DateTime; B: DateTime;",
            "A := CurrentDateTime(); Sleep(200); B := CurrentDateTime(); exit(Format(B - A = 200));"
        ),
        "Yes"
    );
}

#[test]
fn work_date_defaults_to_today_and_can_be_set() {
    // session/TestSessionFunctions.al:53 (non-zero), :62-65 (set and read back),
    // handlers/TestPageTemporalSetValue_Tests.al:285 (WorkDate is the working date shorthand).
    assert!(truth("", "WorkDate() = Today()"));
    assert!(truth("", "WorkDate() <> 0D"));
    assert_eq!(
        text(
            "Old: Date;",
            "Old := WorkDate(); WorkDate(20240101D); exit(Format(WorkDate(), 0, 9));"
        ),
        "2024-01-01"
    );
}

#[test]
fn work_date_of_the_undefined_date_is_refused() {
    // Not pinned: whether WorkDate(0D) resets to today. docs/probes-needed.md.
    let r = run("procedure F() begin WorkDate(0D); end;");
    assert!(
        matches!(r, Err(Signal::Unsupported(ref m)) if m.contains("WorkDate")),
        "{r:?}"
    );
}

// ---------------------------------------------------------------- the identity

#[test]
fn user_id_is_the_fixed_test_user() {
    // session/TestSessionFunctions.al:18,75-77: not empty and stable (ADR-013 decision 9).
    assert_eq!(text("", "exit(UserId());"), "TESTUSER");
}

#[test]
fn user_security_id_is_the_fixed_guid() {
    // session/TestUserSecurityId.al:34-58: not empty and stable (ADR-013 decision 9).
    assert_eq!(
        text("", "exit(Format(UserSecurityId(), 0, 9));"),
        "{C0A1BDFA-0000-0000-0000-545553545553}"
    );
    assert!(truth("", "not IsNullGuid(Database.UserSecurityId())"));
    assert!(truth("", "UserSecurityId() = Database.UserSecurityId()"));
}

#[test]
fn session_and_service_instance_ids_are_positive() {
    // session/TestSessionIdentityValues.al:24-59.
    assert!(truth("", "ServiceInstanceId() > 0"));
    assert!(truth("", "Database.ServiceInstanceId() = ServiceInstanceId()"));
    assert!(truth("", "SessionId() > 0"));
    assert!(truth("", "Database.SessionId() = SessionId()"));
}

#[test]
fn tenant_id_is_not_empty() {
    // session/TestDatabaseExtended.al:63-70.
    assert!(truth("", "Database.TenantId() <> ''"));
}

#[test]
fn platform_answers_of_a_test_session() {
    // session/TestBCPlatformContracts.al:27-42,205-211, session/TestFinalCoverage.al:62-73.
    assert!(truth("", "GuiAllowed()"));
    assert!(truth("", "IsServiceTier()"));
    assert!(truth("", "WindowsLanguage() > 0"));
    assert!(truth("", "GlobalLanguage() > 0"));
}

#[test]
fn global_language_can_be_set() {
    // session/TestSessionSettings.al:534 reads it; setting it is the other form of the builtin.
    assert_eq!(
        text("", "GlobalLanguage(1031); exit(Format(GlobalLanguage()));"),
        "1031"
    );
}

#[test]
fn client_types_are_callable() {
    // session/TestSessionExtended.al:21,31.
    assert!(truth(
        "C: ClientType;",
        "Session.CurrentClientType() = ClientType::Background"
    ));
    assert!(truth("C: ClientType;", "Session.DefaultClientType() = ClientType::Web"));
}

// ---------------------------------------------------------------- system field stamps

const STAMPS: &str = "
procedure Stamps(): Text
var R: Record \"ALT T\";
begin
    R.\"No.\" := 1;
    R.Insert();
    exit(Format(R.SystemCreatedAt, 0, 9) + '|' + Format(R.SystemModifiedAt, 0, 9) + '|' + Format(R.SystemCreatedBy, 0, 9) + '|' + Format(R.SystemModifiedBy, 0, 9));
end;
";

#[test]
fn insert_stamps_all_four_fields() {
    // session/TestTransactionContracts.al:131-170, record/TestBCSystemFieldContracts.al:78-102:
    // the stamps are the clock and UserSecurityId().
    let (engine, oi) = compile(STAMPS);
    let v = call(&mut Session::new(engine), oi, "Stamps").unwrap();
    let Value::Text(t) = v else { panic!() };
    assert_eq!(
        t,
        "2024-01-15T12:00:00.000Z|2024-01-15T12:00:00.000Z|{C0A1BDFA-0000-0000-0000-545553545553}|{C0A1BDFA-0000-0000-0000-545553545553}"
    );
}

#[test]
fn a_changing_modify_moves_modified_and_keeps_created() {
    // record/TestRecordSystemId.al:726-750, session/TestTransactionContracts.al:172-190.
    let t = text(
        "R: Record \"ALT T\"; C0: DateTime; M0: DateTime;",
        "R.\"No.\" := 1; R.Insert(); R.Get(1); C0 := R.SystemCreatedAt; M0 := R.SystemModifiedAt; \
         Sleep(200); R.Amount := 8; R.Modify(); R.Get(1); \
         exit(Format(R.SystemCreatedAt = C0) + Format(R.SystemModifiedAt > M0));",
    );
    assert_eq!(t, "YesYes");
}

#[test]
fn a_modify_that_changes_nothing_does_not_stamp() {
    // record/TestRecordSystemId.al:677-722.
    let t = text(
        "R: Record \"ALT T\"; M0: DateTime;",
        "R.\"No.\" := 1; R.Amount := 7; R.Insert(); R.Get(1); M0 := R.SystemModifiedAt; \
         Sleep(200); R.Modify(); R.Get(1); exit(Format(R.SystemModifiedAt = M0));",
    );
    assert_eq!(t, "Yes");
    let t = text(
        "R: Record \"ALT T\"; M0: DateTime;",
        "R.\"No.\" := 1; R.Amount := 7; R.Insert(); R.Get(1); M0 := R.SystemModifiedAt; \
         Sleep(200); R.Amount := 7; R.Modify(); R.Get(1); exit(Format(R.SystemModifiedAt = M0));",
    );
    assert_eq!(t, "Yes");
}

#[test]
fn rename_stamps_modified_even_to_the_same_key_and_keeps_created() {
    // record/TestRecordSystemId.al:580-648,752-775.
    let t = text(
        "R: Record \"ALT T\"; C0: DateTime; M0: DateTime; B0: Guid;",
        "R.\"No.\" := 504; R.Insert(); R.Get(504); C0 := R.SystemCreatedAt; M0 := R.SystemModifiedAt; B0 := R.SystemCreatedBy; \
         Sleep(200); R.Rename(504); Clear(R); R.Get(504); \
         exit(Format(R.SystemModifiedAt > M0) + Format(R.SystemCreatedAt = C0) + Format(R.SystemCreatedBy = B0) + Format(R.SystemModifiedBy = UserSecurityId()));",
    );
    assert_eq!(t, "YesYesYesYes");
}

#[test]
fn two_writes_in_one_millisecond_still_get_distinct_increasing_stamps() {
    // record/TestRecordSystemId.al:449 and TestXRecStampFields.al:179 need strictly later stamps.
    // With a frozen clock the second write is within the same instant.
    let t = text(
        "R: Record \"ALT T\"; A: DateTime; B: DateTime;",
        "R.\"No.\" := 1; R.Insert(); A := R.SystemModifiedAt; R.Amount := 1; R.Modify(); B := R.SystemModifiedAt; exit(Format(B > A));",
    );
    assert_eq!(t, "Yes");
}

#[test]
fn the_insert_trigger_and_before_event_see_default_stamps_the_buffer_has_them_after() {
    // record/TestXRecStampFields.al:75-99: Rec carries no stamp inside OnInsert; the stamps are
    // written below the trigger and the buffer has them when Insert returns.
    let t = text(
        "R: Record \"ALT T\";",
        "R.\"No.\" := 1; Clear(R.SystemCreatedAt); R.Insert(); exit(Format(R.SystemCreatedAt <> 0DT));",
    );
    assert_eq!(t, "Yes");
}

#[test]
fn temporary_records_are_not_stamped() {
    // Assumption (docs/probes-needed.md): the SQL layer writes the stamps, a temporary record
    // never reaches it. Source: AL Runner RowVersionPatches.cs (NCL decompilation).
    let t = text(
        "R: Record \"ALT Temp\" temporary;",
        "R.K := 1; R.Insert(); exit(Format(R.SystemCreatedAt = 0DT) + Format(IsNullGuid(R.SystemCreatedBy)));",
    );
    assert_eq!(t, "YesYes");
}

// ---------------------------------------------------------------- SessionSettings.Init

#[test]
fn session_settings_init_reads_language_locale_and_time_zone_from_the_host() {
    // session/TestSessionSettings.al:371-455: Init populates a positive language id and a
    // non-empty time zone, overwrites what was set before, and is repeatable. The values are the
    // host's (en-US, UTC), the zone the runtime models (docs/probes-needed.md "Dates").
    assert_eq!(
        text(
            "S: SessionSettings;",
            "S.TimeZone('Sentinel'); S.LanguageId(1030); S.Init(); \
             exit(Format(S.LanguageId()) + '|' + Format(S.LocaleId()) + '|' + S.TimeZone() + '|' + S.Company());"
        ),
        "1033|1033|UTC|"
    );
    assert_eq!(
        text(
            "A: SessionSettings; B: SessionSettings;",
            "A.Init(); B.TimeZone('x'); B.Init(); exit(Format(Format(A) = Format(B)));"
        ),
        "Yes"
    );
}

#[test]
fn company_name_is_the_hosts_company() {
    // session/TestSessionFunctions.al:25 (not empty), session/TestDatabase.al:74 (equals
    // CurrentCompany); the name is AL Runner's (ADR-013 decision 9).
    assert_eq!(text("", "exit(CompanyName());"), "My Company");
    assert!(truth("R: Record \"ALT T\";", "R.CurrentCompany() = CompanyName()"));
}

// ---------------------------------------------------------------- mutation-testing additions

#[test]
fn time_is_the_time_of_day_of_the_frozen_clock() {
    // The default instant is 12:00:00.000 UTC; Time() is the offset into the day, one-based
    // because 0T is the undefined time (Value::Time(0)).
    assert!(truth("", "Time() = 120000T"));
    assert!(truth("", "Time() > 115959.999T"));
    assert!(truth("", "Time() < 120000.001T"));
}

#[test]
fn sleep_of_zero_is_allowed_and_a_negative_wait_is_refused() {
    // Only a non-negative wait is pinned (hostvalues.rs); zero changes nothing.
    assert_eq!(
        text(
            "A: DateTime;",
            "A := CurrentDateTime(); Sleep(0); exit(Format(CurrentDateTime() = A));"
        ),
        "Yes"
    );
    match run("procedure F() begin Sleep(-1); end;") {
        Err(Signal::Unsupported(m)) => assert!(m.contains("non-negative"), "{m}"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn global_language_accepts_only_positive_ids() {
    for bad in ["0", "-1"] {
        match run(&format!("procedure F() begin GlobalLanguage({bad}); end;")) {
            Err(Signal::Unsupported(m)) => assert!(m.contains("positive language id"), "{bad}: {m}"),
            other => panic!("{bad}: {other:?}"),
        }
    }
}

// session/TestSessionComplete.al:45-52: an id no session has is not active. The running session's
// own id is not pinned: refused.
#[test]
fn is_session_active_is_false_for_an_unknown_id_and_refused_for_the_running_session() {
    let r = run("procedure F(): Boolean begin exit(Session.IsSessionActive(99999)); end;");
    assert!(matches!(r, Ok(Value::Boolean(false))), "{r:?}");
    let r = run("procedure F(): Boolean begin exit(Session.IsSessionActive(Database.SessionId())); end;");
    assert!(matches!(r, Err(Signal::Unsupported(_))), "{r:?}");
}
