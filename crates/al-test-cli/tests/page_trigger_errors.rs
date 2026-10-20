//! An error in OnQueryClosePage at TestPage.Close() shows as an unhandled message, and the page is closed after it (corpus
//! pages/lt-cc-page-leads/CcPageLeads.al:611-621 TriggerError_OnQueryClosePage). The message needs a test method, so this
//! runs through the test runner.

const SRC: &str = r#"
table 69995 "TQ Row" { fields { field(1; Id; Integer) { } field(2; Name; Text[30]) { } } keys { key(PK; Id) { } } }
page 69996 "TQ Card"
{
    PageType = Card; SourceTable = "TQ Row"; ApplicationArea = All; UsageCategory = None;
    layout { area(Content) { field(Name; Rec.Name) { } } }
    trigger OnQueryClosePage(CloseAction: Action): Boolean begin Error('TQ failure'); end;
}
codeunit 69997 T
{
    Subtype = Test;
    local procedure Outcome(): Text
    var R: Text;
    begin
        if GetLastErrorText() = 'OK' then R := 'ok' else R := GetLastErrorText() + ' [' + GetLastErrorCode() + ']';
        ClearLastError();
        exit(R);
    end;
    local procedure ErrClose(var P: TestPage "TQ Card") begin P.Close(); Error('OK'); end;
    local procedure ErrRead(var P: TestPage "TQ Card") var V: Text; begin V := P.Name.Value(); Error('OK'); end;
    [Test]
    procedure QueryCloseError()
    var R: Record "TQ Row"; P: TestPage "TQ Card"; Got: Text;
    begin
        R.Id := 1; R.Name := 'a'; R.Insert();
        P.OpenEdit();
        asserterror ErrClose(P);
        Got := Outcome();
        asserterror ErrRead(P);
        Got += '|' + Outcome();
        asserterror ErrClose(P);
        Got += '|' + Outcome();
        if Got <> 'Unhandled UI: Message TQ failure [MissingUIHandler]|The TestPage is not open. [TestPageNotOpened]|The TestPage is not open. [TestPageNotOpened]' then
            Error(Got);
    end;
}
"#;

#[test]
fn an_error_in_on_query_close_page_leaves_the_page_closed() {
    let outcomes = al_testkit::outcomes_in(SRC, "T");
    assert!(
        matches!(outcomes.as_slice(), [(_, al_testrun::Outcome::Pass)]),
        "{outcomes:?}"
    );
}
