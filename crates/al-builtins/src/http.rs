//! `HttpClient`, `HttpRequestMessage`, `HttpResponseMessage`, `HttpContent`, `HttpHeaders` and the handler-side
//! `TestHttpRequestMessage` / `TestHttpResponseMessage` (ADR-027 section 5, owner decision 1).
//!
//! No request ever leaves the process. `HttpClient.Get`, `Post`, `Put`, `Delete`, `Patch` and `Send` hand a
//! request to the session (`Ctx::http_send`), which gives it to the `[HttpClientHandler]` of the running test,
//! raises the policy error, or refuses.
//!
//! Pinned by: corpus network/TestHttpRequestMessage.al and misc/W4XmlJsonHttp.al; the staging probes
//! tests/al-language-onprem/session/PZHttpHandler.al and PZHttpBlock.al (BC 29.0.54011.55816) and
//! tests/al-language/network/lt-http/*.al (the laptop's HTTP topic, BC 29.0.54011.55816). What those do not
//! measure is refused or marked in the assumption registry.

use crate::describe::Describe;
use crate::native::{Ctx, HttpOutgoing, NativeFn, Signal, raise};
use crate::signature::Signature;
use crate::value::{
    CookieState, HeaderBag, HeaderKind, HttpClientState, HttpContentState, HttpHeaderList, HttpRequestState,
    HttpResponseState, LibraryValue, TestHttpRequestState, TestHttpResponseState, Value, new_header_list,
};
use std::cell::RefCell;
use std::rc::Rc;

type Out = Result<Value, Signal>;

fn unsupported<T>(what: impl Into<String>) -> Result<T, Signal> {
    Err(Signal::Unsupported(what.into()))
}

fn assume(a: crate::assumptions::Assumption) {
    crate::assumptions::assume(a);
}

fn lib(name: &str, v: LibraryValue) -> Value {
    Value::Library(name.into(), Rc::new(RefCell::new(v)))
}

fn text(a: &[Value], i: usize) -> Result<String, Signal> {
    match a.get(i).and_then(crate::native::text_of) {
        Some(t) => Ok(t.to_string()),
        None => unsupported(format!("a text argument, got {}", a.get(i).describe())),
    }
}

/// The text of a Text or SecretText argument, and whether it was a SecretText.
fn text_or_secret(a: &[Value], i: usize) -> Result<(String, bool), Signal> {
    match a.get(i) {
        Some(Value::Library(_, h)) => match &*h.borrow() {
            LibraryValue::Empty => Ok((String::new(), true)),
            LibraryValue::Text(s) => Ok((s.clone(), true)),
            other => unsupported(format!("a SecretText argument, got {}", other.describe())),
        },
        _ => Ok((text(a, i)?, false)),
    }
}

// ------------------------------------------------------------------------------------------------ receivers

fn receiver_cell(r: Option<&mut Value>, what: &str) -> Result<Rc<RefCell<LibraryValue>>, Signal> {
    match r {
        Some(Value::Library(_, cell)) => Ok(cell.clone()),
        _ => unsupported(format!("{what} method without a receiver")),
    }
}

fn with_request<R>(r: Option<&mut Value>, f: impl FnOnce(&mut HttpRequestState) -> R) -> Result<R, Signal> {
    let cell = receiver_cell(r, "HttpRequestMessage")?;
    let mut lib = cell.borrow_mut();
    if matches!(&*lib, LibraryValue::Empty) {
        *lib = LibraryValue::HttpRequest(HttpRequestState::default());
    }
    match &mut *lib {
        LibraryValue::HttpRequest(s) => Ok(f(s)),
        other => unsupported(format!("HttpRequestMessage method on {}", other.describe())),
    }
}

fn with_content<R>(r: Option<&mut Value>, f: impl FnOnce(&mut HttpContentState) -> R) -> Result<R, Signal> {
    let cell = receiver_cell(r, "HttpContent")?;
    let mut lib = cell.borrow_mut();
    if matches!(&*lib, LibraryValue::Empty) {
        *lib = LibraryValue::HttpContent(HttpContentState::default());
    }
    match &mut *lib {
        LibraryValue::HttpContent(s) => Ok(f(s)),
        other => unsupported(format!("HttpContent method on {}", other.describe())),
    }
}

fn with_client<R>(r: Option<&mut Value>, f: impl FnOnce(&mut HttpClientState) -> R) -> Result<R, Signal> {
    let cell = receiver_cell(r, "HttpClient")?;
    let mut lib = cell.borrow_mut();
    if matches!(&*lib, LibraryValue::Empty) {
        *lib = LibraryValue::HttpClient(HttpClientState::default());
    }
    match &mut *lib {
        LibraryValue::HttpClient(s) => Ok(f(s)),
        other => unsupported(format!("HttpClient method on {}", other.describe())),
    }
}

fn new_content() -> Value {
    lib("HttpContent", LibraryValue::HttpContent(HttpContentState::default()))
}

fn fresh_response() -> HttpResponseState {
    HttpResponseState {
        status: 200,
        reason: "OK".into(),
        content: new_content(),
        headers: new_header_list(HeaderKind::Response),
        blocked: false,
    }
}

fn with_response<R>(r: Option<&mut Value>, f: impl FnOnce(&mut HttpResponseState) -> R) -> Result<R, Signal> {
    let cell = receiver_cell(r, "HttpResponseMessage")?;
    let mut lib = cell.borrow_mut();
    if matches!(&*lib, LibraryValue::Empty) {
        *lib = LibraryValue::HttpResponse(fresh_response());
    }
    match &mut *lib {
        LibraryValue::HttpResponse(s) => Ok(f(s)),
        other => unsupported(format!("HttpResponseMessage method on {}", other.describe())),
    }
}

/// What a handler is given to fill: 200 OK, successful, nothing blocked, an empty body
/// (network/lt-http/TestHttpMessages.al:FreshTestResponse).
pub fn fresh_test_response() -> TestHttpResponseState {
    TestHttpResponseState {
        request_failed: false,
        status: 200,
        reason: "OK".into(),
        success: true,
        blocked: false,
        content: new_content(),
        headers: new_header_list(HeaderKind::Response),
    }
}

fn with_test_response<R>(r: Option<&mut Value>, f: impl FnOnce(&mut TestHttpResponseState) -> R) -> Result<R, Signal> {
    let cell = receiver_cell(r, "TestHttpResponseMessage")?;
    let mut lib = cell.borrow_mut();
    if matches!(&*lib, LibraryValue::Empty) {
        *lib = LibraryValue::TestHttpResponse(fresh_test_response());
    }
    match &mut *lib {
        LibraryValue::TestHttpResponse(s) => Ok(f(s)),
        other => unsupported(format!("TestHttpResponseMessage method on {}", other.describe())),
    }
}

fn with_test_request<R>(r: Option<&mut Value>, f: impl FnOnce(&mut TestHttpRequestState) -> R) -> Result<R, Signal> {
    let cell = receiver_cell(r, "TestHttpRequestMessage")?;
    let mut lib = cell.borrow_mut();
    if matches!(&*lib, LibraryValue::Empty) {
        *lib = LibraryValue::TestHttpRequest(TestHttpRequestState {
            path: String::new(),
            query: Value::Dictionary(Rc::new(RefCell::new(Vec::new()))),
            has_secret_uri: false,
            request_type: Value::Null,
        });
    }
    match &mut *lib {
        LibraryValue::TestHttpRequest(s) => Ok(f(s)),
        other => unsupported(format!("TestHttpRequestMessage method on {}", other.describe())),
    }
}

fn headers_value(list: &HttpHeaderList) -> Value {
    lib("HttpHeaders", LibraryValue::HttpHeaders(list.clone()))
}

/// Points the `var Headers` argument at `list`.
fn hand_out_headers(arg: Option<&Value>, list: &HttpHeaderList) -> Result<(), Signal> {
    let Some(Value::Library(_, cell)) = arg else {
        return unsupported("GetHeaders without an HttpHeaders variable");
    };
    *cell.borrow_mut() = LibraryValue::HttpHeaders(list.clone());
    Ok(())
}

// ------------------------------------------------------------------------------------------------ uri

const INVALID_URL: &str = "Failed to create the URL for the request. Please, make sure to set a valid URL.";

/// What `System.Uri` makes of a text, for the forms the probes measured (network/lt-http/HttpRequest.al:
/// SetRequestUriValid, SetRequestUriRelativeAndInvalid, SetRequestUriOtherSchemes, SetRequestUriLongAndUnicode).
/// `Err(false)` is an invalid uri, `Err(true)` a text whose validity is not measured.
pub fn normalize_uri(input: &str) -> Result<String, bool> {
    if input.is_empty() {
        return Err(false);
    }
    // A rooted path is a file path on this platform: file:///relative/path.
    if input.starts_with('/') && !input.starts_with("//") {
        return Ok(format!("file://{}", escape(input)));
    }
    let Some(colon) = input.find(':') else {
        return if input.chars().any(char::is_whitespace) {
            Err(false)
        } else {
            Err(true)
        };
    };
    let scheme = &input[..colon];
    let scheme_ok = !scheme.is_empty()
        && scheme.starts_with(|c: char| c.is_ascii_alphabetic())
        && scheme.chars().all(|c| c.is_ascii_alphanumeric() || "+.-".contains(c));
    let rest = &input[colon + 1..];
    if !scheme_ok {
        return if rest.starts_with("//") || input.chars().any(char::is_whitespace) {
            Err(false)
        } else {
            Err(true)
        };
    }
    let scheme = scheme.to_ascii_lowercase();
    let Some(after) = rest.strip_prefix("//") else {
        // mailto:a@b.c, javascript:alert(1): kept as written.
        return Ok(format!("{scheme}:{rest}"));
    };
    let end = after.find(['/', '?', '#']).unwrap_or(after.len());
    let (authority, tail) = after.split_at(end);
    let (userinfo, hostport) = match authority.rfind('@') {
        Some(i) => (Some(&authority[..i]), &authority[i + 1..]),
        None => (None, authority),
    };
    let (host, port) = match hostport.rfind(':') {
        Some(i) if !hostport[i..].contains(']') => (&hostport[..i], Some(&hostport[i + 1..])),
        _ => (hostport, None),
    };
    let file_like = scheme == "file";
    if host.is_empty() && !file_like {
        return Err(false);
    }
    if host
        .chars()
        .any(|c| c.is_whitespace() || c.is_control() || "\\^|<>\"`{}".contains(c))
    {
        return Err(false);
    }
    let port = match port {
        None | Some("") => None,
        Some(p) => match p.parse::<u32>() {
            Ok(n) if n <= 65535 && p.chars().all(|c| c.is_ascii_digit()) => Some(n),
            _ => return Err(false),
        },
    };
    let default_port = match scheme.as_str() {
        "http" | "ws" => Some(80),
        "https" | "wss" => Some(443),
        "ftp" => Some(21),
        _ => None,
    };
    let port = port.filter(|p| Some(*p) != default_port);
    let (path_query, fragment) = match tail.find('#') {
        Some(i) => (&tail[..i], Some(&tail[i + 1..])),
        None => (tail, None),
    };
    let (path, query) = match path_query.find('?') {
        Some(i) => (&path_query[..i], Some(&path_query[i + 1..])),
        None => (path_query, None),
    };
    let path = if path.is_empty() && !file_like {
        "/".to_string()
    } else {
        escape(&path.replace('\\', "/"))
    };
    let mut out = format!("{scheme}://");
    if let Some(u) = userinfo {
        out.push_str(u);
        out.push('@');
    }
    out.push_str(&host.to_ascii_lowercase());
    if let Some(p) = port {
        out.push_str(&format!(":{p}"));
    }
    out.push_str(&path);
    if let Some(q) = query {
        out.push('?');
        out.push_str(&escape(q));
    }
    if let Some(f) = fragment {
        out.push('#');
        out.push_str(&escape(f));
    }
    Ok(out)
}

/// Percent-encodes what `System.Uri` encodes in a path or query: space, non-ASCII (as UTF-8) and the characters
/// `"<>^`{|}`.
fn escape(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if c.is_ascii() && !c.is_ascii_control() && !" \"<>^`{|}".contains(c) {
            out.push(c);
        } else {
            let mut buf = [0u8; 4];
            for b in c.encode_utf8(&mut buf).bytes() {
                out.push_str(&format!("%{b:02X}"));
            }
        }
    }
    out
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && b.len() >= i + 3 {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Resolves `path` against the client's base address the way `new Uri(base, path)` does for the forms in use.
fn resolve_request_path(base: &str, path: &str) -> Result<String, Signal> {
    if path.contains("://") || base.is_empty() {
        return match normalize_uri(path) {
            Ok(u) => Ok(u),
            Err(false) => invalid_url(),
            Err(true) => unsupported(format!(
                "HttpClient with the request path `{path}` and no base address: whether it is valid is not measured"
            )),
        };
    }
    assume(crate::assumptions::Assumption::HttpRelativeRequestPath);
    let Some(after) = base.find("://").map(|i| i + 3) else {
        return unsupported("HttpClient with a base address that has no authority");
    };
    let path_start = base[after..].find('/').map_or(base.len(), |i| after + i);
    let (origin, base_path) = base.split_at(path_start);
    let joined = if let Some(rooted) = path.strip_prefix('/') {
        format!("{origin}/{rooted}")
    } else {
        let dir = &base_path[..base_path.rfind('/').map_or(0, |i| i + 1)];
        format!("{origin}{dir}{path}")
    };
    normalize_uri(&joined).or_else(|_| invalid_url())
}

fn invalid_url<T>() -> Result<T, Signal> {
    raise(INVALID_URL, "CreateUrl:UriFormat")
}

/// `SetRequestUri` and `SetBaseAddress`: a valid uri is kept in its normal form; an invalid one answers false when
/// the result is used and raises otherwise (handlers/PWRefusedBuiltins.al:HttpRequestMessage_SetRequestUri_ValidAndInvalid).
fn checked_uri(c: &dyn Ctx, u: &str) -> Result<Option<String>, Signal> {
    match normalize_uri(u) {
        Ok(n) => Ok(Some(n)),
        Err(false) if c.ret_used() => Ok(None),
        Err(false) => invalid_url(),
        Err(true) => unsupported(format!(
            "a request uri of `{u}`: whether it is valid is not measured (an absolute uri, white space without a scheme, and the forms of network/lt-http/HttpRequest.al are)"
        )),
    }
}

/// `Method := 'GET'` sets and `Method` reads it back as written (network/TestHttpRequestMessage.al:19-25;
/// network/lt-http/HttpRequest.al:MethodValues). A request nobody gave a method is `GET`. A method with a space
/// or a colon raises `The format of the HTTP method is invalid.` (`AL:Format`).
fn method(_: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> Out {
    if a.is_empty() {
        return Ok(Value::Text(
            with_request(r, |s| s.method.clone())?
                .unwrap_or_else(|| "GET".to_string())
                .into(),
        ));
    }
    let m = text(a, 0)?;
    if m.is_empty() || m.chars().any(|c| !is_token_char(c)) {
        return raise("The format of the HTTP method is invalid.", "AL:Format");
    }
    with_request(r, |s| s.method = Some(m)).map(|()| Value::Null)
}

fn set_request_uri(c: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> Out {
    let u = text(a, 0)?;
    match checked_uri(c, &u)? {
        Some(n) => with_request(r, |s| s.uri = Some(n)).map(|()| Value::Boolean(true)),
        None => Ok(Value::Boolean(false)),
    }
}

fn get_request_uri(_: &mut dyn Ctx, r: Option<&mut Value>, _: &mut [Value]) -> Out {
    Ok(Value::Text(
        with_request(r, |s| s.uri.clone())?.unwrap_or_default().into(),
    ))
}

fn request_get_headers(_: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> Out {
    let list = with_request(r, |s| s.headers.clone())?;
    hand_out_headers(a.first(), &list)?;
    Ok(Value::Boolean(true))
}

/// `Content := C` stores a copy of the content (misc/W4XmlJsonHttp.al:RequestContent_IsCopyOrReference: writing
/// the original afterwards does not change the request's), `Content` reads the request's own content. A request
/// nobody set a content on has an uninitialized one (network/lt-http/HttpRequest.al:FreshRequestContent).
fn request_content(_: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> Out {
    if let Some(c) = a.first() {
        let copy = copy_content(c)?;
        return with_request(r, |s| s.content = Some(copy)).map(|()| Value::Null);
    }
    match with_request(r, |s| s.content.clone())? {
        Some(c) => Ok(c),
        None => Ok(lib(
            "HttpContent",
            LibraryValue::HttpContent(HttpContentState {
                uninitialized: true,
                ..HttpContentState::default()
            }),
        )),
    }
}

/// An `HttpContent` value copied: its own data and its own headers.
pub fn copy_content(v: &Value) -> Result<Value, Signal> {
    match v {
        Value::Library(_, cell) => match &*cell.borrow() {
            LibraryValue::Empty => Ok(new_content()),
            LibraryValue::HttpContent(s) => Ok(lib("HttpContent", LibraryValue::HttpContent(s.deep_copy()))),
            other => unsupported(format!("an HttpContent was expected, got {}", other.describe())),
        },
        other => unsupported(format!("an HttpContent was expected, got {}", other.describe())),
    }
}

// ------------------------------------------------------------------------------------------------ headers

const INVALID_HEADER: &str = "Invalid HTTP header. Please, make sure the format of the header is correct.";
const INVALID_OPERATION: &str = "The requested operation cannot be performed in this context.";

fn is_token_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || "!#$%&'*+-.^_`|~".contains(c)
}

/// The headers `HttpContentHeaders` owns: a request, response or client header collection refuses them.
fn is_content_header(name: &str) -> bool {
    [
        "allow",
        "content-disposition",
        "content-encoding",
        "content-language",
        "content-length",
        "content-location",
        "content-md5",
        "content-range",
        "content-type",
        "expires",
        "last-modified",
    ]
    .iter()
    .any(|h| name.eq_ignore_ascii_case(h))
}

fn header_error<T>(code: &str) -> Result<T, Signal> {
    raise(INVALID_HEADER, &format!("InvalidHeader:{code}"))
}

/// Whether `v` is `type/subtype` with optional `;name=value` parameters.
fn is_media_type(v: &str) -> bool {
    let mut parts = v.split(';');
    let main = parts.next().unwrap_or("").trim();
    let Some((t, st)) = main.split_once('/') else {
        return false;
    };
    let token = |s: &str| !s.is_empty() && s.chars().all(is_token_char);
    token(t)
        && token(st)
        && parts.all(|p| matches!(p.split_once('='), Some((n, v)) if token(n.trim()) && !v.trim().is_empty()))
}

/// An RFC 1123 date (`Tue, 15 Nov 1994 08:12:31 GMT`).
fn is_http_date(v: &str) -> bool {
    let p: Vec<&str> = v.split_whitespace().collect();
    if p.len() != 6 || !p[0].ends_with(',') || p[5] != "GMT" {
        return false;
    }
    let days = ["Mon,", "Tue,", "Wed,", "Thu,", "Fri,", "Sat,", "Sun,"];
    let months = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let time_ok = p[4].split(':').count() == 3
        && p[4]
            .split(':')
            .all(|x| x.len() == 2 && x.chars().all(|c| c.is_ascii_digit()));
    days.contains(&p[0])
        && p[1].len() <= 2
        && p[1].chars().all(|c| c.is_ascii_digit())
        && months.contains(&p[2])
        && p[3].len() == 4
        && p[3].chars().all(|c| c.is_ascii_digit())
        && time_ok
}

/// The checks `HttpHeaders.Add` makes beyond the name and the value: the headers .NET types (the probes measured
/// Accept, Authorization, Date, Content-Type); the others are accepted as written (registry entry http-header-rules).
fn check_typed(bag: &HeaderBag, name: &str, value: &str) -> Result<(), Signal> {
    let lower = name.to_ascii_lowercase();
    let existing = find(&bag.items, name).is_some();
    match lower.as_str() {
        "accept" => {
            if value.split(',').all(|m| {
                let m = m.trim();
                !m.is_empty() && (is_media_type(m) || m.starts_with("*/"))
            }) {
                Ok(())
            } else {
                header_error("Format")
            }
        }
        "authorization" | "proxy-authorization" => {
            let scheme = value.trim().split(' ').next().unwrap_or("");
            if !scheme.is_empty() && scheme.chars().all(is_token_char) {
                Ok(())
            } else {
                header_error("Format")
            }
        }
        "date" | "if-modified-since" | "if-unmodified-since" | "last-modified" | "expires" => {
            if existing || !is_http_date(value.trim()) {
                header_error("Format")
            } else {
                Ok(())
            }
        }
        "content-type" => {
            if existing || !is_media_type(value.trim()) {
                header_error("Format")
            } else {
                Ok(())
            }
        }
        _ => Ok(()),
    }
}

fn find<'a>(items: &'a [(String, Vec<String>)], name: &str) -> Option<&'a (String, Vec<String>)> {
    items.iter().find(|(n, _)| n.eq_ignore_ascii_case(name))
}

fn push_value(items: &mut Vec<(String, Vec<String>)>, name: &str, value: String) {
    match items.iter_mut().find(|(n, _)| n.eq_ignore_ascii_case(name)) {
        Some((_, v)) => v.push(value),
        None => items.push((name.to_string(), vec![value])),
    }
}

/// `HttpHeaders.Add` (network/lt-http/HttpHeaders.al, HttpClient.al, HttpContent.al, HttpResponse.al).
fn add_header(bag: &mut HeaderBag, name: &str, value: &str, secret: bool) -> Result<(), Signal> {
    assume(crate::assumptions::Assumption::HttpHeaderRules);
    if name.is_empty() {
        return header_error("Argument");
    }
    if name.chars().any(|c| !is_token_char(c)) {
        return header_error("Format");
    }
    if bag.kind != HeaderKind::Content && is_content_header(name) {
        return raise(INVALID_OPERATION, "InvalidOperation");
    }
    if bag.kind == HeaderKind::Content && name.eq_ignore_ascii_case("content-length") {
        return unsupported("Add of Content-Length to the headers of a content: not measured");
    }
    if value.contains(['\r', '\n']) {
        return header_error("Format");
    }
    if secret {
        push_value(&mut bag.secret, name, value.to_string());
        return Ok(());
    }
    check_typed(bag, name, value)?;
    push_value(&mut bag.items, name, value.to_string());
    Ok(())
}

fn with_bag<R>(r: Option<&Value>, f: impl FnOnce(&mut HeaderBag) -> R) -> Result<R, Signal> {
    let Some(Value::Library(_, cell)) = r else {
        return unsupported("HttpHeaders method without a receiver");
    };
    // A variable no GetHeaders or Headers call filled is an empty collection (network/lt-http/HttpRequest.al:
    // FreshRequestContent reads `Keys().Count()` 0 after a GetHeaders that raised).
    if matches!(&*cell.borrow(), LibraryValue::Empty) {
        *cell.borrow_mut() = LibraryValue::HttpHeaders(new_header_list(HeaderKind::Request));
    }
    match &*cell.borrow() {
        LibraryValue::HttpHeaders(h) => Ok(f(&mut h.borrow_mut())),
        other => unsupported(format!("HttpHeaders method on {}", other.describe())),
    }
}

fn headers_add(_: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> Out {
    let name = text(a, 0)?;
    let (value, secret) = text_or_secret(a, 1)?;
    with_bag(r.as_deref(), |b| add_header(b, &name, &value, secret))??;
    Ok(Value::Boolean(true))
}

/// `Remove(Name)` drops every value of the name and answers true, whether or not it was there.
fn headers_remove(_: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> Out {
    let name = text(a, 0)?;
    with_bag(r.as_deref(), |b| {
        if b.kind != HeaderKind::Content && is_content_header(&name) {
            return unsupported("Remove of a content header from request, response or client headers: not measured");
        }
        b.items.retain(|(n, _)| !n.eq_ignore_ascii_case(&name));
        b.secret.retain(|(n, _)| !n.eq_ignore_ascii_case(&name));
        Ok(())
    })??;
    Ok(Value::Boolean(true))
}

/// Asking a request, response or client header collection for a content header raises
/// `InvalidHeader:InvalidOperation` (network/lt-http/HttpHeaders.al:TryAddWithoutValidationBasics, HttpResponse.al).
fn restricted_lookup(b: &HeaderBag, name: &str) -> Result<(), Signal> {
    if b.kind != HeaderKind::Content && is_content_header(name) {
        return header_error("InvalidOperation");
    }
    if name.chars().any(|c| !is_token_char(c)) {
        return header_error("Format");
    }
    Ok(())
}

fn headers_contains(_: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> Out {
    let name = text(a, 0)?;
    with_bag(r.as_deref(), |b| {
        restricted_lookup(b, &name)?;
        Ok(Value::Boolean(find(&b.items, &name).is_some()))
    })?
}

fn headers_contains_secret(_: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> Out {
    let name = text(a, 0)?;
    with_bag(r.as_deref(), |b| Value::Boolean(find(&b.secret, &name).is_some()))
}

fn headers_clear(_: &mut dyn Ctx, r: Option<&mut Value>, _: &mut [Value]) -> Out {
    with_bag(r.as_deref(), |b| {
        b.items.clear();
        b.secret.clear();
    })?;
    Ok(Value::Null)
}

fn headers_keys(_: &mut dyn Ctx, r: Option<&mut Value>, _: &mut [Value]) -> Out {
    let keys = with_bag(r.as_deref(), |b| {
        b.items
            .iter()
            .map(|(n, _)| Value::Text(n.clone().into()))
            .collect::<Vec<_>>()
    })?;
    Ok(Value::List(Rc::new(RefCell::new(keys))))
}

/// `GetValues(Name, var Values)` fills an array or a list with the values in order and answers true; a name that is
/// not there answers false and leaves the target as it was.
fn headers_get_values(_: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> Out {
    let name = text(a, 0)?;
    let values = with_bag(r.as_deref(), |b| {
        restricted_lookup(b, &name)?;
        Ok(find(&b.items, &name).map(|(_, v)| v.clone()))
    })??;
    let Some(values) = values else {
        return Ok(Value::Boolean(false));
    };
    match a.get(1) {
        Some(Value::Array(arr)) => {
            let mut arr = arr.borrow_mut();
            if values.len() > arr.len() {
                return unsupported(
                    "HttpHeaders.GetValues into an array smaller than the number of values: not measured",
                );
            }
            for (i, v) in values.into_iter().enumerate() {
                arr[i] = Value::Text(v.into());
            }
        }
        Some(Value::List(list)) => {
            let mut list = list.borrow_mut();
            list.clear();
            list.extend(values.into_iter().map(|v| Value::Text(v.into())));
        }
        other => return unsupported(format!("HttpHeaders.GetValues into {}", other.describe())),
    }
    Ok(Value::Boolean(true))
}

/// `TryAddWithoutValidation` adds without checking the value or the known headers: false for a name that is no
/// token, or for a content header on request, response or client headers; true otherwise
/// (network/lt-http/HttpHeaders.al:TryAddWithoutValidationBasics, TryAddWithoutValidationRestricted).
fn headers_try_add(_: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> Out {
    let name = text(a, 0)?;
    let (value, secret) = text_or_secret(a, 1)?;
    with_bag(r.as_deref(), |b| {
        if name.is_empty() || name.chars().any(|c| !is_token_char(c)) {
            return Value::Boolean(false);
        }
        if b.kind != HeaderKind::Content && is_content_header(&name) {
            return Value::Boolean(false);
        }
        push_value(if secret { &mut b.secret } else { &mut b.items }, &name, value);
        Value::Boolean(true)
    })
}

// ------------------------------------------------------------------------------------------------ content

fn write_from(_: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> Out {
    let t = text(a, 0)?;
    with_content(r, |s| s.replace(t.into_bytes(), "text/plain; charset=utf-8")).map(|()| Value::Null)
}

/// `WriteFrom(InStream)` takes the bytes the stream has left.
fn write_from_stream(_: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> Out {
    let bytes = crate::stream::drain_in(&mut a[0])?;
    assume(crate::assumptions::Assumption::HttpContentTypeOfStream);
    with_content(r, |s| s.replace(bytes, "application/octet-stream")).map(|()| Value::Null)
}

/// `ReadAs(var Text)` gives the body back, UTF-8 decoded; a content nobody wrote to reads as an empty text and
/// answers true (misc/W4XmlJsonHttp.al:ReadAs_FreshContent); the content a request has when none was set answers
/// false and leaves the text (network/lt-http/HttpRequest.al:FreshRequestContent).
fn read_as(_: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> Out {
    let (data, uninit) = with_content(r, |s| (s.data.clone(), s.uninitialized))?;
    if uninit {
        return Ok(Value::Boolean(false));
    }
    match String::from_utf8(data) {
        Ok(t) => {
            a[0] = Value::Text(t.into());
            Ok(Value::Boolean(true))
        }
        Err(_) => unsupported("HttpContent.ReadAs(Text) of bytes that are not UTF-8: not measured"),
    }
}

/// `ReadAs(var InStream)` hands the body out once: a second read gets an empty stream
/// (network/lt-http/HttpContent.al:ReadAsInStreamTwice).
fn read_as_stream(_: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> Out {
    let (data, uninit) = with_content(r, |s| {
        let out = if s.stream_taken { Vec::new() } else { s.data.clone() };
        s.stream_taken = true;
        (out, s.uninitialized)
    })?;
    if uninit {
        return Ok(Value::Boolean(false));
    }
    crate::stream::set_stream(
        &a[0],
        LibraryValue::InStream(crate::stream::InState::snapshot(data, crate::stream::Encoding::Utf8)),
    )?;
    Ok(Value::Boolean(true))
}

/// `GetHeaders(var Headers)` hands out the content's own header list (network/TestHttpRequestMessage.al:39); the
/// uninitialized content of a request raises `NotInitialized` (network/lt-http/HttpRequest.al:FreshRequestContent).
fn get_headers(_: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> Out {
    let (list, uninit) = with_content(r, |s| (s.headers.clone(), s.uninitialized))?;
    if uninit {
        return raise(
            "Microsoft.Dynamics.Nav.Runtime.NavHttpContent variable not initialized.",
            "NotInitialized",
        );
    }
    hand_out_headers(a.first(), &list)?;
    Ok(Value::Boolean(true))
}

/// `Clear` empties the body and gives the content its own fresh headers, the Content-Type among them
/// (network/lt-http/HttpContent.al:ClearContent).
fn content_clear(_: &mut dyn Ctx, r: Option<&mut Value>, _: &mut [Value]) -> Out {
    with_content(r, |s| s.replace(Vec::new(), "text/plain; charset=utf-8")).map(|()| Value::Null)
}

// ------------------------------------------------------------------------------------------------ client

/// `HttpClient.Timeout` is 100 seconds until set; a value over 5 minutes raises
/// `HttpClientTimeoutTooLarge` naming the minutes of the value (network/lt-http/HttpClient.al:TimeoutValues).
fn client_timeout(_: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> Out {
    let Some(v) = a.first() else {
        return with_client(r, |s| Value::Duration(s.timeout_ms));
    };
    let ms = match v {
        Value::Duration(d) => *d,
        Value::Integer(i) => i64::from(*i),
        Value::BigInteger(i) => *i,
        other => return unsupported(format!("HttpClient.Timeout set from {}", other.describe())),
    };
    if ms > 300_000 {
        return raise(
            format!(
                "The timeout value of '{}' is larger than the maximum supported value of '5' minutes.",
                (ms / 60_000) % 60
            ),
            "HttpClientTimeoutTooLarge",
        );
    }
    with_client(r, |s| s.timeout_ms = ms).map(|()| Value::Null)
}

fn client_get_base(_: &mut dyn Ctx, r: Option<&mut Value>, _: &mut [Value]) -> Out {
    with_client(r, |s| Value::Text(s.base_address.clone().into()))
}

fn client_set_base(c: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> Out {
    let u = text(a, 0)?;
    match checked_uri(c, &u)? {
        Some(n) => with_client(r, |s| s.base_address = n).map(|()| Value::Boolean(true)),
        None => Ok(Value::Boolean(false)),
    }
}

fn client_default_headers(_: &mut dyn Ctx, r: Option<&mut Value>, _: &mut [Value]) -> Out {
    with_client(r, |s| headers_value(&s.default_headers))
}

/// `Clear` brings the client back to a fresh one: timeout, base address and default headers
/// (network/lt-http/HttpClient.al:ClientClear).
fn client_clear(_: &mut dyn Ctx, r: Option<&mut Value>, _: &mut [Value]) -> Out {
    with_client(r, |s| *s = HttpClientState::default()).map(|()| Value::Null)
}

/// `UseResponseCookies` takes a Boolean and changes nothing a test can see (network/lt-http/HttpClient.al:BooleanSettings).
fn client_use_cookies(_: &mut dyn Ctx, r: Option<&mut Value>, _: &mut [Value]) -> Out {
    with_client(r, |_| ()).map(|()| Value::Null)
}

/// Hands `(method, url)` to the session and writes the response it returns into the `var Response` argument.
fn send(c: &mut dyn Ctx, method: &str, url: String, response: Option<&Value>) -> Out {
    let state = c.http_send(HttpOutgoing {
        method: method.to_ascii_uppercase(),
        url,
        has_secret_uri: false,
    })?;
    let Some(Value::Library(_, cell)) = response else {
        return unsupported("HttpClient call without a response variable");
    };
    // A request that failed (a handler answered true: the real send, to an unresolvable host) answers false and leaves
    // the response a call that got no answer has: status 0 (network/PZHttpHandler.al:HandlerAnsweringTrue_SendsTheRealRequest
    // pins only that the status is not the one the handler set).
    let failed = state.request_failed;
    *cell.borrow_mut() = LibraryValue::HttpResponse(HttpResponseState {
        status: state.status,
        reason: state.reason,
        content: state.content,
        headers: state.headers,
        blocked: state.blocked,
    });
    Ok(Value::Boolean(!failed))
}

macro_rules! verb {
    ($name:ident, $method:expr, $with_content:expr) => {
        fn $name(c: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> Out {
            let base = with_client(r, |s| s.base_address.clone())?;
            let path = text(a, 0)?;
            let url = resolve_request_path(&base, &path)?;
            let response = if $with_content { a.get(2) } else { a.get(1) };
            send(c, $method, url, response)
        }
    };
}
verb!(client_get, "GET", false);
verb!(client_delete, "DELETE", false);
verb!(client_post, "POST", true);
verb!(client_put, "PUT", true);
verb!(client_patch, "PATCH", true);

fn client_send(c: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> Out {
    let base = with_client(r, |s| s.base_address.clone())?;
    let Some(Value::Library(_, req)) = a.first() else {
        return unsupported("HttpClient.Send without a request");
    };
    let (method, uri) = match &*req.borrow() {
        LibraryValue::HttpRequest(s) => (s.method.clone().unwrap_or_else(|| "GET".into()), s.uri.clone()),
        LibraryValue::Empty => ("GET".into(), None),
        other => return unsupported(format!("HttpClient.Send of {}", other.describe())),
    };
    let Some(uri) = uri else {
        return unsupported("HttpClient.Send of a request with no uri: not measured");
    };
    let url = resolve_request_path(&base, &uri)?;
    send(c, &method, url, a.get(1))
}

// ------------------------------------------------------------------------------------------------ response

fn response_status(_: &mut dyn Ctx, r: Option<&mut Value>, _: &mut [Value]) -> Out {
    with_response(r, |s| Value::Integer(s.status))
}

fn response_reason(_: &mut dyn Ctx, r: Option<&mut Value>, _: &mut [Value]) -> Out {
    with_response(r, |s| Value::Text(s.reason.clone().into()))
}

fn response_blocked(_: &mut dyn Ctx, r: Option<&mut Value>, _: &mut [Value]) -> Out {
    with_response(r, |s| Value::Boolean(s.blocked))
}

fn response_success(_: &mut dyn Ctx, r: Option<&mut Value>, _: &mut [Value]) -> Out {
    with_response(r, |s| Value::Boolean((200..300).contains(&s.status)))
}

fn response_content(_: &mut dyn Ctx, r: Option<&mut Value>, _: &mut [Value]) -> Out {
    with_response(r, |s| s.content.clone())
}

fn response_headers(_: &mut dyn Ctx, r: Option<&mut Value>, _: &mut [Value]) -> Out {
    with_response(r, |s| headers_value(&s.headers))
}

// ------------------------------------------------------------------------------------------------ test messages

fn test_request_type(_: &mut dyn Ctx, r: Option<&mut Value>, _: &mut [Value]) -> Out {
    match with_test_request(r, |s| s.request_type.clone())? {
        Value::Null => unsupported("TestHttpRequestMessage.RequestType of a request no client call made: not measured"),
        v => Ok(v),
    }
}

fn test_request_path(_: &mut dyn Ctx, r: Option<&mut Value>, _: &mut [Value]) -> Out {
    with_test_request(r, |s| Value::Text(s.path.clone().into()))
}

fn test_request_query(_: &mut dyn Ctx, r: Option<&mut Value>, _: &mut [Value]) -> Out {
    with_test_request(r, |s| s.query.clone())
}

fn test_request_secret(_: &mut dyn Ctx, r: Option<&mut Value>, _: &mut [Value]) -> Out {
    with_test_request(r, |s| Value::Boolean(s.has_secret_uri))
}

fn test_response_status(_: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> Out {
    match a.first() {
        None => with_test_response(r, |s| Value::Integer(s.status)),
        Some(Value::Integer(n)) => {
            let n = *n;
            with_test_response(r, |s| s.status = n).map(|()| Value::Null)
        }
        Some(other) => unsupported(format!("HttpStatusCode set from {}", other.describe())),
    }
}

fn test_response_reason(_: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> Out {
    if a.is_empty() {
        return with_test_response(r, |s| Value::Text(s.reason.clone().into()));
    }
    let t = text(a, 0)?;
    with_test_response(r, |s| s.reason = t).map(|()| Value::Null)
}

fn test_response_blocked(_: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> Out {
    match a.first() {
        None => with_test_response(r, |s| Value::Boolean(s.blocked)),
        Some(Value::Boolean(b)) => {
            let b = *b;
            with_test_response(r, |s| s.blocked = b).map(|()| Value::Null)
        }
        Some(other) => unsupported(format!("IsBlockedByEnvironment set from {}", other.describe())),
    }
}

fn test_response_success(_: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> Out {
    match a.first() {
        None => with_test_response(r, |s| Value::Boolean(s.success)),
        Some(Value::Boolean(b)) => {
            let b = *b;
            with_test_response(r, |s| s.success = b).map(|()| Value::Null)
        }
        Some(other) => unsupported(format!("IsSuccessfulRequest set from {}", other.describe())),
    }
}

fn test_response_content(_: &mut dyn Ctx, r: Option<&mut Value>, _: &mut [Value]) -> Out {
    with_test_response(r, |s| s.content.clone())
}

fn test_response_headers(_: &mut dyn Ctx, r: Option<&mut Value>, _: &mut [Value]) -> Out {
    with_test_response(r, |s| headers_value(&s.headers))
}

// ------------------------------------------------------------------------------------------------ misc

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

// ------------------------------------------------------------------------------------------------ cookies

/// The error of a Set-Cookie header the container refuses (P94Cookie.al:Response_AllAttributes).
fn cookie_parse_error() -> Signal {
    match raise::<()>(
        "An error occurred when parsing the Cookie header for Uri 'https://bccookie.microsoft.com/'.",
        "AL:Cookie",
    ) {
        Err(e) => e,
        Ok(()) => unreachable!("raise fails"),
    }
}

/// A `DateTime` value of an RFC 1123 date (`Wed, 21 Oct 2037 07:28:00 GMT`).
fn parse_cookie_date(text: &str) -> Option<i64> {
    let rest = text.split_once(',').map_or(text, |(_, r)| r).trim();
    let mut it = rest.split_whitespace();
    let day: u32 = it.next()?.parse().ok()?;
    let month = [
        "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
    ]
    .iter()
    .position(|m| it.clone().next().is_some_and(|x| x.eq_ignore_ascii_case(m)))?
        + 1;
    it.next();
    let year: i32 = it.next()?.parse().ok()?;
    let clock = it.next()?;
    let zone = it.next()?;
    if !zone.eq_ignore_ascii_case("GMT") {
        return None;
    }
    let mut c = clock.split(':');
    let (h, m, s): (i64, i64, i64) = (
        c.next()?.parse().ok()?,
        c.next()?.parse().ok()?,
        c.next()?.parse().ok()?,
    );
    let d = i64::from(crate::format::day_from_civil(year, month as u32, day));
    Some((d - 1) * 86_400_000 + ((h * 60 + m) * 60 + s) * 1000 + 1)
}

/// The cookies a response's Set-Cookie headers leave in a cookie container asked for `https://bccookie.microsoft.com/`, in the
/// order they were set. A Domain attribute is refused, a cookie whose Path is not `/` is not sent to that address and an expired
/// one is gone (platform/lt-xmlport-session-navapp-cookie/P94Cookie.al). SameSite and unknown attributes are ignored.
fn response_cookies(ctx: &dyn Ctx, headers: &HttpHeaderList) -> Result<Vec<CookieState>, Signal> {
    let values: Vec<String> = headers
        .borrow()
        .items
        .iter()
        .filter(|(k, _)| k.eq_ignore_ascii_case("Set-Cookie"))
        .flat_map(|(_, v)| v.clone())
        .collect();
    let mut out: Vec<CookieState> = Vec::new();
    for header in values {
        let mut parts = header.split(';');
        let first = parts.next().unwrap_or_default();
        let (name, value) = match first.split_once('=') {
            Some((n, v)) => (n.trim().to_string(), v.trim().to_string()),
            None => (first.trim().to_string(), String::new()),
        };
        let mut c = CookieState {
            name,
            value,
            path: "/".into(),
            ..CookieState::default()
        };
        let mut path_ok = true;
        let mut expired = false;
        for attr in parts {
            let (k, v) = attr
                .split_once('=')
                .map_or((attr.trim(), ""), |(k, v)| (k.trim(), v.trim()));
            match k.to_ascii_lowercase().as_str() {
                "domain" => return Err(cookie_parse_error()),
                "path" => path_ok = v == "/",
                "secure" => c.secure = true,
                "httponly" => c.http_only = true,
                "expires" => {
                    let Some(at) = parse_cookie_date(v) else {
                        return unsupported(format!("a cookie Expires of the form `{v}` (not pinned)"));
                    };
                    c.expires = at;
                }
                "max-age" => {
                    let Ok(secs) = v.parse::<i64>() else {
                        return unsupported(format!("a cookie Max-Age of `{v}` (not pinned)"));
                    };
                    let Some(now) = ctx.now_unix_millis() else {
                        return unsupported("Max-Age of a cookie without a clock");
                    };
                    c.expires = unix_to_datetime(now + secs * 1000);
                }
                _ => {}
            }
        }
        if let Some(now) = ctx.now_unix_millis() {
            if c.expires != 0 && c.expires <= unix_to_datetime(now) {
                expired = true;
            }
        }
        out.retain(|o| !(o.name == c.name && path_ok));
        if path_ok && !expired {
            out.push(c);
        }
    }
    Ok(out)
}

fn unix_to_datetime(ms: i64) -> i64 {
    (i64::from(crate::format::day_from_civil(1970, 1, 1)) - 1) * 86_400_000 + ms + 1
}

fn response_cookie_names(c: &mut dyn Ctx, r: Option<&mut Value>, _: &mut [Value]) -> Out {
    let headers = with_response(r, |s| s.headers.clone())?;
    let cookies = response_cookies(c, &headers)?;
    let names: Vec<Value> = cookies.into_iter().map(|k| Value::Text(k.name.into())).collect();
    Ok(Value::List(Rc::new(RefCell::new(names))))
}

fn response_get_cookie(c: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> Out {
    let name = text(a, 0)?;
    let headers = with_response(r, |s| s.headers.clone())?;
    let cookies = response_cookies(c, &headers)?;
    match cookies.into_iter().find(|k| k.name == name) {
        Some(k) if !name.is_empty() => {
            a[1] = lib("Cookie", LibraryValue::Cookie(k));
            Ok(Value::Boolean(true))
        }
        _ => Ok(Value::Boolean(false)),
    }
}

fn with_cookie<R>(r: Option<&mut Value>, f: impl FnOnce(&mut CookieState) -> R) -> Result<R, Signal> {
    let cell = receiver_cell(r, "Cookie")?;
    let mut lib = cell.borrow_mut();
    if matches!(&*lib, LibraryValue::Empty) {
        *lib = LibraryValue::Cookie(CookieState::default());
    }
    match &mut *lib {
        LibraryValue::Cookie(s) => Ok(f(s)),
        other => unsupported(format!("Cookie method on {}", other.describe())),
    }
}

fn cookie_name(c: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> Out {
    let new = if a.is_empty() { None } else { Some(text(a, 0)?) };
    // What a setter answers is not pinned (assumption `cookie-setter-result`).
    if new.is_some() && c.ret_used() {
        crate::assumptions::assume(crate::assumptions::Assumption::CookieSetterResult);
    }
    with_cookie(r, |s| {
        if let Some(n) = new {
            s.name = n;
        }
        Value::Text(s.name.clone().into())
    })
}

fn cookie_value(c: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> Out {
    let new = if a.is_empty() { None } else { Some(text(a, 0)?) };
    // What a setter answers is not pinned (assumption `cookie-setter-result`).
    if new.is_some() && c.ret_used() {
        crate::assumptions::assume(crate::assumptions::Assumption::CookieSetterResult);
    }
    with_cookie(r, |s| {
        if let Some(n) = new {
            s.value = n;
        }
        Value::Text(s.value.clone().into())
    })
}

fn cookie_domain(_: &mut dyn Ctx, r: Option<&mut Value>, _: &mut [Value]) -> Out {
    with_cookie(r, |s| Value::Text(s.domain.clone().into()))
}

fn cookie_path(_: &mut dyn Ctx, r: Option<&mut Value>, _: &mut [Value]) -> Out {
    with_cookie(r, |s| Value::Text(s.path.clone().into()))
}

fn cookie_secure(_: &mut dyn Ctx, r: Option<&mut Value>, _: &mut [Value]) -> Out {
    with_cookie(r, |s| Value::Boolean(s.secure))
}

fn cookie_http_only(_: &mut dyn Ctx, r: Option<&mut Value>, _: &mut [Value]) -> Out {
    with_cookie(r, |s| Value::Boolean(s.http_only))
}

fn cookie_expires(_: &mut dyn Ctx, r: Option<&mut Value>, _: &mut [Value]) -> Out {
    with_cookie(r, |s| Value::DateTime(s.expires))
}

pub fn native_for(sig: &Signature) -> Option<NativeFn> {
    let ty = |i: usize| sig.params.get(i).map(|p| p.ty.as_str()).unwrap_or("");
    let n = sig.params.len();
    match (sig.receiver.as_str(), sig.name.as_str()) {
        ("HttpContent", "WriteFrom") if n == 1 && ty(0) == "Text" => Some(write_from),
        ("HttpContent", "WriteFrom") if n == 1 && ty(0) == "InStream" => Some(write_from_stream),
        ("HttpContent", "ReadAs") if n == 1 && ty(0) == "Text" => Some(read_as),
        ("HttpContent", "ReadAs") if n == 1 && ty(0) == "InStream" => Some(read_as_stream),
        ("HttpContent", "GetHeaders") => Some(get_headers),
        ("HttpContent", "Clear") => Some(content_clear),
        ("HttpHeaders", "Add") if n == 2 => Some(headers_add),
        ("HttpHeaders", "Remove") => Some(headers_remove),
        ("HttpHeaders", "Contains") => Some(headers_contains),
        ("HttpHeaders", "ContainsSecret") => Some(headers_contains_secret),
        ("HttpHeaders", "Clear") => Some(headers_clear),
        ("HttpHeaders", "Keys") => Some(headers_keys),
        ("HttpHeaders", "GetValues") if ty(1).starts_with("Array") || ty(1).starts_with("List") => {
            Some(headers_get_values)
        }
        ("HttpHeaders", "TryAddWithoutValidation") => Some(headers_try_add),
        ("HttpRequestMessage", "Content") => Some(request_content),
        ("HttpRequestMessage", "GetHeaders") => Some(request_get_headers),
        ("HttpRequestMessage", "Method") => Some(method),
        ("HttpRequestMessage", "SetRequestUri") if n == 1 && ty(0) == "Text" => Some(set_request_uri),
        ("HttpRequestMessage", "GetRequestUri") => Some(get_request_uri),
        ("HttpClient", "Get") => Some(client_get),
        ("HttpClient", "Delete") => Some(client_delete),
        ("HttpClient", "Post") => Some(client_post),
        ("HttpClient", "Put") => Some(client_put),
        ("HttpClient", "Patch") => Some(client_patch),
        ("HttpClient", "Send") => Some(client_send),
        ("HttpClient", "Timeout") => Some(client_timeout),
        ("HttpClient", "GetBaseAddress") => Some(client_get_base),
        ("HttpClient", "SetBaseAddress") => Some(client_set_base),
        ("HttpClient", "DefaultRequestHeaders") => Some(client_default_headers),
        ("HttpClient", "Clear") => Some(client_clear),
        ("HttpClient", "UseResponseCookies") => Some(client_use_cookies),
        ("HttpResponseMessage", "HttpStatusCode") => Some(response_status),
        ("HttpResponseMessage", "ReasonPhrase") => Some(response_reason),
        ("HttpResponseMessage", "IsBlockedByEnvironment") => Some(response_blocked),
        ("HttpResponseMessage", "IsSuccessStatusCode") => Some(response_success),
        ("HttpResponseMessage", "Content") => Some(response_content),
        ("HttpResponseMessage", "Headers") => Some(response_headers),
        ("TestHttpRequestMessage", "RequestType") => Some(test_request_type),
        ("TestHttpRequestMessage", "Path") => Some(test_request_path),
        ("TestHttpRequestMessage", "QueryParameters") => Some(test_request_query),
        ("TestHttpRequestMessage", "HasSecretUri") => Some(test_request_secret),
        ("TestHttpResponseMessage", "HttpStatusCode") => Some(test_response_status),
        ("TestHttpResponseMessage", "ReasonPhrase") => Some(test_response_reason),
        ("TestHttpResponseMessage", "IsBlockedByEnvironment") => Some(test_response_blocked),
        ("TestHttpResponseMessage", "IsSuccessfulRequest") => Some(test_response_success),
        ("TestHttpResponseMessage", "Content") => Some(test_response_content),
        ("TestHttpResponseMessage", "Headers") => Some(test_response_headers),
        ("HttpResponseMessage", "GetCookieNames") => Some(response_cookie_names),
        ("HttpResponseMessage", "GetCookie") => Some(response_get_cookie),
        ("Cookie", "Name") => Some(cookie_name),
        ("Cookie", "Value") => Some(cookie_value),
        ("Cookie", "Domain") => Some(cookie_domain),
        ("Cookie", "Path") => Some(cookie_path),
        ("Cookie", "Secure") => Some(cookie_secure),
        ("Cookie", "HttpOnly") => Some(cookie_http_only),
        ("Cookie", "Expires") => Some(cookie_expires),
        ("FileUpload", "FileName") => Some(file_upload_name),
        ("FileUpload", "CreateInStream") => Some(file_upload_create_in),
        _ => None,
    }
}

/// The request the session gives a handler: `Path` without the query, the decoded query parameters in order.
pub fn split_url(url: &str) -> (String, Vec<(String, String)>) {
    let no_fragment = url.split('#').next().unwrap_or(url);
    let (path, query) = match no_fragment.split_once('?') {
        Some((p, q)) => (p, q),
        None => (no_fragment, ""),
    };
    let params = query
        .split('&')
        .filter(|p| !p.is_empty())
        .map(|p| match p.split_once('=') {
            Some((k, v)) => (percent_decode(k), percent_decode(v)),
            None => (percent_decode(p), String::new()),
        })
        .collect();
    (path.to_string(), params)
}

impl HttpContentState {
    /// A content with its own data and its own headers.
    pub fn deep_copy(&self) -> HttpContentState {
        let bag = self.headers.borrow();
        let copy = new_header_list(bag.kind);
        {
            let mut c = copy.borrow_mut();
            c.items = bag.items.clone();
            c.secret = bag.secret.clone();
        }
        HttpContentState {
            data: self.data.clone(),
            headers: copy,
            uninitialized: self.uninitialized,
            stream_taken: self.stream_taken,
        }
    }

    /// `WriteFrom` and `Clear`: the content gets the new bytes and a header collection of its own that holds the
    /// Content-Type (network/lt-http/HttpContent.al:ContentTypeHeaderAfterWrite, ClearContent).
    fn replace(&mut self, data: Vec<u8>, content_type: &str) {
        assume(crate::assumptions::Assumption::HttpContentWriteResetsHeaders);
        let headers = new_header_list(HeaderKind::Content);
        headers
            .borrow_mut()
            .items
            .push(("Content-Type".into(), vec![content_type.into()]));
        self.data = data;
        self.headers = headers;
        self.uninitialized = false;
        self.stream_taken = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uris_take_their_normal_form() {
        // network/lt-http/HttpRequest.al:SetRequestUriValid, SetRequestUriRelativeAndInvalid, SetRequestUriOtherSchemes.
        let ok = |i: &str, o: &str| assert_eq!(normalize_uri(i).as_deref(), Ok(o), "{i}");
        ok("https://example.invalid/path?q=1", "https://example.invalid/path?q=1");
        ok("http://example.invalid", "http://example.invalid/");
        ok("HTTPS://EXAMPLE.invalid/A/B", "https://example.invalid/A/B");
        ok("https://example.invalid:8443/x", "https://example.invalid:8443/x");
        ok("https://example.invalid:443/x", "https://example.invalid/x");
        ok("https://example.invalid/a b", "https://example.invalid/a%20b");
        ok("https://example.invalid/x#frag", "https://example.invalid/x#frag");
        ok("https://user@example.invalid/x", "https://user@example.invalid/x");
        ok("/relative/path", "file:///relative/path");
        ok("https://example.invalid/ä", "https://example.invalid/%C3%A4");
        ok("javascript:alert(1)", "javascript:alert(1)");
        ok("ftp://example.invalid/x", "ftp://example.invalid/x");
        for bad in [
            "",
            "http://",
            "ht!tp://x",
            "https://exa mple.invalid/",
            "https://example.invalid:99999/",
        ] {
            assert_eq!(normalize_uri(bad), Err(false), "{bad}");
        }
    }

    #[test]
    fn a_url_splits_into_path_and_decoded_query() {
        let (p, q) = split_url("https://example.invalid/head?q=%20a&x=1#f");
        assert_eq!(p, "https://example.invalid/head");
        assert_eq!(
            q,
            vec![("q".to_string(), " a".to_string()), ("x".to_string(), "1".to_string())]
        );
    }

    #[test]
    fn header_names_and_values_are_checked_as_the_probes_measured() {
        let mut b = HeaderBag::default();
        let code = |r: Result<(), Signal>| match r {
            Err(Signal::Error(e)) => e.code,
            Ok(()) => "ok".into(),
            Err(other) => format!("{other:?}"),
        };
        assert_eq!(code(add_header(&mut b, "X-Test", "one", false)), "ok");
        assert_eq!(code(add_header(&mut b, "", "v", false)), "InvalidHeader:Argument");
        assert_eq!(code(add_header(&mut b, "Bad Name", "v", false)), "InvalidHeader:Format");
        assert_eq!(code(add_header(&mut b, "X-Bad", "a\nb", false)), "InvalidHeader:Format");
        assert_eq!(
            code(add_header(&mut b, "Content-Type", "text/plain", false)),
            "InvalidOperation"
        );
        assert_eq!(code(add_header(&mut b, "Accept", "bad", false)), "InvalidHeader:Format");
        assert_eq!(code(add_header(&mut b, "Accept", "application/json", false)), "ok");
        assert_eq!(
            code(add_header(&mut b, "Authorization", "", false)),
            "InvalidHeader:Format"
        );
        assert_eq!(code(add_header(&mut b, "Authorization", "Bearer abc", false)), "ok");
        assert_eq!(
            code(add_header(&mut b, "Date", "Tue, 15 Nov 1994 08:12:31 GMT", false)),
            "ok"
        );
        assert_eq!(
            code(add_header(&mut b, "Date", "Tue, 15 Nov 1994 08:12:32 GMT", false)),
            "InvalidHeader:Format"
        );
        assert_eq!(b.items.iter().filter(|(n, _)| n == "X-Test").count(), 1);
    }
}
