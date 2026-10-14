//! The `.bacpac` container (port of `Bacpac.cs`): an Open Packaging (zip) container
//! holding `model.xml` (the schema), `Origin.xml` (package metadata and a checksum over
//! model.xml), and one folder of native-BCP data streams per table with rows, named
//! `Data/<schema>.<url-encoded table>/TableData-NNN-MMMMM.BCP`.
//!
//! Structure derived by inspection of exports produced by sqlpackage 170.5.76 from SQL
//! Server 2022 and validated by importing the same file back into a SQL Server and
//! comparing full tables (bcdb PROVENANCE.md "bacpac container").

use crate::bcp::{BacpacColumn, BcpRowReader};
use crate::error::{Error, Result};
use crate::io::ByteSource;
use crate::source::{BcSource, SourceTable};
use crate::types::SysColumn;
use crate::value::Value;
use crate::zip::ZipIndex;
use quick_xml::events::Event;
use quick_xml::Reader;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io::{BufReader, Read};
use std::sync::{Mutex, OnceLock};

/// The only Data stream format version whose row framing this reader has derived.
pub const SUPPORTED_DATA_STREAM_VERSION: &str = "2.0.0.0";

/// A table as model.xml declares it: its columns in data-stream order and its primary key.
#[derive(Debug, Clone)]
pub struct BacpacTable {
    pub schema: String,
    pub name: String,
    pub columns: Vec<BacpacColumn>,
    /// Primary-key columns in key order: the clustered key an `$ext` companion joins on.
    pub key_columns: Vec<String>,
    /// Data-stream entries in (batch, sequence) order; empty for a table with no rows.
    pub data_entries: Vec<String>,
}

struct Model {
    tables: Vec<BacpacTable>,
    source_tables: Vec<SourceTable>,
}

pub struct BacpacSource {
    src: Box<dyn ByteSource>,
    zip: ZipIndex,
    database_name: String,
    product_version: String,
    model_checksum: Option<String>,
    model: OnceLock<std::result::Result<Model, Error>>,
    row_counts: Mutex<HashMap<usize, i64>>,
}

// ---- a minimal element tree, for the few model.xml subtrees that matter -------------

struct Node {
    name: String,
    attrs: Vec<(String, String)>,
    children: Vec<Node>,
}

impl Node {
    fn attr(&self, k: &str) -> Option<&str> {
        self.attrs.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str())
    }

    /// `Relationship[@Name=relationship]/Entry/<leaf>` children: the one shape every
    /// lookup in model.xml has.
    fn under<'a>(&'a self, relationship: &'a str, leaf: &'a str) -> impl Iterator<Item = &'a Node> + 'a {
        self.children
            .iter()
            .filter(move |r| r.name == "Relationship" && r.attr("Name") == Some(relationship))
            .flat_map(|r| r.children.iter().filter(|e| e.name == "Entry"))
            .flat_map(move |e| e.children.iter().filter(move |c| c.name == leaf))
    }

    fn first_under<'a>(&'a self, relationship: &'a str, leaf: &'a str) -> Option<&'a Node> {
        self.under(relationship, leaf).next()
    }

    fn prop(&self, name: &str) -> Option<&str> {
        self.children
            .iter()
            .filter(|c| c.name == "Property")
            .find(|p| p.attr("Name") == Some(name))
            .and_then(|p| p.attr("Value"))
    }
}

fn xml_err(what: &str, e: impl std::fmt::Display) -> Error {
    crate::invalid!("{what}: {e}")
}

fn start_node(e: &quick_xml::events::BytesStart<'_>, what: &str) -> Result<Node> {
    let name = String::from_utf8_lossy(e.local_name().as_ref()).into_owned();
    let mut attrs = Vec::new();
    for a in e.attributes() {
        let a = a.map_err(|x| xml_err(what, x))?;
        let k = String::from_utf8_lossy(a.key.local_name().as_ref()).into_owned();
        let v = a.unescape_value().map_err(|x| xml_err(what, x))?.into_owned();
        attrs.push((k, v));
    }
    Ok(Node { name, attrs, children: Vec::new() })
}

/// Read the subtree whose start tag was just consumed.
fn read_subtree<R: std::io::BufRead>(reader: &mut Reader<R>, start: Node, buf: &mut Vec<u8>) -> Result<Node> {
    let mut stack = vec![start];
    loop {
        buf.clear();
        match reader.read_event_into(buf).map_err(|e| xml_err("model.xml", e))? {
            Event::Start(e) => stack.push(start_node(&e, "model.xml")?),
            Event::Empty(e) => {
                let n = start_node(&e, "model.xml")?;
                stack.last_mut().unwrap().children.push(n);
            }
            Event::End(_) => {
                let done = stack.pop().unwrap();
                match stack.last_mut() {
                    Some(parent) => parent.children.push(done),
                    None => return Ok(done),
                }
            }
            Event::Eof => return Err(Error::InvalidData("model.xml ends inside an element".into())),
            _ => {}
        }
    }
}

/// Splits a model.xml identifier `[a].[b]` or `[a].[b].[c]` into its bracketed parts.
fn split_name(bracketed: &str) -> Result<(String, String, Option<String>)> {
    let b = bracketed.as_bytes();
    let mut parts: Vec<String> = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] != b'[' {
            return Err(crate::invalid!("model.xml: malformed identifier '{bracketed}'"));
        }
        // find the closing ']' that is not doubled
        let mut j = i + 1;
        loop {
            match bracketed[j..].find(']') {
                None => return Err(crate::invalid!("model.xml: malformed identifier '{bracketed}'")),
                Some(k) => {
                    let end = j + k;
                    if end + 1 < b.len() && b[end + 1] == b']' {
                        j = end + 2;
                        continue;
                    }
                    parts.push(bracketed[i + 1..end].replace("]]", "]"));
                    i = end + 1;
                    break;
                }
            }
        }
        if i < b.len() && b[i] == b'.' {
            i += 1;
        }
    }
    let n = parts.len();
    if n != 2 && n != 3 {
        return Err(crate::invalid!("model.xml: identifier '{bracketed}' has {n} parts"));
    }
    let mut it = parts.into_iter();
    let (p1, p2, p3) = (it.next().unwrap(), it.next().unwrap(), it.next());
    Ok((p1, p2, p3))
}

fn parse_table(el: &Node) -> Result<(String, String, Vec<BacpacColumn>)> {
    let (schema, table, _) = split_name(el.attr("Name").unwrap_or(""))?;
    let mut cols = Vec::new();
    for col in el.under("Columns", "Element") {
        // Computed columns (SqlComputedColumn) are not stored and carry no data.
        if col.attr("Type") != Some("SqlSimpleColumn") {
            continue;
        }
        let qualified = col.attr("Name").unwrap_or("");
        let col_name = split_name(qualified)?.2.ok_or_else(|| {
            crate::invalid!("model.xml: column identifier '{qualified}' does not name a column of a table")
        })?;
        let nullable = col.prop("IsNullable") != Some("False");
        let spec = col
            .first_under("TypeSpecifier", "Element")
            .ok_or_else(|| crate::invalid!("model.xml: column [{schema}].[{table}].[{col_name}] has no type specifier"))?;
        let type_name = spec
            .first_under("Type", "References")
            .and_then(|r| r.attr("Name"))
            .map(|n| n.trim_matches(|c| c == '[' || c == ']').to_string())
            .ok_or_else(|| crate::invalid!("model.xml: column [{schema}].[{table}].[{col_name}] names no type"))?;
        let (mut is_max, mut length, mut precision, mut scale) = (false, 0i32, 0u8, 0u8);
        for p in spec.children.iter().filter(|c| c.name == "Property") {
            let v = p.attr("Value");
            let num = |what: &str| -> Result<i64> {
                v.unwrap_or("0")
                    .parse::<i64>()
                    .map_err(|_| crate::invalid!("model.xml: column {col_name}: {what} '{}' is not a number", v.unwrap_or("")))
            };
            match p.attr("Name") {
                Some("IsMax") => is_max = v == Some("True"),
                Some("Length") => length = num("Length")? as i32,
                Some("Precision") => precision = num("Precision")? as u8,
                Some("Scale") => scale = num("Scale")? as u8,
                _ => {}
            }
        }
        cols.push(BacpacColumn::from_model(&col_name, &type_name, nullable, is_max, length, precision, scale));
    }
    Ok((schema, table, cols))
}

fn parse_key(el: &Node) -> Result<Option<(String, Vec<String>)>> {
    let Some(target) = el.first_under("DefiningTable", "References").and_then(|r| r.attr("Name")) else {
        return Ok(None);
    };
    let (schema, table, _) = split_name(target)?;
    let mut cols = Vec::new();
    for spec in el.under("ColumnSpecifications", "Element") {
        if let Some(n) = spec.first_under("Column", "References").and_then(|r| r.attr("Name")) {
            cols.push(split_name(n)?.2.unwrap_or_default());
        }
    }
    Ok(if cols.is_empty() { None } else { Some((format!("{schema}.{table}"), cols)) })
}

/// A reader that hashes every byte that passes through it.
struct HashingRead<R> {
    inner: R,
    hasher: Sha256,
}

impl<R: Read> Read for HashingRead<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.hasher.update(&buf[..n]);
        Ok(n)
    }
}

/// Elements of a small XML document: local name, attributes, first text content.
struct XmlEl {
    name: String,
    attrs: Vec<(String, String)>,
    text: String,
}

fn scan_small(xml: &[u8], what: &str) -> Result<Vec<XmlEl>> {
    let mut reader = Reader::from_reader(xml);
    let mut out: Vec<XmlEl> = Vec::new();
    let mut open: Vec<usize> = Vec::new();
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match reader.read_event_into(&mut buf).map_err(|e| xml_err(what, e))? {
            Event::Start(e) => {
                let n = start_node(&e, what)?;
                out.push(XmlEl { name: n.name, attrs: n.attrs, text: String::new() });
                open.push(out.len() - 1);
            }
            Event::Empty(e) => {
                let n = start_node(&e, what)?;
                out.push(XmlEl { name: n.name, attrs: n.attrs, text: String::new() });
            }
            Event::Text(t) => {
                if let Some(&i) = open.last() {
                    if out[i].text.is_empty() {
                        out[i].text = t.unescape().map_err(|e| xml_err(what, e))?.into_owned();
                    }
                }
            }
            Event::End(_) => {
                open.pop();
            }
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(out)
}

fn unescape_uri(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let (Some(h), Some(l)) = ((b[i + 1] as char).to_digit(16), (b[i + 2] as char).to_digit(16)) {
                out.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned())
}

impl BacpacSource {
    pub fn open(src: Box<dyn ByteSource>) -> Result<BacpacSource> {
        let zip = ZipIndex::parse(&*src)?;
        let origin = scan_small(&zip.read(&*src, "Origin.xml")?, "Origin.xml")?;
        let data_version = origin
            .iter()
            .find(|e| {
                e.name == "Version" && e.attrs.iter().any(|(k, v)| k == "StreamName" && v == "Data")
            })
            .map(|e| e.text.trim().to_string());
        let Some(data_version) = data_version else {
            return Err(Error::InvalidData(
                "Origin.xml declares no Data stream version — refusing to guess the row format".into(),
            ));
        };
        if data_version != SUPPORTED_DATA_STREAM_VERSION {
            return Err(crate::invalid!(
                "Data stream version {data_version}, but only {SUPPORTED_DATA_STREAM_VERSION} has a derived row format — refusing to guess"
            ));
        }
        let model_checksum = origin
            .iter()
            .find(|e| e.name == "Checksum" && e.attrs.iter().any(|(k, v)| k == "Uri" && v == "/model.xml"))
            .map(|e| e.text.trim().to_string());
        let product_version = origin
            .iter()
            .find(|e| e.name == "ProductVersion")
            .map(|e| e.text.trim().to_string())
            .unwrap_or_else(|| "?".into());
        let database_name = scan_small(&zip.read(&*src, "DacMetadata.xml")?, "DacMetadata.xml")?
            .into_iter()
            .find(|e| e.name == "Name")
            .map(|e| e.text.trim().to_string())
            .unwrap_or_default();
        Ok(BacpacSource {
            src,
            zip,
            database_name,
            product_version,
            model_checksum,
            model: OnceLock::new(),
            row_counts: Mutex::new(HashMap::new()),
        })
    }

    pub fn database_name(&self) -> &str {
        &self.database_name
    }

    fn model(&self) -> Result<&Model> {
        self.model.get_or_init(|| self.load_model()).as_ref().map_err(|e| e.clone())
    }

    pub fn bacpac_tables(&self) -> Result<&[BacpacTable]> {
        Ok(&self.model()?.tables)
    }

    /// Streams model.xml once (113 MB in a real export, so never as a DOM), keeping only
    /// table and primary-key elements, and verifies Origin.xml's checksum over the same
    /// bytes as they pass. Called on first use, not at open, so the first query pays it.
    fn load_model(&self) -> Result<Model> {
        let entry = self
            .zip
            .find("model.xml")
            .ok_or_else(|| Error::InvalidData("the package has no model.xml — not a bacpac?".into()))?;
        let raw = self.zip.open(&*self.src, entry)?;
        let hashing = HashingRead { inner: raw, hasher: Sha256::new() };
        let mut reader = Reader::from_reader(BufReader::with_capacity(256 * 1024, hashing));
        reader.config_mut().check_end_names = false;

        let mut tables_cols: Vec<(String, String, Vec<BacpacColumn>)> = Vec::new();
        let mut keys: HashMap<String, Vec<String>> = HashMap::new();
        let mut buf = Vec::new();
        loop {
            buf.clear();
            let ev = reader.read_event_into(&mut buf).map_err(|e| xml_err("model.xml", e))?;
            match ev {
                Event::Start(e) if e.local_name().as_ref() == b"Element" => {
                    let ty = e
                        .try_get_attribute("Type")
                        .map_err(|x| xml_err("model.xml", x))?
                        .map(|a| a.unescape_value().map(|c| c.into_owned()))
                        .transpose()
                        .map_err(|x| xml_err("model.xml", x))?;
                    if matches!(ty.as_deref(), Some("SqlTable") | Some("SqlPrimaryKeyConstraint")) {
                        let start = start_node(&e, "model.xml")?;
                        let mut sub = Vec::new();
                        let el = read_subtree(&mut reader, start, &mut sub)?;
                        if ty.as_deref() == Some("SqlTable") {
                            tables_cols.push(parse_table(&el)?);
                        } else if let Some((k, cols)) = parse_key(&el)? {
                            keys.insert(k, cols);
                        }
                    }
                }
                Event::Eof => break,
                _ => {}
            }
        }
        // XmlReader may stop before the last byte; draining completes the hash and proves
        // the entry decompresses in full.
        let mut hashing = reader.into_inner().into_inner();
        std::io::copy(&mut hashing, &mut std::io::sink())?;
        let actual = hex_upper(&hashing.hasher.finalize());
        if let Some(declared) = &self.model_checksum {
            if !actual.eq_ignore_ascii_case(declared) {
                return Err(crate::invalid!(
                    "model.xml checksum {actual} does not match the {declared} Origin.xml declares — the package is damaged or was edited"
                ));
            }
        }

        let mut entries = self.data_entries_by_table()?;
        let mut tables: Vec<BacpacTable> = Vec::new();
        let mut seen: HashMap<String, ()> = HashMap::new();
        for (schema, name, cols) in tables_cols {
            let key = format!("{schema}.{name}");
            let display = if schema == "dbo" { name.clone() } else { key.clone() };
            let files = entries.remove(&key).unwrap_or_default();
            // Ordinal, and duplicates throw: a case-sensitive-collation database can hold two
            // tables that differ only in case, and silently keeping one of them would be a
            // table quietly missing from every listing.
            if seen.insert(display.clone(), ()).is_some() {
                return Err(crate::invalid!("model.xml declares '{display}' twice"));
            }
            tables.push(BacpacTable {
                schema,
                name,
                columns: cols,
                key_columns: keys.get(&key).cloned().unwrap_or_default(),
                data_entries: files,
            });
        }
        if let Some(first) = entries.keys().min() {
            // (zip order in bcdb; any one name identifies the mismatch)
            return Err(crate::invalid!(
                "data stream for '{first}' has no table in model.xml — schema/data mismatch, refusing to guess"
            ));
        }
        let source_tables = tables
            .iter()
            .enumerate()
            .map(|(i, t)| SourceTable {
                name: if t.schema == "dbo" { t.name.clone() } else { format!("{}.{}", t.schema, t.name) },
                compression: "-".into(), // a bacpac stores logical rows: no storage compression
                handle: i,
            })
            .collect();
        Ok(Model { tables, source_tables })
    }

    /// Data entries grouped by `<schema>.<table>` and ordered by (NNN, MMMMM). NNN is an
    /// export batch, MMMMM its continuation once a batch passes ~4 MiB; both boundaries
    /// fall between rows in every export measured, but concatenating in this order is
    /// correct either way.
    fn data_entries_by_table(&self) -> Result<HashMap<String, Vec<String>>> {
        let mut by: HashMap<String, Vec<(i64, i64, String)>> = HashMap::new();
        for e in &self.zip.entries {
            let n = &e.name;
            let Some(rest) = n.strip_prefix("Data/") else { continue };
            let Some((folder, file)) = rest.split_once('/') else {
                return Err(crate::invalid!("unexpected data entry '{n}'"));
            };
            let Some(nums) = file.strip_prefix("TableData-").and_then(|f| f.strip_suffix(".BCP")) else {
                return Err(crate::invalid!("unexpected data entry '{n}' — expected TableData-NNN-MMMMM.BCP"));
            };
            let parts: Vec<&str> = nums.split('-').collect();
            let parsed = if parts.len() == 2 { (parts[0].parse::<i64>().ok(), parts[1].parse::<i64>().ok()) } else { (None, None) };
            let (Some(batch), Some(seq)) = parsed else {
                return Err(crate::invalid!("cannot read the batch/sequence numbers of data entry '{n}'"));
            };
            let Some((schema, table)) = folder.split_once('.') else {
                return Err(crate::invalid!("data folder '{folder}' is not <schema>.<table>"));
            };
            let key = format!("{}.{}", unescape_uri(schema), unescape_uri(table));
            by.entry(key).or_default().push((batch, seq, n.clone()));
        }
        Ok(by
            .into_iter()
            .map(|(k, mut v)| {
                v.sort();
                (k, v.into_iter().map(|x| x.2).collect())
            })
            .collect())
    }
}

fn hex_upper(b: &[u8]) -> String {
    use std::fmt::Write;
    b.iter().fold(String::with_capacity(b.len() * 2), |mut s, x| {
        let _ = write!(s, "{x:02X}");
        s
    })
}

impl BcSource for BacpacSource {
    fn tables(&self) -> Result<&[SourceTable]> {
        Ok(&self.model()?.source_tables)
    }

    fn row_count(&self, t: &SourceTable) -> Result<i64> {
        if let Some(n) = self.row_counts.lock().unwrap().get(&t.handle) {
            return Ok(*n);
        }
        let mut n = 0i64;
        self.read_rows(t, &[], &mut |_| {
            n += 1;
            Ok(true)
        })?;
        self.row_counts.lock().unwrap().insert(t.handle, n);
        Ok(n)
    }

    fn columns(&self, t: &SourceTable) -> Result<Vec<SysColumn>> {
        Ok(self.model()?.tables[t.handle].columns.iter().enumerate().map(|(i, c)| c.to_sys_column(i as i32 + 1)).collect())
    }

    /// The primary-key constraint model.xml declares.
    fn row_key_columns(&self, t: &SourceTable) -> Result<Vec<String>> {
        Ok(self.model()?.tables[t.handle].key_columns.clone())
    }

    fn read_rows(
        &self,
        t: &SourceTable,
        columns: &[SysColumn],
        sink: &mut dyn FnMut(&[Value]) -> Result<bool>,
    ) -> Result<()> {
        let table = &self.model()?.tables[t.handle];
        let want = columns
            .iter()
            .map(|c| {
                table
                    .columns
                    .iter()
                    .find(|bc| bc.name == c.name)
                    .cloned()
                    .ok_or_else(|| crate::invalid!("bacpac table {} has no column {}", table.name, c.name))
            })
            .collect::<Result<Vec<_>>>()?;
        let reader = BcpRowReader::new(&table.name, &table.columns);
        for entry in &table.data_entries {
            let e = self
                .zip
                .find(entry)
                .ok_or_else(|| crate::invalid!("the package has no {entry} — not a bacpac?"))?;
            let stream = self.zip.open(&*self.src, e)?;
            if !reader.read(stream, &want, sink)? {
                break;
            }
        }
        Ok(())
    }

    fn banner(&self) -> Result<String> {
        let m = self.model()?;
        let with_data = m.tables.iter().filter(|t| !t.data_entries.is_empty()).count();
        Ok(format!(
            "[bacpac \"{}\" exported by DacFx {}: {} tables in model.xml, {} with data]",
            self.database_name,
            self.product_version,
            m.tables.len(),
            with_data
        ))
    }
}
