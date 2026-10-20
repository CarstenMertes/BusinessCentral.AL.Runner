//! `GetUrl` with a company, an object, a record and its filters, for `ClientType::Web`. Only what the corpus pins is built;
//! every other form is refused (ADR-042). The host part is the server address of the host (assumption
//! `geturl-web-base-address`); the query after it is pinned by:
//! - platform/lt-system-database-members/GetUrlCompany.al:67-77 and session/lt-geturl-shape/GetUrlShape.al:121-130 (company),
//! - GetUrlObjects.al:62-91 and GetUrlShape.al:152-163 (object type and id),
//! - GetUrlBookmarkFilter.al (bookmark of the key, filter text, UseFilters), GetUrlRecord.al, GetUrlRecordLayout.al.

use crate::Session;
use crate::records::Rec;
use al_builtins::describe::Describe;
use al_builtins::native::Signal;
use al_builtins::value::{LibraryValue, RtType, Value};

fn unsupported<T>(what: impl Into<String>) -> Result<T, Signal> {
    Err(Signal::Unsupported(what.into()))
}

/// The option member name of a system option value.
fn member(v: Option<&Value>) -> Option<String> {
    match v {
        Some(Value::Option(o, members)) => members.get(*o as usize).cloned(),
        _ => None,
    }
}

/// Percent-encodes `s` for a query value. Letters, digits, `.` and `-` stay; a space, `& ' / % # ? * | < > =` and
/// non-ASCII characters (UTF-8) are encoded with capital hex digits (GetUrlCompany.al:67-77, GetUrlBookmarkFilter.al:
/// filter_*). Any other character is not pinned and is refused.
fn encode(s: &str) -> Result<String, Signal> {
    let mut out = String::new();
    for c in s.chars() {
        if c.is_ascii_alphanumeric() || c == '.' || c == '-' {
            out.push(c);
        } else if c.is_ascii() && !" &'/%#?*|<>=".contains(c) {
            return unsupported(format!(
                "GetUrl with the character `{c}` in a company or filter text: only letters, digits, . - and the characters of the pinned probes are built"
            ));
        } else {
            let mut b = [0u8; 4];
            for byte in c.encode_utf8(&mut b).bytes() {
                out.push_str(&format!("%{byte:02X}"));
            }
        }
    }
    Ok(out)
}

fn base64(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let n = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        for i in 0..=chunk.len() {
            out.push(T[((n >> (18 - 6 * i)) & 63) as usize] as char);
        }
        for _ in chunk.len()..3 {
            out.push('=');
        }
    }
    out
}

impl Session {
    /// `GetUrl(ClientType, Company, ObjectType, ObjectId, Record|RecordRef, UseFilters, Layout)`: `args` is the call's arguments.
    pub(crate) fn get_url(&mut self, args: &[Value]) -> Result<Value, Signal> {
        // GetUrlShape.al:270 (`default alone`): ClientType::Default without other arguments is the web address with the
        // scheme `dynamicsnav` and its path followed by `//`, pinned for an http address only.
        if member(args.first()).as_deref() == Some("Default") && args.len() == 1 {
            al_builtins::assumptions::assume(al_builtins::assumptions::Assumption::GetUrlWebBaseAddress);
            let base = self.host.identity.server_base_url();
            let Some(rest) = base.strip_prefix("http://") else {
                return unsupported(format!(
                    "GetUrl(ClientType::Default) with the server address {base}: only an http address is pinned"
                ));
            };
            return Ok(Value::Text(
                format!("dynamicsnav://{}//", rest.trim_end_matches('/')).into(),
            ));
        }
        if member(args.first()).as_deref() != Some("Web") {
            return unsupported(format!(
                "GetUrl({}; the call had {} arguments): only ClientType::Web is built (the host part of the other client types is not pinned)",
                args.first().describe(),
                args.len()
            ));
        }
        al_builtins::assumptions::assume(al_builtins::assumptions::Assumption::GetUrlWebBaseAddress);
        let base = self.host.identity.server_base_url();
        let company = match args.get(1) {
            None => String::new(),
            Some(Value::Text(t)) | Some(Value::Code(t)) if !t.has_marker() => t.as_str().to_string(),
            other => return unsupported(format!("GetUrl with company {}", other.describe())),
        };
        let mut query: Vec<String> = Vec::new();
        if !company.is_empty() {
            query.push(format!("company={}", encode(&company)?));
        }
        if args.len() > 2 {
            let kind = member(args.get(2));
            let id = match args.get(3) {
                Some(Value::Integer(n)) => *n,
                other => return unsupported(format!("GetUrl with object id {}", other.describe())),
            };
            let key = match kind.as_deref() {
                Some("Page") => "page",
                Some("Report") => "report",
                Some("Table") => "table",
                Some("Query") => "query",
                // codeunits and XMLports have no web client address (GetUrlObjects.al:64,67)
                Some("Codeunit") | Some("XMLport") | Some("XmlPort") if args.len() == 4 => {
                    return Ok(Value::Text("".into()));
                }
                other => {
                    return unsupported(format!(
                        "GetUrl of object type {} with a record (only Page and Table are pinned; Report and Query pin only \"no query part\")",
                        other.unwrap_or("?")
                    ));
                }
            };
            // An id of zero or less has no address (GetUrlObjects.al:84-85,89-90).
            if id <= 0 && args.len() == 4 {
                return Ok(Value::Text("".into()));
            }
            if args.len() > 4 {
                // A record of another object kind than a page: no address (GetUrlShape.al:152-163 `table_record`;
                // the report and query forms are pinned only to have no '?', GetUrlBookmarkFilter.al:`report`, `query`).
                let rec = match args.get(4) {
                    Some(Value::Record(r)) => r.clone(),
                    Some(Value::Library(_, l)) => match &*l.borrow() {
                        LibraryValue::RecordRef(r) => r.clone(),
                        // PGetUrl.al:P109_GetUrl_Overload3_ClosedRecordRef
                        _ => {
                            return Err(Signal::Error(Box::new(al_builtins::native::AlError {
                                message: "The record is not open.".into(),
                                code: "RecordNotOpened".into(),
                                call_stack: String::new(),
                                ..Default::default()
                            })));
                        }
                    },
                    other => return unsupported(format!("GetUrl with record {}", other.describe())),
                };
                if key == "report" || key == "query" {
                    // GetUrlBookmarkFilter.al:`report`, `query` pin only that the result has no query part
                    // ('?'); the value itself (empty text or the bare address) is not pinned.
                    return unsupported(format!(
                        "GetUrl of a {key} with a record: only \"no query part\" is pinned, not the value"
                    ));
                }
                if key != "page" {
                    return Ok(Value::Text("".into()));
                }
                if id <= 0 {
                    return unsupported("GetUrl of page 0 or a negative page with a record (not pinned)");
                }
                let use_filters = match args.get(5) {
                    None => false,
                    Some(Value::Boolean(b)) => *b,
                    other => return unsupported(format!("GetUrl with UseFilters {}", other.describe())),
                };
                // The layout name changes nothing, for a name that does not exist and for the empty text
                // (GetUrlRecordLayout.al:GetUrl_Record_Layout).
                match args.get(6) {
                    None | Some(Value::Text(_)) => {}
                    other => return unsupported(format!("GetUrl with layout {}", other.describe())),
                }
                query.push(format!("page={id}"));
                if let Some(part) = self.url_record_part(&rec, use_filters)? {
                    query.push(part);
                }
            } else {
                if id <= 0 {
                    return Ok(Value::Text("".into()));
                }
                query.push(format!("{key}={id}"));
            }
        }
        if query.is_empty() {
            return Ok(Value::Text(base.into()));
        }
        Ok(Value::Text(
            format!("{}/?{}", base.trim_end_matches('/'), query.join("&")).into(),
        ))
    }

    /// The `bookmark=` or `filter=` part for a record: the filters when `use_filters` and there are some, nothing when
    /// `use_filters` and there are none, else the bookmark of the key (GetUrlBookmarkFilter.al:GetUrl_Filter_Text,
    /// GetUrl_Bookmark_TwoFieldKey `usefilters_true_not_inserted`).
    fn url_record_part(&mut self, rec: &Rec, use_filters: bool) -> Result<Option<String>, Signal> {
        let t = self.table(rec)?;
        {
            let r = rec.borrow();
            if r.temporary {
                return unsupported("GetUrl with a temporary record (not pinned)");
            }
            if r.marked_only || !r.marks.is_empty() && use_filters {
                return unsupported("GetUrl with marked records (not pinned)");
            }
        }
        if use_filters {
            let (groups_ok, mut fields) = {
                let r = rec.borrow();
                (
                    r.filter_group == 0 && r.filters.iter().all(|f| f.group == 0),
                    r.filters.iter().map(|f| f.field).collect::<Vec<_>>(),
                )
            };
            if !groups_ok {
                return unsupported("GetUrl with filters in a filter group other than 0 (not pinned)");
            }
            if fields.is_empty() {
                return Ok(None);
            }
            fields.sort_unstable();
            fields.dedup();
            let mut parts = Vec::new();
            for pos in fields {
                let f = &t.fields[pos];
                if !f.stored || !matches!(f.ty, RtType::Code(_) | RtType::Text(_) | RtType::Integer) {
                    return unsupported(format!(
                        "GetUrl filter on field {} (only Code, Text and Integer fields of the table are pinned)",
                        f.name
                    ));
                }
                if self.field_caption(&t, pos) != f.name {
                    return unsupported("GetUrl filter on a field whose caption differs from its name (not pinned)");
                }
                let text = self.field_filter_text(rec, pos)?;
                parts.push(format!("'{}' IS '{}'", f.name, text.replace('\'', "''")));
            }
            return Ok(Some(format!("filter={}", encode(&parts.join(" AND "))?)));
        }
        // The bookmark: the table number, then the primary key fields. Only a key of one Code[20] field, one Integer
        // field, or a Code[20] and an Integer field (in that order) is pinned.
        let table_id = match self.engine.program.objects[t.object as usize].id {
            Some(id) => id,
            None => return unsupported("GetUrl bookmark of a table without an id"),
        };
        let r = rec.borrow();
        let mut data: Vec<u8> = table_id.to_le_bytes().to_vec();
        let mut size = 6usize;
        let key = &t.keys[0];
        let shape: Vec<&RtType> = key.iter().map(|&p| &t.fields[p].ty).collect();
        let ok = matches!(
            shape.as_slice(),
            [RtType::Code(Some(20))] | [RtType::Integer] | [RtType::Code(Some(20)), RtType::Integer]
        );
        if !ok {
            return unsupported(
                "GetUrl bookmark of a primary key other than Code[20], Integer, or Code[20] and Integer (not pinned)",
            );
        }
        for &p in key.iter() {
            match &r.fields[p] {
                Value::Code(c) => {
                    let s = c.as_str();
                    if !s.is_ascii() {
                        return unsupported("GetUrl bookmark of a Code value with a non-ASCII character (not pinned)");
                    }
                    if s.is_empty() && key.len() > 1 {
                        return unsupported("GetUrl bookmark of an empty Code in a two-field key (not pinned)");
                    }
                    data.extend_from_slice(&[0x02, 0x7b]);
                    if !s.is_empty() {
                        data.push(0xff);
                    }
                    for b in s.bytes() {
                        data.extend_from_slice(&[b, 0]);
                    }
                    data.extend_from_slice(&[0, 0]);
                    size += 5 + 2 * s.len();
                }
                Value::Integer(n) => {
                    data.extend_from_slice(&[0x00, 0x87]);
                    data.extend_from_slice(&n.to_le_bytes());
                    size += 6;
                }
                other => return unsupported(format!("GetUrl bookmark of a key value {}", other.describe())),
            }
        }
        while data.last() == Some(&0) {
            data.pop();
        }
        let b64 = base64(&data)
            .replace('+', "%2b")
            .replace('/', "%2f")
            .replace('=', "%3d");
        Ok(Some(format!("bookmark={size}%3b{b64}")))
    }
}
