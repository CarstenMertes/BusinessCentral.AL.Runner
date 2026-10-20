//! `HttpHeaders.Add` used as an expression and `GetValues` of a name that is not there, on request, content and response headers.
//! Every literal is what real BC answered in the corpus: network/lt-ay-http/AyHeaderAddResult.al (codeunit 80507) and
//! network/lt-http/HttpContent.al:OldHeadersHandle_AfterWriteFrom (codeunit 72019).

use al_testkit::text;

/// The helpers of AyHeaderAddResult.al:17-81: `ret=<Add result> <read back>` or `E[<error>|<code>] <read back>`, where the read back is
/// a GetValues statement inside a TryFunction (`get=E` when it raises).
const SRC: &str = r#"
codeunit 50190 C
{
    [TryFunction]
    local procedure TryAdd(var H: HttpHeaders; N: Text; V: Text; var Ret: Boolean)
    begin
        Ret := H.Add(N, V);
    end;

    [TryFunction]
    local procedure TryGetValues(var H: HttpHeaders; N: Text; var Vals: List of [Text])
    begin
        H.GetValues(N, Vals);
    end;

    local procedure ReadBack(var H: HttpHeaders; N: Text): Text
    var
        Vals: List of [Text];
        V: Text;
        Out: Text;
    begin
        if not TryGetValues(H, N, Vals) then
            exit('get=E');
        Out := 'n=' + Format(Vals.Count());
        foreach V in Vals do
            Out += '[' + V + ']';
        exit(Out);
    end;

    local procedure AddTo(var H: HttpHeaders; N: Text; V: Text): Text
    var
        Ret: Boolean;
    begin
        ClearLastError();
        if TryAdd(H, N, V, Ret) then
            exit('ret=' + Format(Ret) + ' ' + ReadBack(H, N));
        exit('E[' + CopyStr(GetLastErrorText(), 1, 60) + '|' + GetLastErrorCode() + '] ' + ReadBack(H, N));
    end;

    procedure Req(N: Text; V: Text): Text
    var
        R: HttpRequestMessage;
        H: HttpHeaders;
    begin
        R.GetHeaders(H);
        exit(AddTo(H, N, V));
    end;

    procedure Cnt(N: Text; V: Text): Text
    var
        C: HttpContent;
        H: HttpHeaders;
    begin
        C.GetHeaders(H);
        exit(AddTo(H, N, V));
    end;

    procedure Resp(N: Text; V: Text): Text
    var
        R: HttpResponseMessage;
        H: HttpHeaders;
    begin
        H := R.Headers();
        exit(AddTo(H, N, V));
    end;

    procedure ContentStatement(): Text
    var
        C: HttpContent;
        H: HttpHeaders;
        Vals: List of [Text];
    begin
        C.GetHeaders(H);
        C.WriteFrom('hello');
        asserterror H.GetValues('Content-Length', Vals);
        exit(GetLastErrorText() + '|' + GetLastErrorCode());
    end;

    procedure ContentUsed(): Text
    var
        C: HttpContent;
        H: HttpHeaders;
        Vals: List of [Text];
        R: Text;
    begin
        C.GetHeaders(H);
        C.WriteFrom('hello');
        ClearLastError();
        R := Format(H.GetValues('Content-Length', Vals)) + ' n=' + Format(Vals.Count());
        exit(R + '|' + GetLastErrorText() + '|' + GetLastErrorCode());
    end;
}
"#;

fn call(routine: &str, name: &str, value: &str) -> String {
    let mut b = al_testkit::Built::from_source(SRC);
    let (oi, ri) = b.routine(routine);
    let r = b
        .call(
            oi,
            ri,
            vec![
                al_builtins::value::Value::Text(name.into()),
                al_builtins::value::Value::Text(value.into()),
            ],
        )
        .map(|(v, _)| v);
    text(r)
}

const NAV_INVALID_HEADER: &str =
    "Exception of type 'Microsoft.Dynamics.Nav.Types.Exceptions.NavNclInvalidHeaderException' was thrown.";

#[test]
fn get_values_of_a_missing_content_header_raises_as_a_statement_and_sets_the_last_error_when_used() {
    // HttpContent.al:270 and :290-291 (statement), :226 and :281-283 (result used: false, nothing filled, the last error set).
    assert_eq!(
        text(al_testkit::run(SRC, "ContentStatement")),
        format!("{NAV_INVALID_HEADER}|InvalidHeader")
    );
    assert_eq!(
        text(al_testkit::run(SRC, "ContentUsed")),
        format!("No n=0|{NAV_INVALID_HEADER}|InvalidHeader")
    );
}

#[test]
fn get_values_statement_of_a_missing_name_raises_on_request_and_response_headers() {
    // AyHeaderAddResult.al:88 (accept_noslash), :200 (range_bad), :283 (enc_bad), :87 (accept_ok): the Add refused the value, GetValues raises.
    assert_eq!(call("Req", "Accept", "json"), "ret=No get=E");
    assert_eq!(call("Req", "Range", "abc"), "ret=No get=E");
    assert_eq!(call("Cnt", "Content-Encoding", "g z"), "ret=No get=E");
    assert_eq!(
        call("Req", "Accept", "application/json"),
        "ret=Yes n=1[application/json]"
    );
}

#[test]
fn add_as_an_expression_answers_false_for_every_refused_name() {
    // AyHeaderAddResult.al:159 (blank name, Argument), :186-187 (a content header on request headers and the reverse, InvalidOperation),
    // :383 (Content-Type on response headers), :157, :172 and :185 (names that are no token, Format).
    assert_eq!(call("Req", "", "v"), "ret=No get=E");
    assert_eq!(call("Req", "Bad Name", "v"), "ret=No get=E");
    assert_eq!(call("Req", "X-Ä", "v"), "ret=No get=E");
    assert_eq!(call("Req", "Content-Type", "text/plain"), "ret=No get=E");
    assert_eq!(call("Cnt", "Accept", "text/plain"), "ret=No get=E");
    assert_eq!(call("Resp", "Content-Type", "text/plain"), "ret=No get=E");
    assert_eq!(call("Resp", "Bad Name", "v"), "ret=No get=E");
}

#[test]
fn typed_request_header_values_follow_the_probe() {
    // AyHeaderAddResult.al:101 (Accept with an open quote), :241-243 (If-Range, Max-Forwards), :255-257 (Referer, Expect).
    assert_eq!(call("Req", "Accept", "a/b; q=\""), "ret=No get=E");
    assert_eq!(call("Req", "If-Range", "abc"), "ret=No get=E");
    assert_eq!(call("Req", "Max-Forwards", "5"), "ret=Yes n=1[5]");
    assert_eq!(call("Req", "Max-Forwards", "five"), "ret=No get=E");
    assert_eq!(
        call("Req", "Referer", "https://a.example/x"),
        "ret=Yes n=1[https://a.example/x]"
    );
    assert_eq!(call("Req", "Referer", "a b"), "ret=Yes n=1[a%20b]");
    assert_eq!(call("Req", "Expect", "="), "ret=No get=E");
}

#[test]
fn typed_content_header_values_follow_the_probe() {
    // AyHeaderAddResult.al:270, :284-285, :297-299, :311, :325-327.
    assert_eq!(call("Cnt", "Content-Disposition", ";;;"), "ret=No get=E");
    assert_eq!(call("Cnt", "Content-Language", "e n"), "ret=No get=E");
    assert_eq!(
        call("Cnt", "Content-Range", "bytes 0-9/10"),
        "ret=Yes n=1[bytes 0-9/10]"
    );
    assert_eq!(call("Cnt", "Content-Range", "abc"), "ret=No get=E");
    assert_eq!(
        call("Cnt", "Content-MD5", "AAECAwQFBgcICQoLDA0ODw=="),
        "ret=Yes n=1[AAECAwQFBgcICQoLDA0ODw==]"
    );
    assert_eq!(call("Cnt", "Content-MD5", "not base64!"), "ret=No get=E");
    assert_eq!(call("Cnt", "Content-Location", "a b"), "ret=Yes n=1[a%20b]");
    assert_eq!(call("Cnt", "Expires", "0"), "ret=No get=E");
    assert_eq!(call("Cnt", "Last-Modified", "soon"), "ret=No get=E");
    assert_eq!(call("Cnt", "Allow", "G E T"), "ret=No get=E");
}

#[test]
fn typed_response_header_values_follow_the_probe() {
    // AyHeaderAddResult.al:339-341, :353-355, :367-369, :381-382.
    assert_eq!(call("Resp", "Age", "12"), "ret=Yes n=1[12]");
    assert_eq!(call("Resp", "Age", "old"), "ret=No get=E");
    assert_eq!(call("Resp", "ETag", "\"abc\""), "ret=Yes n=1[\"abc\"]");
    assert_eq!(call("Resp", "ETag", "abc"), "ret=No get=E");
    assert_eq!(
        call("Resp", "Location", "https://a.example/x"),
        "ret=Yes n=1[https://a.example/x]"
    );
    assert_eq!(call("Resp", "Location", "a b"), "ret=Yes n=1[a%20b]");
    assert_eq!(call("Resp", "Retry-After", "120"), "ret=Yes n=1[120]");
    assert_eq!(call("Resp", "Retry-After", "later"), "ret=No get=E");
    assert_eq!(call("Resp", "Vary", "a b"), "ret=No get=E");
    // A lone comma is an empty list: the Add answers true and stores nothing (:381, :384).
    assert_eq!(call("Resp", "WWW-Authenticate", ","), "ret=Yes get=E");
    assert_eq!(call("Resp", "Accept-Ranges", "a b"), "ret=No get=E");
}
