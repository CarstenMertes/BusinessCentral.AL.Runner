//! A call of a table procedure with expression arguments: `Log.SetError(StrSubstNo(..), Database::Item, CopyStr(..))`
//! (Base Application's Requisition Line, 540 tests of the Planning bucket were refused with "call .. has no callee the
//! checker resolved").

use al_testkit::{int, run_in};

const SRC: &str = r#"
table 69800 "SC Log" { fields { field(1; "Entry No."; Integer) { } field(2; Descr; Text[250]) { } field(3; "Table ID"; Integer) { } field(4; Pos; Text[250]) { } } keys { key(PK; "Entry No.") { } }
    procedure SetError(TheError: Text[250]; TheTableID: Integer; TheTablePosition: Text[250])
    begin
        "Entry No." := "Entry No." + 1;
        Descr := TheError; "Table ID" := TheTableID; Pos := TheTablePosition;
        Insert();
    end;
}
table 69801 "SC Item" { fields { field(1; "No."; Code[20]) { } } keys { key(PK; "No.") { } } }
codeunit 69802 C
{
    var Log: Record "SC Log" temporary;
        MissingErr: Label '%1 %2 %3', Locked = true;
    procedure F(): Integer
    var Item: Record "SC Item";
    begin
        Item."No." := 'X1';
        Log.SetError(StrSubstNo(MissingErr, Item.TableCaption(), Item."No.", Item.FieldCaption("No.")), Database::"SC Item", CopyStr(Item.GetPosition(), 1, 250));
        exit(Log.Count());
    end;
}
"#;

#[test]
fn a_table_procedure_called_with_expression_arguments_resolves() {
    assert_eq!(int(run_in(SRC, "C", "F")), 1);
}
