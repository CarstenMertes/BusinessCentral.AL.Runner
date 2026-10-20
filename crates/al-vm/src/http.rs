//! `HttpClient` calls answered by the running test's `[HttpClientHandler]` (ADR-027 section 5). Nothing here opens a
//! connection: a request is given to the handler, raises the policy error, or is refused.
//!
//! Pinned by the staging probes tests/al-language-onprem/session/PZHttpHandler.al (codeunit 61297) and
//! PZHttpBlock.al (61298), BC 29.0.54011.55816.

use crate::Session;
use al_builtins::assumptions::{Assumption, assume};
use al_builtins::describe::Describe;
use al_builtins::http::{fresh_test_response, split_url};
use al_builtins::native::{HttpOutgoing, Signal, raise};
use al_builtins::value::{LibraryValue, TestHttpRequestState, TestHttpResponseState, Value};
use std::cell::RefCell;
use std::rc::Rc;

fn unsupported<T>(what: impl Into<String>) -> Result<T, Signal> {
    Err(Signal::Unsupported(what.into()))
}

/// The policy a `TestHttpRequestPolicy` property names, in the casing BC prints it.
fn canonical_policy(written: &str) -> Option<&'static str> {
    [
        "AllowAllOutboundRequests",
        "BlockOutboundRequests",
        "AllowOutboundFromHandler",
    ]
    .into_iter()
    .find(|p| written.trim().trim_matches('\'').eq_ignore_ascii_case(p))
}

impl Session {
    /// The value of property `name` of object `object`, as written.
    pub fn object_property(&self, object: usize, name: &str) -> Option<String> {
        self.engine.program.objects[object]
            .properties
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.trim().to_string())
    }

    /// Sets the `TestHttpRequestPolicy` of the running test codeunit (its property value as written), or none.
    pub fn set_http_policy(&mut self, written: Option<&str>) {
        self.http_policy = written.and_then(canonical_policy);
    }

    /// The `HttpRequestType` value for `method`: a standard method by name, any other `Unknown`.
    fn http_request_type(&self, method: &str) -> Result<Value, Signal> {
        let members = al_builtins::sysenum::option_members("HttpRequestType").expect("known option type");
        let member = match method {
            "GET" => "Get",
            "POST" => "Post",
            "PUT" => "Put",
            "DELETE" => "Delete",
            "PATCH" => "Patch",
            "HEAD" => "Head",
            "OPTIONS" => "Options",
            "CONNECT" => "Connect",
            "TRACE" => "Trace",
            _ => "Unknown",
        };
        let ordinal = members.iter().position(|m| m == member).expect("a member") as i32;
        Ok(Value::Option(ordinal, members.into()))
    }

    pub(crate) fn http_send_impl(&mut self, request: HttpOutgoing) -> Result<TestHttpResponseState, Signal> {
        let policy = self.http_policy;
        let bound = self.has_http_handler();
        // Before BC 29 a bound handler does not answer under BlockOutboundRequests or AllowOutboundFromHandler: the request
        // raises as with no handler, whatever its path, and the handler is not called (al-language-onprem/session/PZHttpBlock.al:54-57;
        // al-language-onprem/network/lt-ay-http-handler/AyHttpHandler.al, the BC 27 and 28 legs: `calls` 0). The bound handler
        // counts as run: those tests, with `[HandlerFunctions(...)]` and the error expected, pass on real BC 27 and 28
        // (oracle/27/junit-onprem.xml:430, oracle/28/junit-onprem.xml:445).
        let blocked_anyway = match policy {
            Some("BlockOutboundRequests") => !self.has(al_version::Gate::HttpHandlerUnderBlockOutboundFrom29),
            Some("AllowOutboundFromHandler") => !self.has(al_version::Gate::HttpHandlerUnderAllowFromHandlerFrom29),
            _ => false,
        };
        if blocked_anyway && bound {
            self.http_handlers_count_as_run();
        }
        if !bound || blocked_anyway {
            return match policy {
                Some(p @ ("BlockOutboundRequests" | "AllowOutboundFromHandler")) => raise(
                    format!(
                        "Unhandled outbound HTTP requests are not permitted when the TestHttpRequestPolicy property is set to '{p}'. Please ensure that an HTTP client handler is configured."
                    ),
                    "TestCodeunitUnhandledHttpRequest",
                ),
                _ => unsupported(
                    "an outbound HTTP request with no [HttpClientHandler] bound to the test: the runtime never opens a connection (ADR-027)",
                ),
            };
        }
        // A handler is pinned under BlockOutboundRequests (PZHttpBlock.al) and AllowOutboundFromHandler (PZHttpHandler.al,
        // AyHttpHandler.al) only; under the default policy or AllowAllOutboundRequests nothing is measured.
        if !matches!(policy, Some("BlockOutboundRequests" | "AllowOutboundFromHandler")) {
            return unsupported(
                "an [HttpClientHandler] under TestHttpRequestPolicy AllowAllOutboundRequests or no policy: whether it answers is not measured (docs/probes-needed.md)",
            );
        }
        if !request.url.contains("://") {
            // With no base address, `a/b`, `a.b` and `?q=1` raise the empty-uri error before the handler runs
            // (AyHttpHandler.al:NoBaseAddress_RelativePaths_1 and _2, BC 29). Other forms are not measured.
            // The handler is not called, and that does not fail the test (the tests pass on real BC 29).
            if al_builtins::http::is_empty_uri_form(&request.url) {
                self.http_handlers_count_as_run();
                return raise("Invalid URI: The URI is empty.", "AL:UriFormat");
            }
            return unsupported(
                "a request to a relative path with no base address, or with no uri, reaches a [HttpClientHandler]: what the handler sees is not measured",
            );
        }
        if request.unpinned_join {
            assume(Assumption::HttpRelativeRequestPath);
        }
        let (path, params) = split_url(&request.url);
        let query = Value::Dictionary(Rc::new(RefCell::new(
            params
                .into_iter()
                .map(|(k, v)| (Value::Text(k.into()), Value::Text(v.into())))
                .collect(),
        )));
        let req = Value::Library(
            "TestHttpRequestMessage".into(),
            Rc::new(RefCell::new(LibraryValue::TestHttpRequest(TestHttpRequestState {
                path,
                query,
                has_secret_uri: request.has_secret_uri,
                request_type: self.http_request_type(&request.method)?,
            }))),
        );
        let resp = Value::Library(
            "TestHttpResponseMessage".into(),
            Rc::new(RefCell::new(LibraryValue::TestHttpResponse(fresh_test_response()))),
        );
        let Some((answer, resp)) = self.dispatch_http_handler(req, resp)? else {
            return unsupported("the [HttpClientHandler] went away");
        };
        match answer {
            Value::Boolean(false) | Value::Null => {}
            Value::Boolean(true) => {
                // BC then asks whether to allow the real request (StrMenu "Allow Always,Allow Once,Block Always,Block Once"); with
                // no StrMenu handler the call answers false with that unhandled-UI error as the last error, whatever the host
                // (AyHttpHandler.al:Handler_FallThrough_ConnectionRefused, http://localhost:1; PZHttpHandler.al:
                // HandlerAnsweringTrue_SendsTheRealRequest, example.invalid). What a StrMenu handler's choice does is not measured.
                if self.has_ui_handler(crate::ui::UiKind::StrMenu) {
                    return unsupported(
                        "an [HttpClientHandler] answering true with a StrMenuHandler bound: what the allow-request choice does is not measured",
                    );
                }
                let mut failed = fresh_test_response();
                failed.request_failed = true;
                failed.failure = Some((
                    "Unhandled UI: StrMenu Allow Always,Allow Once,Block Always,Block Once".into(),
                    "MissingUIHandler".into(),
                ));
                return Ok(failed);
            }
            other => return unsupported(format!("an [HttpClientHandler] that answers {}", other.describe())),
        }
        match resp {
            Value::Library(_, cell) => match &*cell.borrow() {
                LibraryValue::TestHttpResponse(s) => with_reason_phrase(s.clone()),
                LibraryValue::Empty => Ok(fresh_test_response()),
                _ => unsupported("an [HttpClientHandler] left something else than a TestHttpResponseMessage"),
            },
            _ => unsupported("an [HttpClientHandler] left something else than a TestHttpResponseMessage"),
        }
    }
}

/// A response whose handler set a status but no `ReasonPhrase` gets the status's reason phrase: 200 keeps "OK"
/// (PZHttpHandler.al:EmptyHandler_AnswersADefaultResponse) and 404 gives "Not Found" (AyHttpHandler.al:Handler_OnlyStatus404,
/// BC 29). Another status's phrase is not measured: reading it is refused (`reason_unpinned`).
fn with_reason_phrase(mut s: TestHttpResponseState) -> Result<TestHttpResponseState, Signal> {
    if !s.reason_set {
        match s.status {
            200 => s.reason = "OK".into(),
            404 => s.reason = "Not Found".into(),
            _ => s.reason_unpinned = true,
        }
    }
    Ok(s)
}
