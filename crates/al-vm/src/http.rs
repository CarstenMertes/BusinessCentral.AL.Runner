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
        // Before BC 29 a handler does not answer under BlockOutboundRequests: the request raises as if none were bound
        // (al-language-onprem/session/PZHttpBlock.al:55, the `#else` branch of Handler_UnderBlockOutboundRequests_StillAnswers).
        // The test does not fail for a handler that never ran: that test, with `[HandlerFunctions('Handler')]` (:44) and the
        // error expected, passes on real BC 27 and 28 (oracle/27/junit-onprem.xml:430, oracle/28/junit-onprem.xml:445). So the
        // bound handler counts as executed.
        let mut bound = self.has_http_handler();
        if bound
            && policy == Some("BlockOutboundRequests")
            && !self.has(al_version::Gate::HttpHandlerUnderBlockOutboundRequestsFrom29)
        {
            self.mark_http_handler_fired();
            bound = false;
        }
        if !bound {
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
        if !request.url.contains("://") {
            return unsupported(
                "a request to a relative path with no base address, or with no uri, reaches a [HttpClientHandler]: what the handler sees is not measured",
            );
        }
        if request.joined_with_base {
            assume(Assumption::HttpRelativeRequestPath);
        }
        assume(Assumption::HttpHandlerEveryVersion);
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
                // BC then sends the request for real. The runtime never opens a connection, so the request fails the way
                // one to an unresolvable host does (session/PZHttpHandler.al:HandlerAnsweringTrue_SendsTheRealRequest,
                // example.invalid: the call answers false). The handler's response is dropped.
                assume(Assumption::HttpTrueIsUnresolvable);
                let mut failed = fresh_test_response();
                failed.request_failed = true;
                failed.status = 0;
                failed.reason = String::new();
                failed.success = false;
                return Ok(failed);
            }
            other => return unsupported(format!("an [HttpClientHandler] that answers {}", other.describe())),
        }
        match resp {
            Value::Library(_, cell) => match &*cell.borrow() {
                LibraryValue::TestHttpResponse(s) => Ok(s.clone()),
                LibraryValue::Empty => Ok(fresh_test_response()),
                _ => unsupported("an [HttpClientHandler] left something else than a TestHttpResponseMessage"),
            },
            _ => unsupported("an [HttpClientHandler] left something else than a TestHttpResponseMessage"),
        }
    }
}
