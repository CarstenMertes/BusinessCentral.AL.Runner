//! `this` in the code of a report passed on to an event, and the procedures of a report extension called on what
//! the subscriber receives. Microsoft's "Whse.-Source - Create Document" report raises
//! `OnAfterPostReport(..., this)` and its subscribers call procedures the Manufacturing report extension declares
//! (`SetProdOrder`) on the report they receive; about 960 tests of the Tests-SCM buckets (BC 29) were refused at
//! `this` ("passed by reference ... is not lowered"). The corpus pins `this` as the running object inside report and
//! page extensions (types/PYThisExtensions.al:ReportExtension_ThisAndPublicProcedure_BeforeAndAfterRun,
//! PageExtension_ThisInTrigger) and a public procedure of a reportextension called on a variable of the base report
//! (the same file); it does not pin `this` of a report as an argument (docs/probes-needed.md).

use al_testkit::text;

const SRC: &str = r#"
table 69400 "RT Tab" { fields { field(1; Id; Integer) { } } keys { key(PK; Id) { } } }
codeunit 69401 "RT Log"
{
    SingleInstance = true;
    var Log: Text;
    procedure Add(Entry: Text) begin Log += Entry + ';'; end;
    procedure Take(): Text var R: Text; begin R := Log; Log := ''; exit(R); end;
}
report 69402 "RT Base"
{
    ProcessingOnly = true; UseRequestPage = false;
    dataset { dataitem(Base; "RT Tab") { } }
    trigger OnPostReport() begin OnAfterPost(this); end;
    procedure BaseProc(): Text begin exit('baseproc'); end;
    [IntegrationEvent(false, false)]
    local procedure OnAfterPost(var R: Report "RT Base") begin end;
}
reportextension 69403 "RT Ext" extends "RT Base"
{
    var Marker: Text;
    procedure SetMarker(M: Text) begin Marker := M; end;
    procedure GetMarker(): Text begin exit(Marker); end;
}
codeunit 69404 "RT Sub"
{
    [EventSubscriber(ObjectType::Report, Report::"RT Base", 'OnAfterPost', '', false, false)]
    local procedure OnAfterPost(var R: Report "RT Base")
    var L: Codeunit "RT Log";
    begin
        L.Add(R.BaseProc());
        R.SetMarker('x');
        L.Add(R.GetMarker());
    end;
}
codeunit 69405 C
{
    procedure F(): Text var L: Codeunit "RT Log"; R: Record "RT Tab"; begin
        R.Id := 1; R.Insert();
        Report.Run(Report::"RT Base", false);
        exit(L.Take());
    end;
}
"#;

#[test]
fn this_of_a_report_goes_to_an_event_and_the_extension_procedures_work_on_it() {
    let v = al_testkit::run_in(SRC, "C", "F");
    assert_eq!(text(v), "baseproc;x;");
}
