//! TransferFields from another table and the buffer's SystemCreatedAt and SystemModifiedAt (corpus bump 36, writer QE).

use al_builtins::native::Signal;

const SRC: &str = r#"
table 69991 "QE Row"
{
    fields { field(1; Id; Integer) { } field(2; Name; Text[30]) { } }
    keys { key(PK; Id) { } }
}
table 69992 "QE Other"
{
    fields { field(1; Id; Integer) { } field(2; Name; Text[30]) { } }
    keys { key(PK; Id) { } }
}
codeunit 69993 C
{
    procedure CreatedAtAfterTransferFromOther(): Text
    var R: Record "QE Row"; O: Record "QE Other";
    begin
        O.Id := 1; O.Name := 'other'; O.Insert();
        R.Id := 1; R.Name := 'own'; R.Insert();
        R.TransferFields(O);
        exit(Format(R.SystemCreatedAt = O.SystemCreatedAt));
    end;

    procedure ModifiedAtAfterTransferFromOther(): Text
    var R: Record "QE Row"; O: Record "QE Other";
    begin
        O.Id := 1; O.Name := 'other'; O.Insert();
        R.Id := 1; R.Name := 'own'; R.Insert();
        R.TransferFields(O);
        exit(Format(R.SystemModifiedAt = O.SystemModifiedAt));
    end;

    procedure NameAfterTransferFromOther(): Text
    var R: Record "QE Row"; O: Record "QE Other";
    begin
        O.Id := 1; O.Name := 'other'; O.Insert();
        R.Id := 1; R.Name := 'own'; R.Insert();
        R.TransferFields(O);
        exit(R.Name + '|' + Format(R.SystemId = O.SystemId));
    end;
}
"#;

// record/lt-cn-transferfields-modify/CnTransferFieldsModify.al QE_TF_Other_SystemFields_AfterModify
// (`buffer createdat = other`, `buffer modifiedat = other`): from another table the buffer takes the source's
// SystemCreatedAt and SystemModifiedAt. The runtime kept the buffer's own; a read of them is refused.
#[test]
fn system_created_at_after_a_transfer_from_another_table_is_refused() {
    let r = al_testkit::run_in(SRC, "C", "CreatedAtAfterTransferFromOther");
    assert!(matches!(r, Err(Signal::Unsupported(_))), "{r:?}");
}

#[test]
fn system_modified_at_after_a_transfer_from_another_table_is_refused() {
    let r = al_testkit::run_in(SRC, "C", "ModifiedAtAfterTransferFromOther");
    assert!(matches!(r, Err(Signal::Unsupported(_))), "{r:?}");
}

// The same test pins that the buffer keeps its own SystemId (`buffer systemid = own`) and that normal fields move.
#[test]
fn normal_fields_and_system_id_after_a_transfer_from_another_table() {
    let r = al_testkit::run_in(SRC, "C", "NameAfterTransferFromOther");
    assert_eq!(al_testkit::text(r), "other|No");
}
