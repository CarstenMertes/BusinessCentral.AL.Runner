//! The per-test record of what code raised and touched, for the impact index (ADR-043 section 2.2): events raised
//! (also when no subscriber listens), tables a record operation used, tables written, pages and reports used.

use al_testkit::Built;

const SRC: &str = r#"
table 50960 "IT Row"
{
    fields { field(1; Id; Integer) { } field(2; V; Integer) { } }
    keys { key(PK; Id) { } }
}
table 50961 "IT Read"
{
    fields { field(1; Id; Integer) { } }
    keys { key(PK; Id) { } }
}
table 50962 "IT Untouched"
{
    fields { field(1; Id; Integer) { } }
    keys { key(PK; Id) { } }
}
codeunit 50963 "IT Pub"
{
    procedure Raise()
    begin
        OnNobodyListens(1);
    end;

    [IntegrationEvent(false, false)]
    local procedure OnNobodyListens(X: Integer)
    begin
    end;
}
codeunit 50964 C
{
    procedure Work()
    var
        Row: Record "IT Row";
        Other: Record "IT Read";
        Ref: RecordRef;
        Pub: Codeunit "IT Pub";
    begin
        Pub.Raise();
        Row.Id := 1;
        Row.Insert(true);
        Row.Validate(V, 3);
        if Other.Get(7) then;
        Ref.Open(Database::"IT Row");
        Ref.FindFirst();
        Ref.Delete();
    end;

    procedure Nothing()
    begin
    end;
}
"#;

#[test]
fn the_trace_holds_the_raises_and_touches_of_the_code_that_ran() {
    let mut b = Built::from_source(SRC);
    let obj = |name: &str| b.lowered.program.objects.iter().position(|o| o.name == name).unwrap();
    let (row, read, untouched, publ) = (obj("IT Row"), obj("IT Read"), obj("IT Untouched"), obj("IT Pub"));
    // Off: nothing is kept.
    b.run_in("C", "Work").unwrap();
    assert!(b.session.take_impact_trace().is_empty());

    b.session.enable_impact_trace();
    b.run_in("C", "Work").unwrap();
    let t = b.session.take_impact_trace();
    // A publisher with no subscriber is still raised, and so are the table events nobody subscribes to.
    assert!(t.events.contains(&(publ, "onnobodylistens".to_string())), "{t:?}");
    for e in [
        "onbeforeinsertevent",
        "onafterinsertevent",
        "onbeforevalidateevent",
        "onbeforevalidateevent/v",
        "onaftervalidateevent/v",
        "onbeforedeleteevent",
    ] {
        assert!(t.events.contains(&(row, e.to_string())), "{e}: {t:?}");
    }
    // Tables used, written (also through a RecordRef), and read only.
    assert!(
        t.tables.contains(&(row as u32)) && t.tables.contains(&(read as u32)),
        "{t:?}"
    );
    assert!(!t.tables.contains(&(untouched as u32)), "{t:?}");
    assert!(
        t.writes.contains(&(row as u32)) && !t.writes.contains(&(read as u32)),
        "{t:?}"
    );

    // Taking it starts a new record.
    b.run_in("C", "Nothing").unwrap();
    assert!(b.session.take_impact_trace().is_empty());
}
