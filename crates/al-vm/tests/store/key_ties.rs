//! Sorting by a key whose rows tie, and the primary key fields that follow a key
//! (corpus record/lt-setascending-ties/SetAscendingTies.al).

use al_builtins::native::Signal;
use al_testkit::{error_message, text};

const SRC: &str = r#"
table 69990 "KT Row"
{
    fields { field(1; Id; Integer) { } field(2; Grp; Integer) { } }
    keys { key(PK; Id) { } key(ByGrp; Grp) { } key(ByGrpId; Grp, Id) { } }
}
codeunit 69991 C
{
    local procedure Seed()
    var Row: Record "KT Row"; I: Integer;
    begin
        for I := 1 to 6 do begin
            Row.Init(); Row.Id := I; Row.Grp := (I - 1) div 3 + 1; Row.Insert();
        end;
    end;

    local procedure Ids(var Row: Record "KT Row"): Text
    var s: Text;
    begin
        if Row.FindSet() then
            repeat s += Format(Row.Id) + ','; until Row.Next() = 0;
        exit(s);
    end;

    procedure ExplicitPrimaryKey(): Text
    var Row: Record "KT Row";
    begin
        Seed();
        Row.SetCurrentKey(Grp, Id);
        exit(Ids(Row) + '|' + Row.GetView(false));
    end;

    procedure ExplicitPrimaryKeySetId()
    var Row: Record "KT Row";
    begin
        Row.SetCurrentKey(Grp, Id);
        Row.SetAscending(Id, false);
    end;

    procedure GrpDescIdAsc(): Text
    var Row: Record "KT Row";
    begin
        Seed();
        Row.SetCurrentKey(Grp);
        Row.SetAscending(Grp, false);
        Row.SetAscending(Id, true);
        exit(Ids(Row) + '|' + Row.GetView(false));
    end;

    procedure IdAscOnPlainKey()
    var Row: Record "KT Row";
    begin
        Row.SetCurrentKey(Grp);
        Row.SetAscending(Id, true);
    end;

    procedure IdDescAfterGrpDesc()
    var Row: Record "KT Row";
    begin
        Row.SetCurrentKey(Grp);
        Row.SetAscending(Grp, false);
        Row.SetAscending(Id, false);
    end;
}
"#;

// SetAscendingTies.al:173 (`key_grp_id_plain`): the primary key field named after the key is not part of the sorting.
#[test]
fn explicit_primary_key_field_after_the_key_is_dropped() {
    assert_eq!(
        text(al_testkit::run_in(SRC, "C", "ExplicitPrimaryKey")),
        "1,2,3,4,5,6,|VERSION(1) SORTING(Field2)"
    );
    // SetAscendingTies.al:174 (`key_grp_id_id_desc`).
    assert_eq!(
        error_message(al_testkit::run_in(SRC, "C", "ExplicitPrimaryKeySetId")),
        "The specified sorting could not be set on field 'Id' with the key consisting of 'Grp'."
    );
}

// SetAscendingTies.al:155 (`grp_desc_id_asc`): asking the following primary key field for the direction it already has is
// accepted and changes nothing.
#[test]
fn primary_key_field_after_a_descending_key_field_accepts_ascending() {
    assert_eq!(
        text(al_testkit::run_in(SRC, "C", "GrpDescIdAsc")),
        "4,5,6,1,2,3,|VERSION(1) SORTING(Field2) ORDER(1)"
    );
}

// Not pinned: the same request on a key with no descending field, and a descending request after a descending key field.
#[test]
fn unpinned_requests_on_a_following_primary_key_field_are_refused() {
    for routine in ["IdAscOnPlainKey", "IdDescAfterGrpDesc"] {
        let r = al_testkit::run_in(SRC, "C", routine);
        assert!(matches!(r, Err(Signal::Unsupported(_))), "{routine}: {r:?}");
    }
}

const FLOW: &str = r#"
table 69994 "KT Owner"
{
    fields
    {
        field(1; Id; Integer) { }
        field(2; Cnt; Integer) { FieldClass = FlowField; CalcFormula = count("KT Line" where(OwnerId = field(Id))); }
        field(3; All; Integer) { FieldClass = FlowField; CalcFormula = count("KT Line"); }
    }
    keys { key(PK; Id) { } }
}
table 69995 "KT Line" { fields { field(1; Id; Integer) { } field(2; OwnerId; Integer) { } } keys { key(PK; Id) { } } }
codeunit 69996 C
{
    procedure ByRowFlowField()
    var Owner: Record "KT Owner";
    begin
        Owner.SetCurrentKey(Cnt);
    end;

    procedure ByConstantFlowField(): Text
    var Owner: Record "KT Owner";
    begin
        Owner.SetCurrentKey(All);
        exit(Owner.CurrentKey());
    end;
}
"#;

// records/W4SortKeys.al:459, :471: a FlowField whose value is the same for every row is accepted and the key stays the primary
// key. record/lt-set-current-key/SetCurrentKeyFlowFieldOrder.al:109-113 sorts by a FlowField that depends on the row and reports
// it as the key: the rule that tells the two apart is not pinned, so that one is refused.
#[test]
fn set_current_key_on_a_flow_field_that_depends_on_the_row_is_refused() {
    assert_eq!(text(al_testkit::run_in(FLOW, "C", "ByConstantFlowField")), "Id");
    let r = al_testkit::run_in(FLOW, "C", "ByRowFlowField");
    assert!(matches!(r, Err(Signal::Unsupported(_))), "{r:?}");
}
