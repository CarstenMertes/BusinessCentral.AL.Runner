//! Serve mode (port of `Program.Serve`): the file is opened once, then requests arrive one
//! JSON object per line and each is answered with one JSON line, so the open cost is paid
//! once. Requests: `{"id": any, "cmd": "read"|"tables"|"companies"|"describe"|"quit",
//! "table": .., "company": .., "app": .., "top": .., "select": .., "sha256": ..,
//! "merge-extensions": ..}`. The id is echoed back. A failed request answers
//! `{"ok": false, "error": ..}` and the session stays up.
//!
//! A key the command does not accept fails the request instead of being dropped. Serve
//! exists for programmatic callers that build requests in code, where nobody reads the
//! output: a mistyped "tpo" silently losing a row limit, or a mistyped "compayn" silently
//! reading every company, is a wrong answer reported as success.

use crate::error::{Error, Result};
use crate::query::{self, bc_tables, plan_read, ReadRequest, TableSelector};
use crate::source::BcSource;
use crate::symbols::SymbolStore;
use crate::value::json_string;
use serde_json::Value as J;
use std::fmt::Write as _;
use std::io::{BufRead, Write};

const KEYS: &[(&str, &[&str])] = &[
    ("read", &["table", "company", "app", "top", "select", "sha256", "merge-extensions"]),
    ("describe", &["table", "company", "app"]),
    ("tables", &[]),
    ("companies", &[]),
    ("quit", &[]),
];

pub fn serve(
    src: &dyn BcSource,
    sym: Option<&SymbolStore>,
    input: &mut dyn BufRead,
    output: &mut dyn Write,
) -> Result<()> {
    let mut line = String::new();
    loop {
        line.clear();
        if input.read_line(&mut line)? == 0 {
            return Ok(());
        }
        if line.trim().is_empty() {
            continue;
        }
        let mut id_json = "null".to_string();
        match handle(src, sym, &line, &mut id_json) {
            Ok(Reply::Line(s)) => writeln!(output, "{s}")?,
            Ok(Reply::Quit(s)) => {
                writeln!(output, "{s}")?;
                output.flush()?;
                return Ok(());
            }
            Err(e) => {
                let mut msg = String::new();
                json_string(e.message(), &mut msg);
                writeln!(output, "{{\"id\": {id_json}, \"ok\": false, \"error\": {msg}}}")?;
            }
        }
        output.flush()?;
    }
}

enum Reply {
    Line(String),
    Quit(String),
}

fn handle(src: &dyn BcSource, sym: Option<&SymbolStore>, line: &str, id_json: &mut String) -> Result<Reply> {
    let root: J = serde_json::from_str(line).map_err(|e| Error::Argument(e.to_string()))?;
    let obj = root.as_object().ok_or_else(|| Error::Argument("a request must be a JSON object".into()))?;
    *id_json = obj.get("id").map(|v| v.to_string()).unwrap_or_else(|| "null".into());
    let cmd = obj.get("cmd").and_then(|v| v.as_str()).unwrap_or("");
    let Some((_, accepted)) = KEYS.iter().find(|(k, _)| *k == cmd) else {
        return Err(Error::Argument(format!(
            "unknown cmd '{cmd}' — expected {}",
            KEYS.iter().map(|(k, _)| *k).collect::<Vec<_>>().join(", ")
        )));
    };
    for k in obj.keys() {
        if k != "id" && k != "cmd" && !accepted.contains(&k.as_str()) {
            let mut all = vec!["id", "cmd"];
            all.extend(accepted.iter());
            return Err(Error::Argument(format!(
                "unknown request key '{k}' for cmd '{cmd}' — accepted: {}",
                all.join(", ")
            )));
        }
    }
    if cmd == "quit" {
        return Ok(Reply::Quit(format!("{{\"id\": {id_json}, \"ok\": true}}")));
    }
    let opt = |name: &str| -> Option<String> {
        match obj.get(name) {
            None | Some(J::Null) => None,
            Some(J::String(s)) => Some(s.clone()),
            Some(v) => Some(v.to_string()),
        }
    };
    let sel = TableSelector { table: opt("table"), company: opt("company"), app: opt("app") };
    let s = match cmd {
        "read" => {
            let req = ReadRequest {
                sel,
                top: opt("top"),
                select: opt("select"),
                sha256: opt("sha256"),
                // present and not false: a request that says "merge-extensions": false asks for no merge
                merge_extensions: opt("merge-extensions").is_some_and(|v| v != "false"),
            };
            serve_read(src, sym, &req, id_json)?
        }
        "tables" => serve_tables(src, sym, id_json)?,
        "companies" => {
            let mut s = format!("{{\"id\": {id_json}, \"ok\": true, \"companies\": [");
            for (i, c) in query::companies(src)?.iter().enumerate() {
                if i > 0 {
                    s.push_str(", ");
                }
                json_string(c, &mut s);
            }
            s.push_str("]}");
            s
        }
        "describe" => serve_describe(src, sym, &sel, id_json)?,
        _ => unreachable!("cmd was checked against KEYS"),
    };
    Ok(Reply::Line(s))
}

fn serve_read(src: &dyn BcSource, sym: Option<&SymbolStore>, req: &ReadRequest, id_json: &str) -> Result<String> {
    let plan = plan_read(src, sym, req)?;
    let mut s = format!("{{\"id\": {id_json}, \"ok\": true, \"headers\": [");
    for (i, h) in plan.headers.iter().enumerate() {
        if i > 0 {
            s.push_str(", ");
        }
        json_string(h, &mut s);
    }
    s.push_str("], \"rows\": [");
    let mut first = true;
    plan.run(src, &mut |row| {
        if !first {
            s.push_str(", ");
        }
        first = false;
        s.push('[');
        for (i, v) in row.iter().enumerate() {
            if i > 0 {
                s.push_str(", ");
            }
            v.write_json(&mut s);
        }
        s.push(']');
        Ok(true)
    })?;
    s.push(']');
    if let Some(n) = &plan.note {
        s.push_str(", \"note\": ");
        json_string(n, &mut s);
    }
    s.push('}');
    Ok(s)
}

fn serve_tables(src: &dyn BcSource, sym: Option<&SymbolStore>, id_json: &str) -> Result<String> {
    let mut tables = bc_tables(src)?;
    tables.sort_by(|a, b| (&a.company, &a.table_name).cmp(&(&b.company, &b.table_name)));
    let mut s = format!("{{\"id\": {id_json}, \"ok\": true, \"tables\": [");
    for (i, t) in tables.iter().enumerate() {
        if i > 0 {
            s.push_str(", ");
        }
        let al = sym.and_then(|sy| sy.find_for_sql_table(query::strip_ext(&t.table_name), t.app_id.as_deref()));
        s.push_str("{\"company\": ");
        match &t.company {
            Some(c) => json_string(c, &mut s),
            None => s.push_str("null"),
        }
        s.push_str(", \"name\": ");
        json_string(&t.table_name, &mut s);
        let _ = write!(s, ", \"rows\": {}, \"compression\": ", src.row_count(&t.table)?);
        json_string(&t.table.compression, &mut s);
        if let Some(al) = al {
            let _ = write!(s, ", \"al\": {{\"id\": {}, \"name\": ", al.id);
            json_string(&al.name, &mut s);
            s.push_str(", \"app\": ");
            json_string(&al.app_name, &mut s);
            s.push('}');
        }
        s.push('}');
    }
    s.push_str("]}");
    Ok(s)
}

fn serve_describe(src: &dyn BcSource, sym: Option<&SymbolStore>, sel: &TableSelector, id_json: &str) -> Result<String> {
    let sym = sym.ok_or_else(|| {
        Error::Argument("describe requires serve to be started with --symbols (a .app package or SymbolReference.json)".into())
    })?;
    let d = query::describe(src, sym, sel)?;
    let mut s = format!("{{\"id\": {id_json}, \"ok\": true, \"table\": {{\"id\": {}, \"name\": ", d.al.id);
    json_string(&d.al.name, &mut s);
    s.push_str(", \"app\": ");
    json_string(&d.al.app_name, &mut s);
    s.push_str(", \"appId\": ");
    json_string(&d.al.app_id, &mut s);
    s.push_str(", \"sqlObject\": ");
    json_string(&d.sql_object, &mut s);
    s.push_str("}, \"fields\": [");
    let mut first = true;
    let mut sep = |s: &mut String| {
        if !first {
            s.push_str(", ");
        }
        first = false;
    };
    for f in &d.fields {
        sep(&mut s);
        let _ = write!(s, "{{\"id\": {}, \"name\": ", f.field.id);
        json_string(&f.field.name, &mut s);
        s.push_str(", \"type\": ");
        json_string(&f.field.type_name, &mut s);
        if f.field.field_class != "Normal" {
            s.push_str(", \"fieldClass\": ");
            json_string(&f.field.field_class, &mut s);
            s.push_str(", \"sqlColumn\": null");
        } else {
            s.push_str(", \"sqlColumn\": ");
            match &f.sql_column {
                Some(c) => json_string(c, &mut s),
                None => s.push_str("null"),
            }
            s.push_str(", \"sqlType\": ");
            match &f.sql_type {
                Some(c) => json_string(c, &mut s),
                None => s.push_str("null"),
            }
        }
        s.push('}');
    }
    for e in &d.ext_fields {
        sep(&mut s);
        s.push_str("{\"id\": ");
        match &e.field {
            Some(f) => {
                let _ = write!(s, "{}", f.id);
            }
            None => s.push_str("null"),
        }
        s.push_str(", \"name\": ");
        json_string(e.field.as_ref().map(|f| f.name.as_str()).unwrap_or(&e.column), &mut s);
        s.push_str(", \"type\": ");
        match &e.field {
            Some(f) => json_string(&f.type_name, &mut s),
            None => s.push_str("null"),
        }
        s.push_str(", \"sqlColumn\": ");
        json_string(&e.column, &mut s);
        s.push_str(", \"sqlType\": ");
        json_string(&e.sql_type, &mut s);
        s.push_str(", \"extension\": ");
        match &e.ext_name {
            None => {
                s.push_str("{\"app\": ");
                json_string(&e.ext_app_id, &mut s);
                s.push('}');
            }
            Some(n) => {
                s.push_str("{\"name\": ");
                json_string(n, &mut s);
                s.push_str(", \"app\": ");
                json_string(e.ext_app_guid.as_deref().unwrap_or(""), &mut s);
                s.push_str(", \"appName\": ");
                json_string(e.ext_app_name.as_deref().unwrap_or(""), &mut s);
                s.push('}');
            }
        }
        s.push('}');
    }
    s.push_str("]}");
    Ok(s)
}
