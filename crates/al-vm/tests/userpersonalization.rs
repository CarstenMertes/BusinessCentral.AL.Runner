//! "User Personalization" (2000000073): the platform creates the session user's row when the user
//! signs in to a client. The corpus pins no such row (access-control/TestUserTableSessionUser.al
//! only validates the relation to User; session/TestSessionSettings.al:402-407 says what the row
//! holds "depends on tenant data"). A read that finds no row for the session user is therefore
//! refused, not answered "not found" (docs/probes-needed.md, "Probe batch 6"). Rows a test writes
//! itself (profile/TestTenantProfileRenameCascade.al:39-80) are ordinary rows.

use al_builtins::native::Signal;
use al_builtins::value::Value;
use al_sema::workspace::AppSource;
use al_vm::{Engine, Session};

const TABLES: &str = r#"
table 2000000120 "User" {
    fields { field(1; "User Security ID"; Guid) { } field(2; "User Name"; Code[50]) { } field(3; "Full Name"; Text[80]) { } }
    keys { key(PK; "User Security ID") { } }
}
table 2000000073 "User Personalization" {
    fields { field(1; "User ID"; Code[50]) { } field(3; "User SID"; Guid) { } field(2; "Profile ID"; Code[30]) { } }
    keys { key(PK; "User SID") { } }
}
"#;

fn run(body: &str) -> Result<Value, Signal> {
    let src = format!(
        "{TABLES}\ncodeunit 50100 C {{ procedure F(): Text var P: Record \"User Personalization\"; Other: Guid; begin {body} end; }}"
    );
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
    let ri = l.program.objects[oi]
        .routines
        .iter()
        .position(|r| r.name == "F")
        .unwrap();
    let mut s = Session::new(Engine::new(l.program, blocked));
    let inst = s.instance(oi);
    s.call(oi, ri, inst, vec![]).map(|(v, _)| v)
}

#[test]
fn a_get_of_the_session_users_missing_row_is_refused() {
    let r = run("exit(Format(P.Get(UserSecurityId())));");
    assert!(
        matches!(&r, Err(Signal::Unsupported(m)) if m.contains("User Personalization") && m.contains("signs in")),
        "{r:?}"
    );
    // Without the Boolean result the same read would raise "does not exist": refused too.
    let r = run("P.Get(UserSecurityId()); exit('');");
    assert!(matches!(&r, Err(Signal::Unsupported(_))), "{r:?}");
}

#[test]
fn rows_a_test_wrote_and_other_users_are_ordinary() {
    // The row of the session user, once the test has written it, is found and changed.
    let r = run(
        "P.\"User SID\" := UserSecurityId(); P.\"Profile ID\" := 'X'; P.Insert(); \
         P.Get(UserSecurityId()); P.\"Profile ID\" := 'Y'; P.Modify(); P.Get(UserSecurityId()); exit(P.\"Profile ID\");",
    );
    assert!(matches!(&r, Ok(Value::Text(t)) if t == "Y"), "{r:?}");
    // Another user's missing row is the ordinary answer.
    let r = run("Other := CreateGuid(); exit(Format(P.Get(Other)));");
    assert!(matches!(&r, Ok(Value::Text(t)) if t == "No"), "{r:?}");
}
