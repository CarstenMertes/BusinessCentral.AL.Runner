//! A RapidStart configuration package as a source of test data.
//!
//! Business Central ships its demo data as `platform/ConfigurationPackages/*.rapidstart`: a gzip file holding UTF-16 XML.
//! The root is a `DataList`; each child is one table (`<TableID>`, `<PageID>`, an optional `<SkipTableTriggers>`, then one
//! element per record whose children are the fields). A field element is named after the field with every character that
//! is not a letter or a digit removed (`Due Date Calculation` is `DueDateCalculation`); the first record carries the
//! `PrimaryKey` and `ProcessingOrder` attributes, the later records only the values.
//!
//! [`Package::parse`] reads the bytes. [`PackageProvider`] serves the tables as a [`TableProvider`]: it needs the compiled
//! schema to type the text values, so it asks for it ([`TableProvider::wants_fields`]). A package holds no company name;
//! the provider is given the one the run uses.

use al_host::testdata::{Cell, FieldKind, RequestField, TableError, TableProvider, TableRequest, TableRows};
use quick_xml::Reader;
use quick_xml::events::Event;
use std::collections::HashMap;
use std::io::Read;
use std::sync::Arc;

/// One table of a package.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageTable {
    /// The AL object id of the table (`<TableID>`).
    pub table_id: u32,
    /// `<SkipTableTriggers>1`: the import writes the rows without running the table's triggers. A table without the element
    /// runs them in BC; this reader gives such a table the same raw rows (see [`Package::triggered_tables`]).
    pub skip_table_triggers: bool,
    /// The field elements in `ProcessingOrder`.
    pub columns: Vec<String>,
    /// Which columns are part of the primary key (`PrimaryKey` attribute on the first record).
    pub primary_key: Vec<bool>,
    /// One text per column and record; an empty text is a blank value.
    pub rows: Vec<Vec<String>>,
}

/// A parsed package.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Package {
    /// The package code (`W1.ENU.EXTENDED`).
    pub code: String,
    pub product_version: String,
    pub tables: Vec<PackageTable>,
}

impl Package {
    /// Reads a `.rapidstart` file's bytes: gzip when it starts with the gzip magic, then UTF-16 (or UTF-8) XML.
    pub fn parse(bytes: &[u8]) -> Result<Package, String> {
        let raw = if bytes.starts_with(&[0x1f, 0x8b]) {
            let mut out = Vec::with_capacity(bytes.len() * 6);
            flate2::read::GzDecoder::new(bytes)
                .read_to_end(&mut out)
                .map_err(|e| format!("not a gzip file: {e}"))?;
            out
        } else {
            bytes.to_vec()
        };
        let text = decode_text(&raw)?;
        Package::parse_xml(&text)
    }

    /// Reads the XML text of a package.
    pub fn parse_xml(xml: &str) -> Result<Package, String> {
        let mut reader = Reader::from_str(xml);
        let mut pkg = Package::default();
        let mut depth = 0usize;
        // The table being read: its data, the name of the record element, and the map of column names.
        let mut table: Option<PackageTable> = None;
        let mut index: HashMap<String, usize> = HashMap::new();
        let mut record_name: Option<String> = None;
        let mut record: Option<Vec<String>> = None;
        let mut first_record = true;
        // The orders of the first record, to sort the columns afterwards.
        let mut orders: Vec<Option<u32>> = Vec::new();
        let mut field: Option<(String, String)> = None;
        let mut field_attrs: (Option<u32>, bool) = (None, false);
        let mut simple: Option<(String, String)> = None;
        loop {
            match reader
                .read_event()
                .map_err(|e| format!("XML error at {}: {e}", reader.buffer_position()))?
            {
                Event::Start(e) => {
                    depth += 1;
                    let name = String::from_utf8_lossy(e.name().as_ref()).into_owned();
                    match depth {
                        1 => {
                            for a in e.attributes().flatten() {
                                let v = a.unescape_value().map(|v| v.into_owned()).unwrap_or_default();
                                match a.key.as_ref() {
                                    b"Code" => pkg.code = v,
                                    b"ProductVersion" => pkg.product_version = v,
                                    _ => {}
                                }
                            }
                        }
                        2 => {
                            table = Some(PackageTable {
                                table_id: 0,
                                skip_table_triggers: false,
                                columns: Vec::new(),
                                primary_key: Vec::new(),
                                rows: Vec::new(),
                            });
                            index.clear();
                            record_name = None;
                            first_record = true;
                            orders.clear();
                        }
                        3 => {
                            if matches!(name.as_str(), "TableID" | "PageID" | "SkipTableTriggers") {
                                simple = Some((name, String::new()));
                            } else {
                                record_name.get_or_insert(name);
                                record = Some(vec![String::new(); table.as_ref().map_or(0, |t| t.columns.len())]);
                            }
                        }
                        4 => {
                            field_attrs = (None, false);
                            for a in e.attributes().flatten() {
                                let v = a.unescape_value().map(|v| v.into_owned()).unwrap_or_default();
                                match a.key.as_ref() {
                                    b"ProcessingOrder" => field_attrs.0 = v.trim().parse().ok(),
                                    b"PrimaryKey" => field_attrs.1 = v.trim() == "1",
                                    _ => {}
                                }
                            }
                            field = Some((name, String::new()));
                        }
                        _ => {}
                    }
                }
                Event::Empty(e) => {
                    // An element with no content: a blank field value.
                    let name = String::from_utf8_lossy(e.name().as_ref()).into_owned();
                    if depth == 3 {
                        if let (Some(t), Some(rec)) = (table.as_mut(), record.as_mut()) {
                            let mut po = None;
                            let mut pk = false;
                            for a in e.attributes().flatten() {
                                let v = a.unescape_value().map(|v| v.into_owned()).unwrap_or_default();
                                match a.key.as_ref() {
                                    b"ProcessingOrder" => po = v.trim().parse().ok(),
                                    b"PrimaryKey" => pk = v.trim() == "1",
                                    _ => {}
                                }
                            }
                            set_field(
                                t,
                                &mut index,
                                rec,
                                &mut orders,
                                &name,
                                String::new(),
                                po,
                                pk,
                                first_record,
                            );
                        }
                    }
                }
                Event::Text(t) => {
                    let s = t.unescape().map_err(|e| format!("XML text error: {e}"))?;
                    if let Some((_, v)) = field.as_mut() {
                        v.push_str(&s);
                    } else if let Some((_, v)) = simple.as_mut() {
                        v.push_str(&s);
                    }
                }
                Event::CData(t) => {
                    let s = String::from_utf8_lossy(&t.into_inner()).into_owned();
                    if let Some((_, v)) = field.as_mut() {
                        v.push_str(&s);
                    }
                }
                Event::End(_) => {
                    match depth {
                        4 => {
                            if let (Some((name, value)), Some(t), Some(rec)) =
                                (field.take(), table.as_mut(), record.as_mut())
                            {
                                let (po, pk) = field_attrs;
                                set_field(t, &mut index, rec, &mut orders, &name, value, po, pk, first_record);
                            }
                        }
                        3 => {
                            if let Some((name, value)) = simple.take() {
                                if let Some(t) = table.as_mut() {
                                    match name.as_str() {
                                        "TableID" => {
                                            t.table_id = value
                                                .trim()
                                                .parse()
                                                .map_err(|_| format!("TableID '{}' is not a number", value.trim()))?;
                                        }
                                        "SkipTableTriggers" => t.skip_table_triggers = value.trim() == "1",
                                        _ => {}
                                    }
                                }
                            } else if let (Some(t), Some(mut rec)) = (table.as_mut(), record.take()) {
                                rec.resize(t.columns.len(), String::new());
                                t.rows.push(rec);
                                first_record = false;
                            }
                        }
                        2 => {
                            if let Some(mut t) = table.take() {
                                // ProcessingOrder: the order the import assigns the fields in. Columns without one keep their place.
                                let n = t.columns.len();
                                let mut perm: Vec<usize> = (0..n).collect();
                                perm.sort_by_key(|&i| (orders.get(i).copied().flatten().unwrap_or(u32::MAX), i));
                                if perm.iter().enumerate().any(|(a, &b)| a != b) {
                                    t.columns = perm.iter().map(|&i| t.columns[i].clone()).collect();
                                    t.primary_key = perm.iter().map(|&i| t.primary_key[i]).collect();
                                    for r in &mut t.rows {
                                        r.resize(n, String::new());
                                        *r = perm.iter().map(|&i| std::mem::take(&mut r[i])).collect();
                                    }
                                }
                                if t.table_id != 0 {
                                    pkg.tables.push(t);
                                }
                            }
                        }
                        _ => {}
                    }
                    depth = depth.saturating_sub(1);
                }
                Event::Eof => break,
                _ => {}
            }
        }
        if pkg.tables.is_empty() && pkg.code.is_empty() {
            return Err("not a RapidStart package (no DataList root)".into());
        }
        Ok(pkg)
    }

    /// The tables that have rows and do not skip the table triggers: BC runs their triggers while it imports them.
    pub fn triggered_tables(&self) -> Vec<u32> {
        self.tables
            .iter()
            .filter(|t| !t.skip_table_triggers && !t.rows.is_empty())
            .map(|t| t.table_id)
            .collect()
    }

    /// The table with this object id.
    pub fn table(&self, id: u32) -> Option<&PackageTable> {
        self.tables.iter().find(|t| t.table_id == id)
    }
}

#[allow(clippy::too_many_arguments)]
fn set_field(
    t: &mut PackageTable,
    index: &mut HashMap<String, usize>,
    rec: &mut Vec<String>,
    orders: &mut Vec<Option<u32>>,
    name: &str,
    value: String,
    order: Option<u32>,
    pk: bool,
    first_record: bool,
) {
    let i = match index.get(name) {
        Some(&i) => i,
        None => {
            // A new column: the first record defines them; a later record may bring one the first lacked.
            let _ = first_record;
            let i = t.columns.len();
            t.columns.push(name.to_string());
            t.primary_key.push(pk);
            orders.push(order);
            index.insert(name.to_string(), i);
            i
        }
    };
    if rec.len() <= i {
        rec.resize(i + 1, String::new());
    }
    rec[i] = value;
}

/// Decodes the XML bytes: UTF-16 with a byte order mark (or UTF-8), and drops the declaration, which names an encoding the
/// text no longer has.
fn decode_text(raw: &[u8]) -> Result<String, String> {
    let mut text = if raw.starts_with(&[0xff, 0xfe]) || raw.starts_with(&[0xfe, 0xff]) {
        let le = raw[0] == 0xff;
        let units: Vec<u16> = raw[2..]
            .chunks_exact(2)
            .map(|c| {
                if le {
                    u16::from_le_bytes([c[0], c[1]])
                } else {
                    u16::from_be_bytes([c[0], c[1]])
                }
            })
            .collect();
        String::from_utf16(&units).map_err(|e| format!("the package is not valid UTF-16: {e}"))?
    } else {
        let body = raw.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(raw);
        String::from_utf8(body.to_vec()).map_err(|e| format!("the package is not valid UTF-8: {e}"))?
    };
    if text.starts_with("<?xml") {
        if let Some(end) = text.find("?>") {
            text.drain(..end + 2);
        }
    }
    Ok(text)
}

/// The name a field has in a package: the AL name with every character that is not a letter or a digit removed,
/// except that a hyphen becomes an underscore. The shipped package pins this: `Unit_AmountRoundingPrecision` for
/// "Unit-Amount Rounding Precision", `Bill_toName` for "Bill-to Name", `Asm_to_OrderShptBinCode` for
/// "Asm.-to-Order Shpt. Bin Code"; no tag of the package has a dot, slash or percent sign in it.
pub fn element_name(field_name: &str) -> String {
    field_name
        .chars()
        .filter_map(|c| if c == '-' { Some('_') } else { c.is_alphanumeric().then_some(c) })
        .collect()
}

// ---- values --------------------------------------------------------------------------------------------------------

/// Days since 0001-01-01 (that day is 0) of a proleptic Gregorian date.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    // Howard Hinnant's algorithm gives days since 1970-01-01.
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let since_1970 = era * 146_097 + doe - 719_468;
    // 0001-01-01 is 719162 days before 1970-01-01.
    since_1970 + 719_162
}

fn parse_date(s: &str) -> Option<i64> {
    let mut it = s.split('-');
    let y: i64 = it.next()?.parse().ok()?;
    let m: i64 = it.next()?.parse().ok()?;
    let d: i64 = it.next()?.parse().ok()?;
    if it.next().is_some() || !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    Some(days_from_civil(y, m, d))
}

/// `HH:MM:SS` with optional fraction, in milliseconds.
fn parse_clock(s: &str) -> Option<i64> {
    let s = s.trim_end_matches('Z');
    let (hms, frac) = s.split_once('.').unwrap_or((s, ""));
    let mut it = hms.split(':');
    let h: i64 = it.next()?.parse().ok()?;
    let m: i64 = it.next()?.parse().ok()?;
    let sec: i64 = it.next().unwrap_or("0").parse().ok()?;
    if it.next().is_some() {
        return None;
    }
    let ms: i64 = if frac.is_empty() {
        0
    } else {
        let f: String = frac.chars().take(3).collect();
        format!("{f:0<3}").parse().ok()?
    };
    Some(((h * 60 + m) * 60 + sec) * 1000 + ms)
}

fn parse_decimal(s: &str) -> Option<Cell> {
    let (negative, digits) = match s.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, s.strip_prefix('+').unwrap_or(s)),
    };
    let (whole, frac) = digits.split_once('.').unwrap_or((digits, ""));
    if (whole.is_empty() && frac.is_empty())
        || !whole.bytes().all(|b| b.is_ascii_digit())
        || !frac.bytes().all(|b| b.is_ascii_digit())
        || frac.len() > 28
    {
        return None;
    }
    let magnitude: u128 = format!("{whole}{frac}").parse().ok()?;
    Some(Cell::Decimal {
        negative,
        magnitude,
        scale: frac.len() as u8,
    })
}

/// The 16 bytes of a Guid the way the database stores them (.NET byte order) from its text.
fn parse_guid(s: &str) -> Option<[u8; 16]> {
    let hex: String = s.chars().filter(|c| !matches!(c, '{' | '}' | '-')).collect();
    if hex.len() != 32 {
        return None;
    }
    let mut t = [0u8; 16];
    for (i, b) in t.iter_mut().enumerate() {
        *b = u8::from_str_radix(hex.get(2 * i..2 * i + 2)?, 16).ok()?;
    }
    Some([
        t[3], t[2], t[1], t[0], t[5], t[4], t[7], t[6], t[8], t[9], t[10], t[11], t[12], t[13], t[14], t[15],
    ])
}

/// A date formula (`<-WD1+2W>`) as BC stores it: the quantors `C D WD W M Q Y` are the control characters 1 to 7.
fn date_formula_stored(text: &str) -> Option<String> {
    let body = text
        .trim()
        .strip_prefix('<')
        .and_then(|t| t.strip_suffix('>'))
        .unwrap_or(text.trim());
    let mut out = String::new();
    let mut chars = body.chars().peekable();
    while let Some(c) = chars.next() {
        let q = match c.to_ascii_uppercase() {
            'C' => Some('\u{1}'),
            'D' => Some('\u{2}'),
            'W' if chars.peek().is_some_and(|n| n.eq_ignore_ascii_case(&'D')) => {
                chars.next();
                Some('\u{3}')
            }
            'W' => Some('\u{4}'),
            'M' => Some('\u{5}'),
            'Q' => Some('\u{6}'),
            'Y' => Some('\u{7}'),
            _ => None,
        };
        match q {
            Some(q) => out.push(q),
            None if c.is_ascii_digit() || matches!(c, '+' | '-') => out.push(c),
            None => return None,
        }
    }
    Some(out)
}

/// A package value as the cell the runtime rebuilds the field's value from. `Err`: why the value cannot be read.
pub fn convert(kind: FieldKind, text: &str) -> Result<Cell, String> {
    let t = text.trim();
    // A blank value: whitespace only. Text keeps what it holds, but a value of blanks is the blank value.
    if t.is_empty() {
        return Ok(match kind {
            FieldKind::Text | FieldKind::Code | FieldKind::DateFormula => Cell::Text(String::new()),
            _ => Cell::Null,
        });
    }
    let bad = |what: &str| Err(format!("'{t}' is not {what}"));
    Ok(match kind {
        FieldKind::Text | FieldKind::Code => Cell::Text(text.to_string()),
        FieldKind::Boolean => match t {
            "0" | "false" | "False" | "No" => Cell::Bool(false),
            "1" | "true" | "True" | "Yes" => Cell::Bool(true),
            _ => return bad("a Boolean"),
        },
        FieldKind::Integer | FieldKind::BigInteger | FieldKind::Byte | FieldKind::Ordinal | FieldKind::Duration => {
            match t.parse::<i64>() {
                Ok(i) => Cell::Int(i),
                Err(_) => return bad("a whole number"),
            }
        }
        FieldKind::Decimal => match parse_decimal(t) {
            Some(c) => c,
            None => return bad("a decimal"),
        },
        FieldKind::Date => match parse_date(t.split('T').next().unwrap_or(t)) {
            Some(days) => Cell::Date { days },
            None => return bad("a date (yyyy-mm-dd)"),
        },
        FieldKind::Time => match parse_clock(t) {
            Some(ms) => Cell::Time { units: ms, scale: 3 },
            None => return bad("a time (hh:mm:ss)"),
        },
        FieldKind::DateTime => {
            let (d, c) = t.split_once('T').unwrap_or((t, "00:00:00"));
            match (parse_date(d), parse_clock(c)) {
                (Some(days), Some(ms)) => Cell::DateTime2 {
                    days,
                    units: ms,
                    scale: 3,
                },
                _ => return bad("a date and time (yyyy-mm-ddThh:mm:ss)"),
            }
        }
        FieldKind::Guid => match parse_guid(t) {
            Some(b) => Cell::Guid(b),
            None => return bad("a Guid"),
        },
        FieldKind::DateFormula => match date_formula_stored(t) {
            Some(s) => Cell::Text(s),
            None => return bad("a date formula"),
        },
        // A Blob or Media is not imported: its content is left blank.
        FieldKind::Binary => Cell::Null,
        FieldKind::Other => return Err(format!("a value of a type this reader does not rebuild ('{t}')")),
    })
}

// ---- the provider --------------------------------------------------------------------------------------------------

/// A package as the test data of a run.
pub struct PackageProvider {
    package: Package,
    company: String,
    source: String,
    identity: String,
}

impl PackageProvider {
    /// `bytes` are the file; `source` its name for the summary; `company` the name AL code sees as `CompanyName()`.
    pub fn from_bytes(bytes: &[u8], source: &str, company: &str) -> Result<PackageProvider, String> {
        let package = Package::parse(bytes)?;
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for b in bytes {
            h ^= u64::from(*b);
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
        let identity = format!(
            "rapidstart:{}:{}:{}:{h:016x}",
            package.code,
            package.product_version,
            bytes.len()
        );
        Ok(PackageProvider {
            package,
            company: company.to_string(),
            source: source.to_string(),
            identity,
        })
    }

    pub fn package(&self) -> &Package {
        &self.package
    }
}

impl TableProvider for PackageProvider {
    fn company(&self) -> &str {
        &self.company
    }

    fn source(&self) -> String {
        self.source.clone()
    }

    fn wants_fields(&self) -> bool {
        true
    }

    fn identity(&self) -> Option<String> {
        Some(self.identity.clone())
    }

    fn read(&self, req: &TableRequest) -> Result<Arc<TableRows>, TableError> {
        let Some(id) = req.table_id else {
            return Err(TableError::Reader("a configuration package is read by table id".into()));
        };
        let Some(table) = self.package.table(id) else {
            return Err(TableError::Absent);
        };
        if table.rows.is_empty() {
            return Err(TableError::Absent);
        }
        // Field of each package column, by the package's name for it. Two fields with one package name: the lowest number wins.
        let mut by_name: HashMap<String, &RequestField> = HashMap::new();
        let mut fields: Vec<&RequestField> = req.fields.iter().collect();
        fields.sort_by_key(|f| f.id);
        for f in fields {
            by_name.entry(element_name(&f.name).to_lowercase()).or_insert(f);
        }
        let mut columns = Vec::new();
        let mut kinds = Vec::new();
        let mut keep = Vec::new();
        for (i, c) in table.columns.iter().enumerate() {
            if let Some(f) = by_name.get(&c.to_lowercase()) {
                if !columns.contains(&f.column) {
                    columns.push(f.column.clone());
                    kinds.push(f.kind);
                    keep.push(i);
                }
            }
        }
        let mut rows = Vec::with_capacity(table.rows.len());
        for r in &table.rows {
            let mut out = Vec::with_capacity(keep.len());
            for (k, &i) in keep.iter().enumerate() {
                let text = r.get(i).map_or("", String::as_str);
                out.push(
                    convert(kinds[k], text)
                        .map_err(|why| TableError::Reader(format!("column '{}': {why}", table.columns[i])))?,
                );
            }
            rows.push(out);
        }
        Ok(Arc::new(TableRows { columns, rows }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const XML: &str = r#"<?xml version="1.0" encoding="UTF-16" standalone="yes"?>
<DataList Code="T.ENU" ProductVersion="NAV27.5">
  <PaymentTermsList>
    <TableID>3</TableID>
    <PageID>4</PageID>
    <SkipTableTriggers>1</SkipTableTriggers>
    <PaymentTerms>
      <Code PrimaryKey="1" ProcessingOrder="2">10 DAYS</Code>
      <DueDateCalculation ProcessingOrder="1">&lt;10D&gt;</DueDateCalculation>
      <Discount ProcessingOrder="3">1.5</Discount>
      <LastModifiedDateTime ProcessingOrder="4">2026-04-09T16:52:05.237Z</LastModifiedDateTime>
    </PaymentTerms>
    <PaymentTerms>
      <Code>14 DAYS</Code>
      <DueDateCalculation>&lt;1M+CM&gt;</DueDateCalculation>
      <Discount>
      </Discount>
      <LastModifiedDateTime/>
    </PaymentTerms>
  </PaymentTermsList>
  <EmptyList>
    <TableID>5</TableID>
  </EmptyList>
  <NoSkipList>
    <TableID>7</TableID>
    <Thing><A ProcessingOrder="1">x</A></Thing>
  </NoSkipList>
</DataList>"#;

    #[test]
    fn a_package_is_read_in_processing_order() {
        let p = Package::parse_xml(XML).unwrap();
        assert_eq!((p.code.as_str(), p.product_version.as_str()), ("T.ENU", "NAV27.5"));
        assert_eq!(p.tables.len(), 3);
        let t = p.table(3).unwrap();
        assert!(t.skip_table_triggers);
        assert_eq!(
            t.columns,
            ["DueDateCalculation", "Code", "Discount", "LastModifiedDateTime"]
        );
        assert_eq!(t.primary_key, [false, true, false, false]);
        assert_eq!(t.rows[0], ["<10D>", "10 DAYS", "1.5", "2026-04-09T16:52:05.237Z"]);
        assert_eq!(t.rows[1][..2], ["<1M+CM>", "14 DAYS"]);
        assert!(t.rows[1][2].trim().is_empty() && t.rows[1][3].is_empty());
        assert!(p.table(5).unwrap().rows.is_empty());
        assert_eq!(p.triggered_tables(), vec![7]);
    }

    #[test]
    fn gzip_utf16_bytes_are_read() {
        use std::io::Write;
        let mut utf16: Vec<u8> = vec![0xff, 0xfe];
        for u in XML.encode_utf16() {
            utf16.extend_from_slice(&u.to_le_bytes());
        }
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        gz.write_all(&utf16).unwrap();
        let bytes = gz.finish().unwrap();
        let p = Package::parse(&bytes).unwrap();
        assert_eq!(p.table(3).unwrap().rows.len(), 2);
        assert!(Package::parse(b"hello").is_err());
    }

    #[test]
    fn values_become_the_cells_of_their_field_kind() {
        assert_eq!(convert(FieldKind::Date, "0001-01-01"), Ok(Cell::Date { days: 0 }));
        assert_eq!(convert(FieldKind::Date, "1970-01-01"), Ok(Cell::Date { days: 719_162 }));
        assert_eq!(
            convert(FieldKind::DateTime, "2026-04-09T16:52:05.237Z"),
            Ok(Cell::DateTime2 {
                days: days_from_civil(2026, 4, 9),
                units: 60_725_237,
                scale: 3
            })
        );
        assert_eq!(
            convert(FieldKind::Time, "01:02:03"),
            Ok(Cell::Time {
                units: 3_723_000,
                scale: 3
            })
        );
        assert_eq!(
            convert(FieldKind::Decimal, "-0.015"),
            Ok(Cell::Decimal {
                negative: true,
                magnitude: 15,
                scale: 3
            })
        );
        assert_eq!(convert(FieldKind::Boolean, "1"), Ok(Cell::Bool(true)));
        assert_eq!(convert(FieldKind::Ordinal, " 3 "), Ok(Cell::Int(3)));
        assert_eq!(convert(FieldKind::Date, "\n   "), Ok(Cell::Null));
        assert_eq!(convert(FieldKind::Code, "\n   "), Ok(Cell::Text(String::new())));
        assert_eq!(convert(FieldKind::Text, "a b"), Ok(Cell::Text("a b".into())));
        assert_eq!(convert(FieldKind::Binary, "AAAA"), Ok(Cell::Null));
        assert!(convert(FieldKind::Integer, "x").is_err());
        assert!(convert(FieldKind::Other, "x").is_err());
        assert_eq!(
            convert(FieldKind::DateFormula, "<10D>"),
            Ok(Cell::Text("10\u{2}".into()))
        );
        assert_eq!(
            convert(FieldKind::DateFormula, "<1M+CM>"),
            Ok(Cell::Text("1\u{5}+\u{1}\u{5}".into()))
        );
        assert_eq!(
            convert(FieldKind::DateFormula, "<-WD1+2W>"),
            Ok(Cell::Text("-\u{3}1+2\u{4}".into()))
        );
        // The text order AB12CD34-EF56-... is stored in .NET byte order.
        assert_eq!(
            convert(FieldKind::Guid, "{AB12CD34-EF56-4A78-9B01-23456789ABCD}"),
            Ok(Cell::Guid([
                0x34, 0xcd, 0x12, 0xab, 0x56, 0xef, 0x78, 0x4a, 0x9b, 0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd
            ]))
        );
    }

    #[test]
    fn element_names_turn_a_hyphen_into_an_underscore_and_drop_other_punctuation() {
        assert_eq!(element_name("Unit-Amount Rounding Precision"), "Unit_AmountRoundingPrecision");
        assert_eq!(element_name("Bill-to Name"), "Bill_toName");
        assert_eq!(element_name("Asm.-to-Order Shpt. Bin Code"), "Asm_to_OrderShptBinCode");
        assert_eq!(element_name("Country/Region Code"), "CountryRegionCode");
    }

    #[test]
    fn element_names_drop_everything_but_letters_and_digits() {
        assert_eq!(element_name("Due Date Calculation"), "DueDateCalculation");
        assert_eq!(element_name("No."), "No");
        assert_eq!(element_name("Amount (LCY)"), "AmountLCY");
    }

    fn field(id: u32, name: &str, kind: FieldKind) -> RequestField {
        RequestField {
            id,
            name: name.into(),
            column: name.replace('.', "_"),
            kind,
        }
    }

    #[test]
    fn the_provider_types_the_columns_by_the_requested_fields() {
        let p = PackageProvider::from_bytes(XML.as_bytes(), "t.rapidstart", "CRONUS International Ltd.").unwrap();
        assert!(p.wants_fields());
        assert_eq!(p.company(), "CRONUS International Ltd.");
        let req = TableRequest {
            name: "Payment Terms".into(),
            table_id: Some(3),
            fields: vec![
                field(1, "Code", FieldKind::Code),
                field(2, "Due Date Calculation", FieldKind::DateFormula),
                field(5, "Discount %", FieldKind::Decimal),
                field(9, "Last Modified DateTime", FieldKind::DateTime),
                field(10, "Not In The Package", FieldKind::Text),
            ],
            ..Default::default()
        };
        let rows = p.read(&req).unwrap();
        assert_eq!(
            rows.columns,
            ["Due Date Calculation", "Code", "Discount %", "Last Modified DateTime"]
        );
        assert_eq!(rows.rows.len(), 2);
        assert_eq!(rows.rows[0][0], Cell::Text("10\u{2}".into()));
        assert_eq!(rows.rows[0][1], Cell::Text("10 DAYS".into()));
        assert_eq!(rows.rows[1][2], Cell::Null);
        assert_eq!(rows.rows[1][3], Cell::Null);
        assert_eq!(
            p.read(&TableRequest {
                table_id: Some(99),
                ..req.clone()
            }),
            Err(TableError::Absent)
        );
        assert_eq!(
            p.read(&TableRequest {
                table_id: Some(5),
                ..req.clone()
            }),
            Err(TableError::Absent)
        );
        // A value that cannot be typed refuses the table with the column named.
        let bad = TableRequest {
            fields: vec![field(1, "Code", FieldKind::Integer)],
            ..req
        };
        match p.read(&bad) {
            Err(TableError::Reader(why)) => assert!(why.contains("'Code'") && why.contains("whole number"), "{why}"),
            other => panic!("{other:?}"),
        }
    }
}
