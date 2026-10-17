//! NaviPartner NP Retail blockers: FieldRef.GetRangeMin/GetRangeMax and a SystemId set in OnInsert.

use al_builtins::native::Signal;
use al_testkit::text;

const T: &str = r#"
table 69980 "NB T" { fields { field(1; K; Code[10]) { } field(2; N; Integer) { } } keys { key(PK; K) { } } }
codeunit 69981 C
{
    procedure F(): Text
    var R: Record "NB T"; RR: RecordRef; FR: FieldRef; S: Text;
    begin
        RR.GetTable(R);
        FR := RR.Field(2);
        FR.SetRange(20, 40);
        S := Format(FR.GetRangeMin()) + '/' + Format(FR.GetRangeMax());
        FR.SetFilter('>20');
        S += ' ' + Format(FR.GetRangeMin());
        exit(S);
    end;
    procedure NoUpper(): Text
    var R: Record "NB T"; RR: RecordRef; FR: FieldRef;
    begin
        RR.GetTable(R);
        FR := RR.Field(2);
        FR.SetFilter('>20');
        exit(Format(FR.GetRangeMax()));
    end;
    procedure Many(): Text
    var R: Record "NB T"; RR: RecordRef; FR: FieldRef;
    begin
        RR.GetTable(R);
        FR := RR.Field(2);
        FR.SetFilter('10..20|40..50');
        exit(Format(FR.GetRangeMin()));
    end;
}
"#;

// Pins: corpus recordref/lt-refs/RfTFieldFilters.al:183, record/lt-filters-keys-finds/P68B_RangeMinMax.al.
#[test]
fn field_ref_range_bounds_follow_the_record_rules() {
    assert_eq!(text(al_testkit::run_in(T, "C", "F")), "20/40 20");
    for (routine, code, msg) in [
        (
            "NoUpper",
            "DB:Ok",
            "The filter \">20\" on the N field in the NB T table has no upper bound.",
        ),
        (
            "Many",
            "DB:NotIntervalFilter",
            "The filter expression \"10..20|40..50\" applied to the field N in the NB T table results in more than one range.",
        ),
    ] {
        match al_testkit::run_in(T, "C", routine) {
            Err(Signal::Error(e)) => {
                assert_eq!(e.code, code);
                assert_eq!(e.message, msg);
            }
            other => panic!("{routine}: {other:?}"),
        }
    }
}

const SID: &str = r#"
table 69982 "NB S" { fields { field(1; K; Integer) { } } keys { key(PK; K) { } }
    trigger OnInsert() begin SystemId := CreateGuid(); end; }
codeunit 69983 C { procedure F(): Text var R: Record "NB S"; begin R.K := 1; R.Insert(true); exit('ran'); end; }
"#;

// Whether BC keeps a SystemId that OnInsert sets is not pinned (probe wanted): refused, not guessed.
#[test]
fn a_system_id_set_in_on_insert_is_refused() {
    match al_testkit::run_in(SID, "C", "F") {
        Err(Signal::Unsupported(m)) => assert!(m.contains("SystemId"), "{m}"),
        other => panic!("{other:?}"),
    }
}

const ISO: &str = r#"
codeunit 69984 C
{
    procedure F(): Text
    var G: Guid; V: Text;
    begin
        Evaluate(G, '{6F2C3D4E-5B6C-4D7E-9F80-AB1C2D3E4F50}');
        IsolatedStorage.Set(G, 'x', DataScope::Company);
        IsolatedStorage.Get(Format(G), DataScope::Company, V);
        exit(V + Format(IsolatedStorage.Contains(G, DataScope::Company)));
    end;
}
"#;

// session/lt-isolated-storage-guid/IsoStorageGuidKey.al: a Guid key is its Format text.
#[test]
fn an_isolated_storage_guid_key_is_its_text() {
    assert_eq!(text(al_testkit::run_in(ISO, "C", "F")), "xYes");
}
