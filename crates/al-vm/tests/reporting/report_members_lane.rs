//! Report members of a processing-only report (report/lt-report-requestpage-query-members/P105*.al,
//! report/lt-report-members-testrequestpage/P91*.al): ShowOutput, TotalsCausedBy, CreateTotals, the layout streams of a
//! report without a layout file, and a second RunModal on one variable.

use al_testkit::text;

const SRC: &str = r#"
table 69990 "RML Row" { fields { field(1; Id; Integer) { } field(2; Amount; Decimal) { } } keys { key(PK; Id) { } } }
codeunit 69991 "RML Log"
{
    SingleInstance = true;
    var T: Text;
    procedure Add(S: Text) begin T += S + ';'; end;
    procedure Take(): Text var R: Text; begin R := T; T := ''; exit(R); end;
}
report 69992 "RML Rpt"
{
    ProcessingOnly = true; UseRequestPage = false;
    dataset
    {
        dataitem(R; "RML Row")
        {
            trigger OnPreDataItem() begin CurrReport.CreateTotals(Sum); CurrReport.ShowOutput(true); L.Add('pre ' + Format(CurrReport.ShowOutput())); end;
            trigger OnAfterGetRecord() begin Sum += R.Amount; L.Add(Format(R.Id) + ' tc=' + Format(CurrReport.TotalsCausedBy()) + ' sum=' + Format(Sum)); end;
        }
    }
    var L: Codeunit "RML Log"; Sum: Decimal;
}
query 69994 "RML Qry" { elements { dataitem(R; "RML Row") { column(Id; Id) { } column(Amount; Amount) { } } } }
table 69995 "RML Store" { fields { field(1; Id; Integer) { } field(2; Data; Blob) { } } keys { key(PK; Id) { } } }
codeunit 69993 C
{
    procedure Csv(Fmt: Integer; Sep: Text): Text
    var Store: Record "RML Store"; Q: Query "RML Qry"; OutS: OutStream; InS: InStream; Line: Text; R: Text; Ok: Boolean;
    begin
        Seed();
        Store.Id := 1; Store.Insert(); Store.Data.CreateOutStream(OutS);
        Ok := Q.SaveAsCsv(OutS, Fmt, Sep);
        Store.Modify(); Store.CalcFields(Data);
        R := Format(Ok) + ':';
        if Store.Data.HasValue() then begin
            Store.Data.CreateInStream(InS);
            while not InS.EOS do begin InS.ReadText(Line); R += Line + '|'; end;
        end;
        exit(R);
    end;

    procedure CsvZero(): Text begin exit(Csv(0, '')); end;
    procedure CsvSemicolon(): Text begin exit(Csv(1, ';')); end;
    procedure CsvTwoEmpty(): Text begin exit(Csv(2, '')); end;

    procedure Seed()
    var Row: Record "RML Row";
    begin
        Row.DeleteAll(); Row.Id := 1; Row.Amount := 2; Row.Insert(); Row.Id := 2; Row.Amount := 3; Row.Insert();
    end;

    procedure Members(): Text
    var Rpt: Report "RML Rpt"; L: Codeunit "RML Log";
    begin
        Seed(); L.Take();
        Rpt.RunModal();
        exit(L.Take());
    end;

    procedure RunModalTwice(): Text
    var Rpt: Report "RML Rpt"; L: Codeunit "RML Log";
    begin
        Seed();
        Rpt.RunModal(); L.Take();
        asserterror Rpt.RunModal();
        exit(GetLastErrorCode() + '|' + L.Take());
    end;

    procedure RunModalClearRunModal(): Text
    var Rpt: Report "RML Rpt"; L: Codeunit "RML Log";
    begin
        Seed();
        Rpt.RunModal(); L.Take();
        Clear(Rpt);
        Rpt.RunModal();
        exit(L.Take());
    end;

    procedure LayoutNotFound(): Text
    var InS: InStream; Ok: Boolean;
    begin
        Ok := Report.RdlcLayout(Report::"RML Rpt", InS);
        asserterror if InS.EOS() then;
        exit(Format(Ok) + '|' + GetLastErrorCode() + '|' + GetLastErrorText());
    end;

    procedure LayoutStatement(): Text
    var InS: InStream;
    begin
        asserterror Report.WordLayout(Report::"RML Rpt", InS);
        exit(GetLastErrorCode());
    end;
}
"#;

#[test]
fn a_processing_only_report_shows_no_output_and_makes_no_totals() {
    assert_eq!(
        text(al_testkit::run_in(SRC, "C", "Members")),
        "pre No;1 tc=0 sum=2;2 tc=0 sum=5;"
    );
}

#[test]
fn a_second_runmodal_on_one_variable_raises_and_runs_nothing() {
    assert_eq!(
        text(al_testkit::run_in(SRC, "C", "RunModalTwice")),
        "ObjectHasAlreadyBeenRun|"
    );
}

#[test]
fn clear_lets_the_variable_run_modal_again() {
    assert_eq!(
        text(al_testkit::run_in(SRC, "C", "RunModalClearRunModal")),
        "pre No;1 tc=0 sum=2;2 tc=0 sum=5;"
    );
}

#[test]
fn a_layout_call_that_finds_no_layout_leaves_the_stream_uninitialized() {
    assert_eq!(
        text(al_testkit::run_in(SRC, "C", "LayoutNotFound")),
        "No|NotInitialized|Microsoft.Dynamics.Nav.Runtime.NavInStream variable not initialized."
    );
    assert_eq!(text(al_testkit::run_in(SRC, "C", "LayoutStatement")), "ReportNoLayout");
}

#[test]
fn savecsv_format_zero_writes_nothing_and_format_one_takes_the_separator() {
    assert_eq!(text(al_testkit::run_in(SRC, "C", "CsvZero")), "No:");
    assert_eq!(
        text(al_testkit::run_in(SRC, "C", "CsvSemicolon")),
        "Yes:Id;Amount|1;2|2;3|"
    );
    assert_eq!(
        text(al_testkit::run_in(SRC, "C", "CsvTwoEmpty")),
        "Yes:Id,Amount|1,2|2,3|"
    );
}
