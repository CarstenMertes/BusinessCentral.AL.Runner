//! A table extension reads and writes the `protected var` variables of its base table, on the globals of the same
//! record variable. Microsoft's tableextension 906 "Asm. Requisition Line" uses Requisition Line's `Item`,
//! `TempPlanningErrorLog` and `PlanningResiliency`; about 600 tests of the Planning bucket run it. Not pinned by a
//! corpus test (assumption `table-extension-base-vars`, docs/probes-needed.md).

use al_testkit::text;

const SRC: &str = r#"
table 69900 "TB Row"
{
    fields { field(1; Id; Integer) { } }
    keys { key(PK; Id) { } }
    protected var
        Counter: Integer;
        Tag: Text;
    procedure BaseRead(): Text
    begin
        exit(Tag + ':' + Format(Counter));
    end;
    trigger OnInsert()
    begin
        Counter += 10;
    end;
}
tableextension 69901 "TB Ext" extends "TB Row"
{
    procedure Bump(): Integer
    begin
        Counter += 1;
        Tag := 'ext';
        exit(Counter);
    end;
}
codeunit 69902 C
{
    procedure F(): Text
    var R: Record "TB Row"; Q: Record "TB Row";
    begin
        R.Id := 1;
        R.Insert(true);
        exit(Format(R.Bump()) + ';' + R.BaseRead() + ';' + Format(Q.Bump()));
    end;
}
"#;

#[test]
fn a_table_extension_shares_the_protected_variables_of_its_base_table_per_record() {
    assert_eq!(text(al_testkit::run_in(SRC, "C", "F")), "11;ext:11;1");
}
