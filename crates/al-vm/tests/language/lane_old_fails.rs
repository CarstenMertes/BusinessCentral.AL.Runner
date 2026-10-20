//! Corpus cases that failed before corpus bump 34 and the runtime got wrong (branch lane-old-fails).
//! Each test cites its case.

use al_builtins::native::Signal;
use al_builtins::value::Value;

fn out(r: Result<Value, Signal>) -> String {
    match r {
        Ok(Value::Text(t) | Value::Code(t)) => t.to_string(),
        Ok(Value::Integer(i)) => i.to_string(),
        Ok(Value::Boolean(b)) => b.to_string(),
        Err(Signal::Error(e)) => format!("error:{}|{}", e.message, e.code),
        Err(Signal::Unsupported(why)) => format!("refused:{why}"),
        other => format!("{other:?}"),
    }
}

fn text(vars: &str, body: &str) -> String {
    let src = format!("codeunit 50100 C {{ procedure F(): Text var {vars} begin {body} end; }}");
    out(al_testkit::run_in(&src, "C", "F"))
}

/// `Clear` of an array of JsonObject leaves every element as it was (json/lt-db-json-leads/DbJsonLeads.al:150-164).
#[test]
fn clear_of_an_array_of_json_object_leaves_the_elements() {
    let r = text(
        "A: array[3] of JsonObject; T: Text; U: Text;",
        "A[1].ReadFrom('{\"a\":1}'); A[3].ReadFrom('{\"c\":3}'); Clear(A); A[1].WriteTo(T); A[3].WriteTo(U); exit(T + U);",
    );
    assert_eq!(r, "{\"a\":1}{\"c\":3}");
}

/// `Clear` of an array of another JSON type is not pinned: refused.
#[test]
fn clear_of_an_array_of_json_array_is_refused() {
    let r = text("A: array[2] of JsonArray;", "A[1].Add(1); Clear(A); exit('ran');");
    assert!(r.starts_with("refused:"), "{r}");
}

/// `ReadAs(var Text)` after the body was handed out as a stream: a copy of a response's content reads an empty text (network/
/// lt-db-http-leads/DbHttpContentLeads.al:146), the response's own content the body (:113).
#[test]
fn http_content_read_as_text_after_a_stream_was_taken() {
    let r = text(
        "R: HttpResponseMessage; C: HttpContent; S: InStream; T: Text; Line: Text;",
        "R.Content().WriteFrom('body'); C := R.Content(); C.ReadAs(S); S.ReadText(Line); C.ReadAs(T); exit(Line + '|' + T);",
    );
    assert_eq!(r, "body|");
    let r = text(
        "R: HttpResponseMessage; S: InStream; T: Text; Line: Text;",
        "R.Content().WriteFrom('body'); R.Content().ReadAs(S); S.ReadText(Line); R.Content().ReadAs(T); exit(Line + '|' + T);",
    );
    assert_eq!(r, "body|body");
    // A variable written from a text reads the body again (xml/lt-xml-attr-doc-http-members/P97ContentRead.al:89).
    let r = text(
        "C: HttpContent; S: InStream; T: Text; Line: Text;",
        "C.WriteFrom('body'); C.ReadAs(S); S.ReadText(Line); C.ReadAs(T); exit(Line + '|' + T);",
    );
    assert_eq!(r, "body|body");
}

/// `WriteFrom(InStream)` leaves the source stream where it was: it is not at its end
/// (network/lt-db-http-leads/DbHttpContentLeads.al:310). Any other use of the source afterwards is not pinned.
#[test]
fn http_content_write_from_stream_leaves_the_source_not_at_its_end() {
    let src = r#"table 50101 B { fields { field(1; Id; Integer) { } field(2; Data; Blob) { } } }
codeunit 50100 C {
  procedure Fill(var R: Record B temporary; var S: InStream) var O: OutStream; begin R.Data.CreateOutStream(O); O.WriteText('picture'); R.Insert(); R.CalcFields(Data); R.Data.CreateInStream(S); end;
  procedure F(): Text var R: Record B temporary; C: HttpContent; S: InStream; T1: Text; T2: Text; begin Fill(R, S); C.WriteFrom(S); C.ReadAs(T1); C.ReadAs(T2); exit(T1 + '|' + T2 + '|' + Format(S.EOS())); end;
  procedure ReadSource(): Text var R: Record B temporary; C: HttpContent; S: InStream; T: Text; begin Fill(R, S); C.WriteFrom(S); S.ReadText(T); exit(T); end; }"#;
    assert_eq!(out(al_testkit::run_in(src, "C", "F")), "picture|picture|No");
    let r = out(al_testkit::run_in(src, "C", "ReadSource"));
    assert!(r.starts_with("refused:"), "{r}");
}

/// `Clear` on the content of a request that has none gives it a body and content headers: `GetHeaders` then hands out the
/// content's headers with `text/plain; charset=utf-8` (network/lt-db-http-leads/DbHttpContentLeads.al:176-219).
#[test]
fn http_request_content_clear_then_get_headers() {
    let src = r#"codeunit 50100 C {
  [TryFunction] local procedure TryHeaders(var Req: HttpRequestMessage; var H: HttpHeaders) begin Req.Content.GetHeaders(H); end;
  local procedure V(var H: HttpHeaders; N: Text): Text var Values: array[5] of Text; begin if not H.Contains(N) then exit('absent'); H.GetValues(N, Values); exit(Values[1]); end;
  procedure F(): Text var Req: HttpRequestMessage; H: HttpHeaders; R: Text; T: Text; begin
    if TryHeaders(Req, H) then R := 'ok1' else R := 'err1';
    Req.Content.Clear();
    Req.Content.GetHeaders(H);
    R += '|' + V(H, 'Content-Type');
    R += '|' + Format(H.Remove('Content-Type')) + Format(H.Add('Content-Type', 'application/json')) + V(H, 'Content-Type');
    R += '|' + Format(Req.Content.ReadAs(T)) + T;
    Req.Content.GetHeaders(H);
    R += '|' + V(H, 'Content-Type');
    Req.Content.WriteFrom('{"a":1}');
    Req.Content.GetHeaders(H);
    exit(R + '|' + V(H, 'Content-Type'));
  end; }"#;
    assert_eq!(
        out(al_testkit::run_in(src, "C", "F")),
        "err1|text/plain; charset=utf-8|YesYesapplication/json|Yes|application/json|text/plain; charset=utf-8"
    );
}

/// An InStream handed back from a routine whose local owns its bytes reads as empty on BC once the local is out of scope
/// (streams/lt-dc-stream-leads/DcInStreamScope.al:107-112, :125-126): refused. A stream of a global and one read in scope run.
#[test]
fn an_instream_of_a_local_handed_back_is_refused() {
    let src = r#"table 50101 B { fields { field(1; Id; Integer) { } field(2; Data; Blob) { } } }
codeunit 50100 C {
  var G: Record B temporary;
  local procedure FromLocal() Result: InStream var R: Record B temporary; O: OutStream; begin R.Data.CreateOutStream(O); O.WriteText('x'); R.Data.CreateInStream(Result); end;
  local procedure FromLocalVar(var S: InStream) var R: Record B temporary; O: OutStream; begin R.Data.CreateOutStream(O); O.WriteText('x'); R.Data.CreateInStream(S); end;
  local procedure FromGlobal() Result: InStream var O: OutStream; begin G.Data.CreateOutStream(O); O.WriteText('kept'); G.Data.CreateInStream(Result); end;
  procedure Local(): Text var S: InStream; T: Text; begin S := FromLocal(); S.ReadText(T); exit(T); end;
  procedure LocalVar(): Text var S: InStream; T: Text; begin FromLocalVar(S); S.ReadText(T); exit(T); end;
  procedure Global(): Text var S: InStream; T: Text; begin S := FromGlobal(); S.ReadText(T); exit(T); end; }"#;
    for routine in ["Local", "LocalVar"] {
        let r = out(al_testkit::run_in(src, "C", routine));
        assert!(r.contains("out of scope"), "{routine}: {r}");
    }
    assert_eq!(out(al_testkit::run_in(src, "C", "Global")), "kept");
}
