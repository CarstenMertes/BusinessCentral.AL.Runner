//! Runtime values (ADR-010).

use rust_decimal::Decimal;
use std::cell::RefCell;
use std::rc::Rc;

/// The members of the system option `Action`, in docs order (none of the corpus reads an
/// ordinal: docs/probes-needed.md).
pub const ACTION_MEMBERS: [&str; 9] = [
    "None",
    "OK",
    "Cancel",
    "LookupOK",
    "LookupCancel",
    "Yes",
    "No",
    "RunObject",
    "RunSystem",
];

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
    /// `TestPage` of the page with this object index (ADR-012).
    TestPage(u32),
    /// A `Page` variable of the page with this object index: a page instance that exists before
    /// it is run (ADR-012).
    Page(u32),
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
    /// A Blob field's content: a shared buffer that streams write to (see `stream`).
    Blob(crate::stream::BlobRef),
    /// A `TestPage` (or `TestPart`): a handle on a page and the test client
    /// driving it (ADR-012). Shared like a codeunit instance.
    TestPage(Rc<RefCell<PageState>>),
    /// A control of a test page (`TestField`, `TestAction`, `TestFilter`).
    TestControl(Rc<ControlRef>),
}

/// How a page was opened by the test client.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageMode {
    View,
    Edit,
    New,
}

/// Where the test client's cursor stands on a page (ADR-012).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cursor {
    /// No row: an empty view.
    None,
    /// On a stored row (or, for a new record, the row being typed).
    Row,
    /// On the blank line an editable, insert-allowed repeater carries past its data
    /// (corpus handlers/TestPageNewRowLine_Tests.al:1-30).
    NewLine,
}

/// The test client's state for one page (ADR-012). Fields grow with the
/// behaviours the corpus pins.
pub struct PageState {
    /// Object index of the page.
    pub object: u32,
    /// Open pages answer; closed ones, and ones torn down by an error in a row load,
    /// raise "The TestPage is not open." (corpus testpageteardown/TestPageTeardown.al:3-24).
    pub open: bool,
    pub mode: PageMode,
    /// The running page: its globals hold `Rec`, `xRec` and `CurrPage`.
    pub instance: Option<Rc<RefCell<Instance>>>,
    pub cursor: Cursor,
    /// The page offers the trailing new-row line.
    pub new_line: bool,
    /// The cursor stands on a row that is not stored yet (OpenNew, New).
    pub draft: bool,
    /// Fields written by the client since the row was loaded.
    pub dirty: bool,
    /// `Visible` of each control as it was when the page opened (the property is frozen then,
    /// corpus handlers/TestPageControlPropertyLiveness_Tests.al:56).
    pub frozen_visible: Vec<bool>,
    /// Primary keys of the rows a repeater page has populated (its window).
    pub window: Vec<Vec<Value>>,
    /// The control that has the client's focus, and whether it is on the current row. Moving
    /// focus to a non-key control of a new row is what inserts it (corpus
    /// handlers/TestPageBlankKeyInsert.al:1-25).
    pub focus: Option<usize>,
    pub focus_in_row: bool,
    /// Validation errors the client holds: (control index, stored text).
    pub errors: Vec<(usize, String)>,
    /// The started line's insert was refused and the client moved on without raising
    /// (corpus handlers/TestPageFailedInsert_Tests.al:1-30): `refused` keeps the line, `pending`
    /// is the line left behind by Previous()/First(), whose insert fails again at Close().
    pub refused: bool,
    pub pending: Option<Vec<Value>>,
    /// Opened by AL code (`Page.RunModal`, `Page.Run`) and answered by a handler, not by the test.
    pub modal: bool,
    pub lookup: bool,
    /// The `Action` the page closed with, once it has.
    pub result: Option<String>,
    /// `TestPage.Trap()` was called on this variable and no page has been caught yet.
    pub trapping: bool,
    /// `CurrPage.Update` was called: the client re-reads the row once the call that made it returns.
    pub refresh: bool,
    /// A row with the new record's starting key was already stored when the draft started.
    pub draft_stored: bool,
    /// OnInit/OnOpenPage are running, and `CurrPage.Editable(false)` has been called in them.
    pub opening: bool,
    pub editable_off: bool,
}

// The page instance holds `CurrPage`, which points back here: print only the facts.
impl std::fmt::Debug for PageState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "PageState(page {}, open {}, mode {:?})",
            self.object, self.open, self.mode
        )
    }
}

impl PageState {
    pub fn closed(object: u32) -> PageState {
        PageState {
            object,
            open: false,
            mode: PageMode::View,
            instance: None,
            cursor: Cursor::None,
            new_line: false,
            draft: false,
            dirty: false,
            frozen_visible: Vec::new(),
            window: Vec::new(),
            focus: None,
            focus_in_row: false,
            errors: Vec::new(),
            refused: false,
            pending: None,
            modal: false,
            lookup: false,
            result: None,
            trapping: false,
            refresh: false,
            draft_stored: false,
            opening: false,
            editable_off: false,
        }
    }
}

/// What a `TestControl` stands for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControlTarget {
    /// A control of the page by index into its control list.
    Control(usize),
    /// A built-in action: OK, Cancel, Yes, No, Edit, View.
    BuiltIn(String),
    /// `TestPage.Filter`.
    Filter,
}

#[derive(Debug)]
pub struct ControlRef {
    pub page: Rc<RefCell<PageState>>,
    pub target: ControlTarget,
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
    /// The filter group `SetRange` and `SetFilter` write to and `GetFilter` reads.
    pub filter_group: i32,
    /// Primary keys marked with `Mark(true)`, and whether `MarkedOnly` restricts the set to them.
    pub marks: Vec<Vec<Value>>,
    pub marked_only: bool,
    /// `ReadIsolation` and `SecurityFiltering` as member indexes of their platform option types.
    pub read_isolation: i32,
    pub security_filtering: i32,
    /// Index of the current key in the table's keys.
    pub key: usize,
    pub ascending: bool,
    /// `SetAscending(Field, false)` on the single field of the current key. Unlike `ascending` it
    /// does not survive a page's view round trip (handlers/TestPageViewRoundTripRoutes.al).
    pub field_descending: bool,
    /// Position of the last row found (key fields then primary key), for `Next`.
    pub position: Option<Vec<Value>>,
    /// Field positions of `SetAutoCalcFields`: calculated whenever a row is read.
    pub auto_calc: Vec<usize>,
}

/// What a `WebServiceActionContext` holds: the object an API action points the OData layer at.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WebServiceActionState {
    pub object_id: i32,
    /// Ordinal in the platform's object type option; 0 is the unnamed default.
    pub object_type: i32,
    /// Index into the `WebServiceActionResultCode` members; 0 is `None`.
    pub result_code: i32,
    /// Field ids that have an entity key.
    pub keys: Vec<i32>,
}

/// What a `SessionSettings` holds. A property nobody wrote is `None`: the corpus does not say what
/// a pristine language id, locale id, time zone or profile id reads as, so reading one is refused.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionSettingsState {
    pub language_id: Option<i32>,
    pub locale_id: Option<i32>,
    pub time_zone: Option<String>,
    pub profile_id: Option<String>,
    pub profile_app_id: Option<[u8; 16]>,
    pub company: String,
}

/// What a `Notification` holds.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NotificationState {
    pub message: Option<String>,
    /// Ordinal in the `NotificationScope` members.
    pub scope: Option<i32>,
    pub data: Vec<(String, String)>,
    /// (Caption, codeunit id, method name, description): kept, never run.
    pub actions: Vec<(String, i32, String, Option<String>)>,
}

/// What an `ErrorInfo` holds (corpus errorinfo/TestErrorInfoType.al).
#[derive(Debug, Clone)]
pub struct ErrorInfoState {
    pub message: String,
    pub title: String,
    pub detailed_message: String,
    pub control_name: String,
    /// Indexes into the `Verbosity` and `DataClassification` members.
    pub verbosity: i32,
    pub data_classification: i32,
    /// `None` until a test sets one: the default is not pinned.
    pub error_type: Option<i32>,
    pub collectible: bool,
    pub table_id: i32,
    /// The `RecordId` value of the record `Create` was given.
    pub record_id: Option<Value>,
    pub field_no: i32,
    pub page_no: i32,
    pub custom_dimensions: Vec<(String, String)>,
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
    /// An open `RecordRef`: it owns a record buffer exactly as a `Record`
    /// variable does (fields, filters, key), so record operations are shared.
    RecordRef(Rc<RefCell<RecordState>>),
    /// A `FieldRef`: the record buffer it points into and the field's position.
    FieldRef(Rc<RefCell<RecordState>>, usize),
    /// A `KeyRef`: the record buffer and the key's index in the table's keys.
    KeyRef(Rc<RefCell<RecordState>>, usize),
    /// A `RecordRef` opened on a system table the workspace has no schema for:
    /// only its number is known (corpus recordref/TestRecordRefOpenCompilationTarget.al).
    SystemTable(i64),
    /// A `RecordId`: table object index and number, table name (for `Format`), primary key values.
    RecordId {
        table: u32,
        table_no: u32,
        name: String,
        key: Vec<Value>,
    },
    /// A `WebServiceActionContext` that has been written to (an untouched one is `Empty`).
    WebServiceAction(WebServiceActionState),
    /// A `SessionSettings` that has been written to (an untouched one is `Empty`).
    SessionSettings(SessionSettingsState),
    /// A `Notification` that has been written to (an untouched one is `Empty`).
    Notification(NotificationState),
    /// A `Version`: major, minor, build, revision.
    Version([i32; 4]),
    /// An `ErrorInfo` (the zero-argument `Create` and `Create(Message, ...)` build one).
    ErrorInfo(ErrorInfoState),
    /// A JSON node behind a JsonObject / JsonArray / JsonToken / JsonValue handle.
    Json(crate::json::JsonCell),
    /// A node behind an Xml* handle.
    Xml(crate::xml::XmlCell),
    /// An XmlNodeList: a snapshot of nodes.
    XmlList(Vec<crate::json::Handle>),
    /// An XmlAttributeCollection: a view of one element's attributes.
    XmlAttrs(crate::json::Handle),
    /// An XmlNameTable (opaque).
    XmlNameTable,
    /// An InStream: a snapshot being read.
    InStream(crate::stream::InState),
    /// An OutStream: appends to a Blob buffer.
    OutStream(crate::stream::BlobRef, crate::stream::Encoding),
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
            RtType::TestPage(id) | RtType::Page(id) => Value::TestPage(Rc::new(RefCell::new(PageState::closed(*id)))),
            // Records need the table's field types: the VM builds them.
            RtType::Record(..) => Value::Null,
            RtType::Library(name) if name.eq_ignore_ascii_case("Blob") => Value::Blob(crate::stream::new_blob()),
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
            Value::TestPage(_) | Value::TestControl(_) => false,
            Value::Blob(_) => false,
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
