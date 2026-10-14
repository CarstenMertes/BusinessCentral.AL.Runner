//! `HttpRequestMessage`: the locally configured parts of a request (corpus network/TestHttpRequestMessage.al).
//! No request is ever sent: `HttpClient` is not implemented.

use crate::describe::Describe;
use crate::native::{Ctx, NativeFn, Signal};
use crate::signature::Signature;
use crate::value::{HttpRequestState, LibraryValue, Value};

type Out = Result<Value, Signal>;

fn unsupported<T>(what: impl Into<String>) -> Result<T, Signal> {
    Err(Signal::Unsupported(what.into()))
}

fn with_request<R>(r: Option<&mut Value>, f: impl FnOnce(&mut HttpRequestState) -> R) -> Result<R, Signal> {
    let Some(Value::Library(_, cell)) = r else {
        return unsupported("HttpRequestMessage method without a receiver");
    };
    let mut lib = cell.borrow_mut();
    if matches!(&*lib, LibraryValue::Empty) {
        *lib = LibraryValue::HttpRequest(HttpRequestState::default());
    }
    match &mut *lib {
        LibraryValue::HttpRequest(s) => Ok(f(s)),
        other => unsupported(format!("HttpRequestMessage method on {}", other.describe())),
    }
}

fn text(a: &[Value], i: usize) -> Result<String, Signal> {
    match a.get(i).and_then(crate::native::text_of) {
        Some(t) => Ok(t),
        None => unsupported(format!("a text argument, got {}", a.get(i).describe())),
    }
}

/// `Method := 'GET'` sets and `Method` reads it back (network/TestHttpRequestMessage.al:19-25). The
/// method of a request nobody gave one is not pinned: reading it is refused.
fn method(_: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> Out {
    if a.is_empty() {
        return match with_request(r, |s| s.method.clone())? {
            Some(m) => Ok(Value::Text(m)),
            None => unsupported(
                "HttpRequestMessage.Method of a request nobody set a method on: the default is not pinned (docs/probes-needed.md)",
            ),
        };
    }
    let m = text(a, 0)?;
    with_request(r, |s| s.method = Some(m)).map(|()| Value::Null)
}

/// `SetRequestUri(uri)` stores it and `GetRequestUri()` returns it unchanged
/// (network/TestHttpRequestMessage.al:46-55). Whether `SetRequestUri` validates the text, and what
/// an unset uri reads as, are not pinned.
fn set_request_uri(_: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> Out {
    let u = text(a, 0)?;
    with_request(r, |s| s.uri = Some(u)).map(|()| Value::Boolean(true))
}

fn get_request_uri(_: &mut dyn Ctx, r: Option<&mut Value>, _: &mut [Value]) -> Out {
    match with_request(r, |s| s.uri.clone())? {
        Some(u) => Ok(Value::Text(u)),
        None => unsupported(
            "HttpRequestMessage.GetRequestUri of a request nobody set a uri on: the default is not pinned (docs/probes-needed.md)",
        ),
    }
}

pub fn native_for(sig: &Signature) -> Option<NativeFn> {
    if sig.receiver != "HttpRequestMessage" {
        return None;
    }
    match sig.name.as_str() {
        "Method" => Some(method),
        "SetRequestUri" if sig.params.len() == 1 && sig.params[0].ty == "Text" => Some(set_request_uri),
        "GetRequestUri" => Some(get_request_uri),
        _ => None,
    }
}
