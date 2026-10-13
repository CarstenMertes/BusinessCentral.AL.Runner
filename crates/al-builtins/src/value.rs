//! Runtime values (ADR-010).

use rust_decimal::Decimal;
use std::cell::RefCell;
use std::rc::Rc;

/// Runtime type of a slot: what a variable, field or parameter holds.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum RtType {
    Boolean,
    Integer,
    BigInteger,
    Decimal,
    Byte,
    Char,
    Text(Option<u32>),
    Code(Option<u32>),
    Date,
    Time,
    DateTime,
    Duration,
    Guid,
    DateFormula,
    Option(Vec<String>),
    Enum(u32),
    Variant,
    List(Box<RtType>),
    Dictionary(Box<RtType>, Box<RtType>),
    Array(Vec<u32>, Box<RtType>),
    /// Record of the table with this object index; temporary flag.
    Record(u32, bool),
    /// Codeunit with this object index.
    Codeunit(u32),
    /// Library types whose values are opaque handles (`JsonObject`, `TextBuilder`, ...).
    Library(String),
}

#[derive(Debug, Clone)]
pub enum Value {
    /// An unassigned Variant.
    Null,
    Boolean(bool),
    Integer(i32),
    BigInteger(i64),
    Decimal(Decimal),
    Byte(u8),
    Char(u16),
    Text(String),
    Code(String),
    /// Day number (0 = undefined `0D`), closing-date flag.
    Date(i32, bool),
    /// Milliseconds since midnight plus one (0 = undefined `0T`).
    Time(i64),
    /// Milliseconds since 0001-01-01 UTC plus one (0 = undefined `0DT`).
    DateTime(i64),
    /// Milliseconds.
    Duration(i64),
    Guid([u8; 16]),
    DateFormula(String),
    /// Ordinal and the option's member names (needed by Format).
    Option(i32, Rc<[String]>),
    Enum(u32, i32),
    List(Rc<RefCell<Vec<Value>>>),
    Dictionary(Rc<RefCell<Vec<(Value, Value)>>>),
    Array(Rc<RefCell<Vec<Value>>>),
    Codeunit(Rc<RefCell<Instance>>),
    Library(String, Rc<RefCell<LibraryValue>>),
    /// A record variable (ADR-010: a handle; `:=` copies fields, see the VM).
    Record(Rc<RefCell<RecordState>>),
}

/// Everything a record variable carries.
#[derive(Debug, Clone)]
pub struct RecordState {
    /// Object index of the table.
    pub table: u32,
    pub temporary: bool,
    /// Identity of the private table behind a temporary record (shared by `Copy(x, true)`).
    pub temp_id: Option<u64>,
    /// Field values in the table's field order.
    pub fields: Vec<Value>,
    pub filters: Vec<crate::filter::FieldFilter>,
    /// Index of the current key in the table's keys.
    pub key: usize,
    pub ascending: bool,
    /// Position of the last row found (key fields then primary key), for `Next`.
    pub position: Option<Vec<Value>>,
}

/// The global variables of one codeunit instance.
#[derive(Debug, Default)]
pub struct Instance {
    pub object: u32,
    pub globals: Vec<Value>,
    pub initialized: bool,
}

/// State behind a library-type handle; grows with the builtins that need it.
#[derive(Debug, Default)]
pub enum LibraryValue {
    #[default]
    Empty,
    Text(String),
    /// A JSON node behind a JsonObject / JsonArray / JsonToken / JsonValue handle.
    Json(crate::json::JsonCell),
}

impl Value {
    pub fn default_for(t: &RtType) -> Value {
        match t {
            RtType::Boolean => Value::Boolean(false),
            RtType::Integer => Value::Integer(0),
            RtType::BigInteger => Value::BigInteger(0),
            RtType::Decimal => Value::Decimal(Decimal::ZERO),
            RtType::Byte => Value::Byte(0),
            RtType::Char => Value::Char(0),
            RtType::Text(_) => Value::Text(String::new()),
            RtType::Code(_) => Value::Code(String::new()),
            RtType::Date => Value::Date(0, false),
            RtType::Time => Value::Time(0),
            RtType::DateTime => Value::DateTime(0),
            RtType::Duration => Value::Duration(0),
            RtType::Guid => Value::Guid([0; 16]),
            RtType::DateFormula => Value::DateFormula(String::new()),
            RtType::Option(m) => Value::Option(0, m.clone().into()),
            RtType::Enum(id) => Value::Enum(*id, 0),
            RtType::Variant => Value::Null,
            RtType::List(_) => Value::List(Rc::default()),
            RtType::Dictionary(..) => Value::Dictionary(Rc::default()),
            RtType::Array(dims, el) => {
                let n = dims.iter().map(|d| *d as usize).product();
                Value::Array(Rc::new(RefCell::new((0..n).map(|_| Value::default_for(el)).collect())))
            }
            RtType::Codeunit(id) => Value::Codeunit(Rc::new(RefCell::new(Instance {
                object: *id,
                ..Default::default()
            }))),
            // Records need the table's field types: the VM builds them.
            RtType::Record(..) => Value::Null,
            RtType::Library(name) => Value::Library(name.clone(), Rc::default()),
        }
    }

    /// Length in UTF-16 code units, as AL's StrLen and MaxStrLen count it.
    pub fn text_len(s: &str) -> usize {
        s.encode_utf16().count()
    }

    /// AL's dynamic type tests (`Variant.IsInteger`, ...).
    pub fn is(&self, kind: &str) -> bool {
        let k = kind.to_ascii_lowercase();
        match self {
            Value::Null => false,
            Value::Boolean(_) => k == "boolean",
            Value::Integer(_) => k == "integer",
            Value::BigInteger(_) => k == "biginteger",
            Value::Decimal(_) => k == "decimal",
            Value::Byte(_) => k == "byte",
            Value::Char(_) => k == "char",
            Value::Text(_) => k == "text",
            Value::Code(_) => k == "code",
            Value::Date(..) => k == "date",
            Value::Time(_) => k == "time",
            Value::DateTime(_) => k == "datetime",
            Value::Duration(_) => k == "duration",
            Value::Guid(_) => k == "guid",
            Value::DateFormula(_) => k == "dateformula",
            // An enum in a Variant reports IsOption: corpus types/TestVariantExtended.al:261.
            Value::Option(..) | Value::Enum(..) => k == "option",
            Value::List(_) => k == "list",
            Value::Dictionary(_) => k == "dictionary",
            Value::Array(_) => false,
            Value::Codeunit(_) => k == "codeunit",
            Value::Library(n, _) => n.eq_ignore_ascii_case(&k),
            Value::Record(_) => k == "record",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults() {
        assert!(matches!(Value::default_for(&RtType::Integer), Value::Integer(0)));
        assert!(matches!(Value::default_for(&RtType::Boolean), Value::Boolean(false)));
        assert!(matches!(Value::default_for(&RtType::Text(Some(10))), Value::Text(ref s) if s.is_empty()));
        assert!(matches!(Value::default_for(&RtType::Code(None)), Value::Code(ref s) if s.is_empty()));
        assert!(matches!(Value::default_for(&RtType::Date), Value::Date(0, false)));
        assert!(matches!(Value::default_for(&RtType::Decimal), Value::Decimal(d) if d.is_zero()));
        assert!(matches!(Value::default_for(&RtType::Variant), Value::Null));
        assert!(matches!(
            Value::default_for(&RtType::Option(vec!["A".into()])),
            Value::Option(0, _)
        ));
        assert!(matches!(Value::default_for(&RtType::Guid), Value::Guid(g) if g == [0; 16]));
    }

    #[test]
    fn arrays_default_to_their_element_default_per_cell() {
        let Value::Array(a) = Value::default_for(&RtType::Array(vec![2, 3], Box::new(RtType::Integer))) else {
            panic!()
        };
        assert_eq!(a.borrow().len(), 6);
        assert!(matches!(a.borrow()[5], Value::Integer(0)));
    }

    #[test]
    fn lists_share_on_clone_text_copies() {
        let l = Value::default_for(&RtType::List(Box::new(RtType::Integer)));
        let l2 = l.clone();
        if let Value::List(v) = &l {
            v.borrow_mut().push(Value::Integer(1));
        }
        let Value::List(v2) = &l2 else { panic!() };
        assert_eq!(v2.borrow().len(), 1, "a List is a reference type");
        let t = Value::Text("a".into());
        let mut t2 = t.clone();
        if let Value::Text(s) = &mut t2 {
            s.push('b');
        }
        assert!(matches!(t, Value::Text(ref s) if s == "a"), "Text is a value type");
    }

    #[test]
    fn each_default_list_is_a_fresh_handle() {
        let a = Value::default_for(&RtType::List(Box::new(RtType::Integer)));
        let b = Value::default_for(&RtType::List(Box::new(RtType::Integer)));
        let (Value::List(a), Value::List(b)) = (a, b) else {
            panic!()
        };
        assert!(!Rc::ptr_eq(&a, &b));
    }

    #[test]
    fn utf16_length() {
        assert_eq!(Value::text_len("abc"), 3);
        assert_eq!(Value::text_len("ä"), 1);
        assert_eq!(Value::text_len("😀"), 2, "a surrogate pair counts as two");
    }

    #[test]
    fn dynamic_type_tests() {
        assert!(Value::Integer(1).is("Integer"));
        assert!(!Value::Integer(1).is("Decimal"));
        assert!(Value::Code("A".into()).is("code"));
        assert!(!Value::Code("A".into()).is("Text"));
        assert!(Value::Option(1, Rc::from(vec![])).is("Option"));
        assert!(Value::Enum(1, 0).is("Option"));
        assert!(!Value::Null.is("Integer"));
    }
}
