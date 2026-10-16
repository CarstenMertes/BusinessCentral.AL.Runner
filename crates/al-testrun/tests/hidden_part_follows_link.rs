//! A part whose `Visible` expression came out false is in the test page and answers First() (corpus
//! misc/lt-probes-needed-7/PnGPartTriggers.al:HiddenPartExpression, :174). Its rows follow the host's current row through
//! SubPageLink like any part's: Microsoft's "Fixed Asset Card" hides its "FA Depreciation Books Subform" with
//! `Visible = not Simple`, and ERM Fixed Assets Journal.AcquireFixedAssetNotification sets a field of that part for the
//! asset it filtered the card on. The link of a hidden part is not pinned (assumption hidden-part-follows-link).

use al_sema::workspace::AppSource;
use al_testrun::runtime::{CorpusRuntime, RunOptions, SuiteRun};
use al_testrun::{CaseId, Outcome};

const SRC: &str = r#"
table 50100 "HP Head" { fields { field(1; Code; Code[10]) { } } keys { key(PK; Code) { Clustered = true; } } }
table 50101 "HP Line" { fields { field(1; "Head Code"; Code[10]) { } field(2; Years; Integer) { } } keys { key(PK; "Head Code") { Clustered = true; } } }
page 50102 "HP Part" { PageType = ListPart; SourceTable = "HP Line"; layout { area(Content) { repeater(R) { field("Head Code"; Rec."Head Code") { } field(Years; Rec.Years) { } } } } }
page 50103 "HP Card"
{
    PageType = Card; SourceTable = "HP Head";
    layout { area(Content) { field(Code; Rec.Code) { } part(Lines; "HP Part") { Visible = not Simple; SubPageLink = "Head Code" = field(Code); } } }
    trigger OnOpenPage() begin Simple := true; end;
    var Simple: Boolean;
}
codeunit 50104 "T"
{
    Subtype = Test;
    [Test]
    procedure HiddenPartShowsTheRowOfTheFilteredHost()
    var H: Record "HP Head"; L: Record "HP Line"; P: TestPage "HP Card";
    begin
        H.Code := 'A'; H.Insert(); H.Code := 'B'; H.Insert();
        L."Head Code" := 'A'; L.Years := 1; L.Insert(); L."Head Code" := 'B'; L.Years := 2; L.Insert();
        P.OpenEdit();
        P.Filter.SetFilter(Code, 'B');
        if P.Lines.Years.AsInteger() <> 2 then Error('the hidden part shows %1', P.Lines.Years.AsInteger());
        P.Lines.Years.SetValue(7);
        P.Close();
        L.Get('B');
        if L.Years <> 7 then Error('the line has %1', L.Years);
    end;
}
"#;

#[test]
fn a_hidden_part_follows_the_link_of_the_host() {
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
        codeunit: 50104,
        name: "HiddenPartShowsTheRowOfTheFilteredHost".into(),
    });
    assert_eq!(o, Outcome::Pass);
}
