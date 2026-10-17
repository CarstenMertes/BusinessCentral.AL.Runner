//! `HttpClient` served by `[HttpClientHandler]` mocks, and the HTTP message types around it. Never a network:
//! a request is answered by a handler of the test, or it raises the policy error, or it is refused.
//!
//! Pins: the staging probes tests/al-language-onprem/session/PZHttpHandler.al (BC 29.0.54011.55816, codeunit
//! 61297) and PZHttpBlock.al (61298), and tests/al-language/network/lt-http/*.al (the laptop's HTTP topic, BC
//! 29.0.54011.55816). Each test names its source.

use al_sema::workspace::AppSource;
use al_test_cli::report::Outcome;
use al_test_cli::runtime::run_codeunit;
use al_vm::Engine;

fn compile(src: String) -> (al_sema::workspace::Workspace, al_sema::lower::Lowered) {
    al_sema::lower::compile(
        vec![AppSource {
            id: "a".into(),
            name: "a".into(),
            dependencies: vec![],
            files: vec![("f.al".into(), src)],
        }],
        &Default::default(),
    )
}

/// Runs the tests of codeunit `T`, which gets `header` as its properties and `body` as its members.
fn run_with(header: &str, body: &str) -> Vec<(String, Outcome)> {
    let (ws, l) = compile(format!("codeunit 69400 T {{ Subtype = Test; {header} {body} }}"));
    assert!(ws.diagnostics.is_empty(), "{:?}", ws.diagnostics);
    let oi = l.program.objects.iter().position(|o| o.name == "T").unwrap();
    let blocked = l
        .unsupported
        .iter()
        .map(|o| o.iter().map(|r| r.first().cloned()).collect())
        .collect();
    let engine = Engine::new(l.program.clone(), blocked);
    run_codeunit(&engine, &l, oi)
}

fn all_pass(header: &str, body: &str) {
    for (name, outcome) in run_with(header, body) {
        assert_eq!(outcome, Outcome::Pass, "{name}");
    }
}

fn not_run(header: &str, body: &str) -> String {
    match run_with(header, body).remove(0).1 {
        Outcome::NotRun(w) => w,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

const HANDLER: &str = r#"
    var Seen: Text; Calls: Integer; Answer: Boolean;
    [HttpClientHandler]
    procedure Handler(Request: TestHttpRequestMessage; var Response: TestHttpResponseMessage): Boolean
    var Params: Dictionary of [Text, Text]; ParamName: Text; ParamText: Text;
    begin
        Calls += 1;
        Params := Request.QueryParameters;
        foreach ParamName in Params.Keys() do
            ParamText += ParamName + '=' + Params.Get(ParamName) + '&';
        Seen += Format(Request.RequestType) + ' [' + Request.Path + '] params=[' + ParamText + '] secret=' + Format(Request.HasSecretUri) + ';';
        Response.HttpStatusCode := 201;
        Response.ReasonPhrase := 'Made Up';
        Response.Content.WriteFrom('reply ' + Format(Calls));
        Response.Headers.Add('X-Mock', 'yes');
        exit(Answer);
    end;
    local procedure Describe(Ok: Boolean; var Resp: HttpResponseMessage): Text
    var Body: Text; Values: array[1] of Text; Hdr: Text;
    begin
        Resp.Content.ReadAs(Body);
        if Resp.Headers.GetValues('X-Mock', Values) then Hdr := Values[1];
        exit('ok=' + Format(Ok) + ' status=' + Format(Resp.HttpStatusCode) + ' reason=' + Resp.ReasonPhrase + ' body=[' + Body + '] success=' + Format(Resp.IsSuccessStatusCode) + ' blocked=' + Format(Resp.IsBlockedByEnvironment) + ' header=[' + Hdr + ']');
    end;
"#;

// PZHttpHandler.al:EveryMethod_ReachesTheHandler_WithItsRequestMembers.
#[test]
fn every_method_reaches_the_handler_with_its_request_members() {
    all_pass(
        "TestHttpRequestPolicy = AllowOutboundFromHandler;",
        &format!(
            r#"{HANDLER}
    [Test] [HandlerFunctions('Handler')] procedure EveryMethod()
    var Client: HttpClient; Resp: HttpResponseMessage; Req: HttpRequestMessage; Content: HttpContent; Ok: Boolean; D: Text;
    begin
        Ok := Client.Get('https://example.invalid/a/b?x=1&y=two', Resp);
        D := Describe(Ok, Resp);
        if D <> 'ok=Yes status=201 reason=Made Up body=[reply 1] success=Yes blocked=No header=[yes]' then Error('get %1', D);
        Content.WriteFrom('payload');
        Ok := Client.Post('https://example.invalid/post', Content, Resp);
        D := Describe(Ok, Resp);
        if D <> 'ok=Yes status=201 reason=Made Up body=[reply 2] success=Yes blocked=No header=[yes]' then Error('post %1', D);
        Ok := Client.Put('https://example.invalid/put', Content, Resp);
        Ok := Client.Delete('https://example.invalid/del', Resp);
        Ok := Client.Patch('https://example.invalid/patch', Content, Resp);
        Req.Method := 'HEAD';
        Req.SetRequestUri('https://example.invalid/head?q=%20a');
        Ok := Client.Send(Req, Resp);
        D := Describe(Ok, Resp);
        if D <> 'ok=Yes status=201 reason=Made Up body=[reply 6] success=Yes blocked=No header=[yes]' then Error('send %1', D);
        if Calls <> 6 then Error('calls %1', Calls);
        if Seen <> 'Get [https://example.invalid/a/b] params=[x=1&y=two&] secret=No;Post [https://example.invalid/post] params=[] secret=No;Put [https://example.invalid/put] params=[] secret=No;Delete [https://example.invalid/del] params=[] secret=No;Patch [https://example.invalid/patch] params=[] secret=No;Head [https://example.invalid/head] params=[q= a&] secret=No;' then Error('seen %1', Seen);
    end;"#
        ),
    );
}

// PZHttpHandler.al:EmptyHandler_AnswersADefaultResponse.
#[test]
fn a_handler_that_sets_nothing_answers_200_ok() {
    all_pass(
        "TestHttpRequestPolicy = AllowOutboundFromHandler;",
        r#"
    [HttpClientHandler]
    procedure EmptyHandler(Request: TestHttpRequestMessage; var Response: TestHttpResponseMessage): Boolean
    begin
    end;
    [Test] [HandlerFunctions('EmptyHandler')] procedure Empty()
    var Client: HttpClient; Resp: HttpResponseMessage; Body: Text; Ok: Boolean;
    begin
        Ok := Client.Get('https://example.invalid/empty', Resp);
        Resp.Content.ReadAs(Body);
        if not Ok or (Resp.HttpStatusCode <> 200) or (Resp.ReasonPhrase <> 'OK') or (Body <> '') or not Resp.IsSuccessStatusCode or Resp.IsBlockedByEnvironment then
            Error('default response');
    end;"#,
    );
}

// PZHttpHandler.al:HandlerAnsweringTrue_SendsTheRealRequest (BC 29.0): a handler that answers true lets the real request
// go; to an unresolvable name (example.invalid) the call answers false, the handler ran once, and the response is not
// the one the handler could have filled. The runtime never sends: every such request is treated as one to an
// unresolvable host (assumption http-true-is-unresolvable for hosts other than .invalid).
#[test]
fn a_handler_that_answers_true_lets_the_request_fail_like_an_unresolvable_host() {
    all_pass(
        "TestHttpRequestPolicy = AllowOutboundFromHandler;",
        r#"
    var Calls: Integer;
    [HttpClientHandler]
    procedure FallThrough(Request: TestHttpRequestMessage; var Response: TestHttpResponseMessage): Boolean
    begin
        Calls += 1;
        Response.HttpStatusCode := 202;
        exit(true);
    end;
    [Test] [HandlerFunctions('FallThrough')] procedure Fall()
    var Client: HttpClient; Resp: HttpResponseMessage; Ok: Boolean;
    begin
        Ok := Client.Get('https://example.invalid/fall-through', Resp);
        if Ok or (Calls <> 1) or (Resp.HttpStatusCode = 202) then
            Error('fall through');
    end;"#,
    );
}

// PZHttpHandler.al:NoHandler_UnderAllowOutboundFromHandler and PZHttpBlock.al:NoHandler_UnderBlockOutboundRequests.
#[test]
fn a_request_without_a_handler_raises_the_policy_error() {
    for policy in ["AllowOutboundFromHandler", "BlockOutboundRequests"] {
        all_pass(
            &format!("TestHttpRequestPolicy = {policy};"),
            &format!(
                r#"
    [TryFunction] local procedure TryGet(Url: Text) var Client: HttpClient; Resp: HttpResponseMessage; begin if not Client.Get(Url, Resp) then Error('get returned false'); end;
    [Test] procedure NoHandler()
    var Ok: Boolean;
    begin
        Ok := TryGet('https://example.invalid/none');
        if Ok then Error('no error');
        if GetLastErrorText() <> 'Unhandled outbound HTTP requests are not permitted when the TestHttpRequestPolicy property is set to ''{policy}''. Please ensure that an HTTP client handler is configured.' then Error('text %1', GetLastErrorText());
        if GetLastErrorCode() <> 'TestCodeunitUnhandledHttpRequest' then Error('code %1', GetLastErrorCode());
    end;"#
            ),
        );
    }
}

// PZHttpBlock.al:Handler_UnderBlockOutboundRequests_StillAnswers (BC 29).
#[test]
fn a_handler_still_answers_under_block_outbound_requests() {
    all_pass(
        "TestHttpRequestPolicy = BlockOutboundRequests;",
        r#"
    [HttpClientHandler]
    procedure Handler(Request: TestHttpRequestMessage; var Response: TestHttpResponseMessage): Boolean
    begin
        Response.HttpStatusCode := 202;
    end;
    [Test] [HandlerFunctions('Handler')] procedure Handled()
    var Client: HttpClient; Resp: HttpResponseMessage;
    begin
        if not Client.Get('https://example.invalid/handled', Resp) then Error('false');
        if Resp.HttpStatusCode <> 202 then Error('status %1', Resp.HttpStatusCode);
    end;"#,
    );
}

// Without TestHttpRequestPolicy a request would go out for real (network/TestHttpClient.al); the runtime refuses.
#[test]
fn a_request_with_no_handler_and_no_policy_is_refused_not_sent() {
    let why = not_run(
        "",
        r#"
    [Test] procedure Real()
    var Client: HttpClient; Resp: HttpResponseMessage;
    begin
        Client.Get('http://example.invalid', Resp);
    end;"#,
    );
    assert!(why.contains("connection"), "{why}");
}

// An HttpContent passed by reference is the caller's own object: headers handed out before still show it afterwards
// (lt-http-header-rules/P87Content.al:RewriteKeepsHeader reads the body through `Read(var C)` between header changes).
#[test]
fn an_http_content_passed_by_reference_keeps_the_headers_handed_out_before() {
    all_pass(
        "",
        r#"
    local procedure Rd(var C: HttpContent): Text
    var T: Text;
    begin
        C.ReadAs(T);
        exit(T);
    end;
    [Test] procedure Alias()
    var C: HttpContent; H: HttpHeaders; H2: HttpHeaders;
    begin
        C.WriteFrom('x');
        C.GetHeaders(H);
        if Rd(C) <> 'x' then Error('read');
        H.Remove('Content-Type');
        C.GetHeaders(H2);
        if H2.Contains('Content-Type') then Error('the old handle no longer reaches the content');
    end;"#,
    );
}

// Header forms the corpus does not pin are refused (docs/probes-needed.md): a Cache-Control with directives beyond no-store and no-cache, and
// a known header other than Accept written in lower case.
#[test]
fn an_unpinned_cache_control_or_header_case_is_refused() {
    let why = not_run(
        "",
        r#"
    [Test] procedure Order()
    var Req: HttpRequestMessage; H: HttpHeaders;
    begin
        Req.GetHeaders(H);
        H.Add('Cache-Control', 'public, no-cache');
    end;"#,
    );
    assert!(why.contains("Cache-Control") && why.contains("not pinned"), "{why}");
    let why = not_run(
        "",
        r#"
    [Test] procedure Case()
    var Req: HttpRequestMessage; H: HttpHeaders;
    begin
        Req.GetHeaders(H);
        H.Add('user-agent', 'A/1');
    end;"#,
    );
    assert!(why.contains("user-agent") && why.contains("not pinned"), "{why}");
}
