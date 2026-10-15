//! `HttpRequestMessage`: the locally configured parts of a request (corpus network/TestHttpRequestMessage.al).
//! No request is ever sent: `HttpClient` is not implemented.

use crate::describe::Describe;
use crate::native::{Ctx, NativeFn, Signal};
use crate::signature::Signature;
use crate::value::{HttpContentState, HttpHeaderList, HttpRequestState, LibraryValue, Value};

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
        Some(t) => Ok(t.to_string()),
        None => unsupported(format!("a text argument, got {}", a.get(i).describe())),
    }
}

/// `Method := 'GET'` sets and `Method` reads it back as written (network/TestHttpRequestMessage.al:19-25;
/// types/TestHttpRequestFileUploadFieldErrorContracts.al:HttpRequestMessage_Defaults_AndSetRequestUri keeps
/// `post` as `post`). A request nobody gave a method is `GET` (same test).
fn method(_: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> Out {
    if a.is_empty() {
        return Ok(Value::Text(
            with_request(r, |s| s.method.clone())?
                .unwrap_or_else(|| "GET".to_string())
                .into(),
        ));
    }
    let m = text(a, 0)?;
    with_request(r, |s| s.method = Some(m)).map(|()| Value::Null)
}

/// Whether `SetRequestUri` takes `uri`: `Some(true)` for an absolute uri (`scheme://rest`, no white space),
/// `Some(false)` for text that has white space and no `://` (`not a uri`), and `None` for any other text,
/// which is not pinned (types/TestHttpRequestFileUploadFieldErrorContracts.al,
/// network/TestHttpRequestMessage.al:46-55).
fn uri_validity(uri: &str) -> Option<bool> {
    let scheme_end = uri.find("://");
    let absolute = scheme_end.is_some_and(|i| {
        i > 0
            && uri[..i].starts_with(|c: char| c.is_ascii_alphabetic())
            && uri[..i].chars().all(|c| c.is_ascii_alphanumeric() || "+.-".contains(c))
            && uri.len() > i + 3
            && !uri.chars().any(char::is_whitespace)
    });
    if absolute {
        Some(true)
    } else if scheme_end.is_none() && uri.chars().any(char::is_whitespace) {
        Some(false)
    } else {
        None
    }
}

/// `SetRequestUri(uri)` stores a valid uri and `GetRequestUri()` returns it unchanged; a text that is no uri
/// answers false and keeps the uri that was there, or with the result unused raises `Failed to create the URL
/// for the request. Please, make sure to set a valid URL.` / `CreateUrl:UriFormat`
/// (handlers/PWRefusedBuiltins.al:HttpRequestMessage_SetRequestUri_ValidAndInvalid).
fn set_request_uri(c: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> Out {
    let u = text(a, 0)?;
    match uri_validity(&u) {
        Some(true) => with_request(r, |s| s.uri = Some(u)).map(|()| Value::Boolean(true)),
        Some(false) if c.ret_used() => Ok(Value::Boolean(false)),
        Some(false) => Err(Signal::Error(Box::new(crate::native::AlError {
            message: "Failed to create the URL for the request. Please, make sure to set a valid URL.".into(),
            code: "CreateUrl:UriFormat".into(),
            call_stack: String::new(),
        }))),
        None => unsupported(format!(
            "SetRequestUri of `{u}`: whether it is a valid uri is not pinned (only an absolute uri and white space without a scheme are)"
        )),
    }
}

/// An empty uri before one is set (types/TestHttpRequestFileUploadFieldErrorContracts.al:HttpRequestMessage_Defaults_AndSetRequestUri).
fn get_request_uri(_: &mut dyn Ctx, r: Option<&mut Value>, _: &mut [Value]) -> Out {
    Ok(Value::Text(
        with_request(r, |s| s.uri.clone())?.unwrap_or_default().into(),
    ))
}

/// A `FileUpload` no client filled: `FileName()` is a null string (assigned to a Text it is empty) and
/// `CreateInStream` opens an empty stream without an error
/// (types/TestHttpRequestFileUploadFieldErrorContracts.al:FileUpload_NoClientFilled_OpensAnEmptyStream).
/// A filled FileUpload cannot exist here: there is no client.
fn file_upload_name(_: &mut dyn Ctx, _: Option<&mut Value>, _: &mut [Value]) -> Out {
    Ok(Value::Null)
}

fn file_upload_create_in(_: &mut dyn Ctx, _: Option<&mut Value>, a: &mut [Value]) -> Out {
    crate::stream::create_empty_in_stream(a)?;
    Ok(Value::Null)
}

fn with_content<R>(r: Option<&mut Value>, f: impl FnOnce(&mut HttpContentState) -> R) -> Result<R, Signal> {
    let Some(Value::Library(_, cell)) = r else {
        return unsupported("HttpContent method without a receiver");
    };
    let mut lib = cell.borrow_mut();
    if matches!(&*lib, LibraryValue::Empty) {
        *lib = LibraryValue::HttpContent(HttpContentState::default());
    }
    match &mut *lib {
        LibraryValue::HttpContent(s) => Ok(f(s)),
        other => unsupported(format!("HttpContent method on {}", other.describe())),
    }
}

fn headers_of(v: Option<&Value>) -> Result<HttpHeaderList, Signal> {
    let Some(Value::Library(_, cell)) = v else {
        return unsupported("HttpHeaders method without a receiver");
    };
    match &mut *cell.borrow_mut() {
        LibraryValue::HttpHeaders(h) => Ok(h.clone()),
        LibraryValue::Empty => unsupported("HttpHeaders of a variable GetHeaders was not called on"),
        other => unsupported(format!("HttpHeaders method on {}", other.describe())),
    }
}

/// `WriteFrom(Text)` stores the body and `ReadAs(var Text)` gives it back unchanged
/// (network/TestHttpRequestMessage.al:34-43). The encoding it reads with, a default Content-Type and what
/// `ReadAs` answers for a content nobody wrote to are not pinned.
fn write_from(_: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> Out {
    let t = text(a, 0)?;
    with_content(r, |s| {
        s.body = Some(t);
        s.bytes = None;
    })
    .map(|()| Value::Null)
}

/// `WriteFrom(InStream)` takes the bytes the stream has left. Reading them back as Text is refused.
fn write_from_stream(_: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> Out {
    let bytes = crate::stream::drain_in(&mut a[0])?;
    with_content(r, |s| {
        s.bytes = Some(bytes);
        s.body = None;
    })
    .map(|()| Value::Null)
}

fn read_as(_: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> Out {
    match with_content(r, |s| (s.body.clone(), s.bytes.is_some()))? {
        (Some(b), _) => {
            a[0] = Value::Text(b.into());
            Ok(Value::Boolean(true))
        }
        (None, true) => unsupported(
            "HttpContent.ReadAs(Text) of a content written from a stream: the encoding it reads with is not pinned",
        ),
        (None, false) => unsupported(
            "HttpContent.ReadAs of a content nobody wrote to: what it answers is not pinned (docs/probes-needed.md)",
        ),
    }
}

/// `GetHeaders(var Headers)` hands out the content's own header list (network/TestHttpRequestMessage.al:39).
fn get_headers(_: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> Out {
    let list = with_content(r, |s| s.headers.clone())?;
    let Some(Value::Library(_, cell)) = a.first() else {
        return unsupported("HttpContent.GetHeaders without an HttpHeaders variable");
    };
    *cell.borrow_mut() = LibraryValue::HttpHeaders(list);
    Ok(Value::Boolean(true))
}

/// `Add(Name, Value)` appends a value to the header; `Remove(Name)` drops it, whether or not it was there
/// (network/TestHttpRequestMessage.al:40-41 removes `Content-Type` from a content that has none and adds it).
/// Header names compare without case, as HTTP does. Whether `Add` validates the name or the value, and
/// the answer of `Remove` for a missing header, are not pinned.
fn headers_add(_: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> Out {
    let list = headers_of(r.as_deref())?;
    let (name, value) = (text(a, 0)?, text(a, 1)?);
    let mut list = list.borrow_mut();
    match list.iter_mut().find(|(n, _)| n.eq_ignore_ascii_case(&name)) {
        Some((_, v)) => v.push(value),
        None => list.push((name, vec![value])),
    }
    Ok(Value::Boolean(true))
}

fn headers_remove(_: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> Out {
    let list = headers_of(r.as_deref())?;
    let name = text(a, 0)?;
    list.borrow_mut().retain(|(n, _)| !n.eq_ignore_ascii_case(&name));
    Ok(Value::Boolean(true))
}

/// `Content := C` stores the content, `Content` reads it back (network/TestHttpRequestMessage.al:38-43).
/// A request nobody gave a content: what `Content` answers is not pinned.
fn content(_: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> Out {
    if let Some(c) = a.first() {
        let c = c.clone();
        return with_request(r, |s| s.content = Some(c)).map(|()| Value::Null);
    }
    match with_request(r, |s| s.content.clone())? {
        Some(c) => Ok(c),
        None => unsupported(
            "HttpRequestMessage.Content of a request nobody set a content on: the default is not pinned (docs/probes-needed.md)",
        ),
    }
}

pub fn native_for(sig: &Signature) -> Option<NativeFn> {
    let one_text = sig.params.len() == 1 && sig.params[0].ty == "Text";
    match (sig.receiver.as_str(), sig.name.as_str()) {
        ("HttpContent", "WriteFrom") if one_text => return Some(write_from),
        ("HttpContent", "WriteFrom") if sig.params.len() == 1 && sig.params[0].ty == "InStream" => {
            return Some(write_from_stream);
        }
        ("HttpContent", "ReadAs") if sig.params.len() == 1 && sig.params[0].ty == "Text" => return Some(read_as),
        ("HttpContent", "GetHeaders") => return Some(get_headers),
        ("HttpHeaders", "Add") if sig.params.len() == 2 && sig.params[1].ty == "Text" => return Some(headers_add),
        ("HttpHeaders", "Remove") => return Some(headers_remove),
        ("HttpRequestMessage", "Content") => return Some(content),
        ("FileUpload", "FileName") => return Some(file_upload_name),
        ("FileUpload", "CreateInStream") => return Some(file_upload_create_in),
        _ => {}
    }
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
