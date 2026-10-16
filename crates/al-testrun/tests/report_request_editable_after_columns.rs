//! A request page control whose `Editable` names a variable still gets its routine when the report has dataset
//! columns before the request page. The routines of the columns are inserted into the same object, ahead of the
//! request page, and must not shift the names of the control routines (Microsoft's "Fixed Asset - Projected Value"
//! has `Editable = NumberOfDaysCtrlEditable;` after its dataset; Tests-Fixed Asset FixedAssetWithProjectedDisposal
//! in codeunit 134981 was refused with "property editable of control NumberOfDays has no routine").

use al_sema::workspace::AppSource;
use al_testrun::runtime::{CorpusRuntime, RunOptions, SuiteRun};
use al_testrun::{CaseId, Outcome};

const SRC: &str = r#"
table 50100 "RE Row" { fields { field(1; Id; Integer) { } field(2; Amount; Decimal) { } } keys { key(PK; Id) { Clustered = true; } } }
report 50101 "RE Rpt"
{
    ProcessingOnly = true;
    dataset { dataitem(Rows; "RE Row") { column(IdCol; Id) { } column(AmountCol; Amount) { } column(Calc; Amount * 2) { } } }
    requestpage { layout { area(Content) { group(Options) { field(Days; Period) { Editable = DaysEditable; } } } } trigger OnInit() begin DaysEditable := true; end; }
    var Period: Integer; DaysEditable: Boolean;
}
codeunit 50102 "T"
{
    Subtype = Test;
    [RequestPageHandler]
    procedure H(var RP: TestRequestPage "RE Rpt") begin RP.Days.SetValue(7); RP.Cancel().Invoke(); end;
    [Test]
    [HandlerFunctions('H')]
    procedure EditableAfterColumns()
    var R: Report "RE Rpt";
    begin
        R.RunModal();
    end;
}
"#;

#[test]
fn an_editable_variable_after_dataset_columns_has_its_routine() {
    let rt = CorpusRuntime::run_suites_with(
        vec![AppSource {
            id: "main".into(),
            name: "main".into(),
            dependencies: vec![],
            files: vec![("main.al".into(), SRC.into())],
        }],
        vec![al_sema::AppKind::Workspace],
        &[],
        &Default::default(),
        None,
        Default::default(),
        &[SuiteRun::everything()],
        &RunOptions::default(),
    );
    let o = rt.first_outcome(&CaseId {
        codeunit: 50102,
        name: "EditableAfterColumns".into(),
    });
    assert_eq!(o, Outcome::Pass);
}
