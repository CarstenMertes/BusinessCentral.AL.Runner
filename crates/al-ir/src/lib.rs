//! Typed IR (ADR-011): every name resolved, every node typed, conversions
//! explicit. Plain data, serde-serialisable, no syntax nodes.

use al_builtins::value::RtType;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Program {
    pub objects: Vec<Object>,
    /// The apps the objects belong to, in the order the compiler was given them (`Object::app`
    /// indexes this). Publisher and version come from the runner, which reads the manifests.
    #[serde(default)]
    pub apps: Vec<AppInfo>,
}

/// What `NavApp.GetCurrentModuleInfo` reports about an app.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct AppInfo {
    pub id: String,
    pub name: String,
    pub publisher: String,
    /// `1.0.0.0`.
    pub version: String,
    /// Ids of the apps it depends on.
    pub dependencies: Vec<String>,
    /// The packaged resources (`resourceFolders` of the manifest): name relative to its folder, content.
    #[serde(default)]
    pub resources: Vec<(String, Vec<u8>)>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Object {
    /// `codeunit`, `table`, ...
    pub kind: String,
    pub id: Option<u32>,
    pub name: String,
    pub globals: Vec<Slot>,
    pub routines: Vec<Routine>,
    /// `Subtype = Test` and similar, as written.
    pub properties: Vec<(String, String)>,
    /// For enums: (declared value, name), extensions included.
    pub enum_values: Vec<(i32, String)>,
    /// For enums: the `Caption` of values that declare one, by ordinal.
    #[serde(default)]
    pub enum_captions: Vec<(i32, String)>,
    /// For tables: fields (extensions and system fields included), keys as
    /// field positions (`keys[0]` primary), `DataPerCompany`.
    pub table: Option<Table>,
    /// Global slots of the implicit `Rec` / `xRec` (tables, table extensions).
    pub rec_slot: Option<usize>,
    pub xrec_slot: Option<usize>,
    /// For pages: the control tree and page triggers (ADR-012).
    pub page: Option<Page>,
    /// Index of the app that owns the object, in the order the apps were given to the compiler.
    /// Builtins scoped to a module (`IsolatedStorage`) key on it.
    #[serde(default)]
    pub app: u32,
    /// For queries: dataitems, columns and the query-level properties.
    #[serde(default)]
    pub query: Option<Query>,
    /// For xmlports: the schema. Triggers and procedures are ordinary routines of the object.
    #[serde(default)]
    pub xmlport: Option<XmlPort>,
    /// For reports: the data items and report-level triggers. The triggers and procedures are
    /// ordinary routines of the object.
    #[serde(default)]
    pub report: Option<Report>,
    /// For reportextensions that change what a run does: the triggers and data items they add.
    #[serde(default)]
    pub report_ext: Option<ReportExt>,
}

/// A report as the runtime sees it: processing only, with no layout.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Report {
    /// `ProcessingOnly = true`.
    pub processing_only: bool,
    /// `UseRequestPage` when the report declares it (it defaults to true).
    pub use_request_page: Option<bool>,
    /// The report declares a `requestpage` section.
    pub has_request_page: bool,
    /// `TransactionType`, lower-case, when the report declares it.
    pub transaction_type: Option<String>,
    /// Data items in source order; `parent` makes the tree. Top-level items have no parent.
    pub items: Vec<ReportItem>,
    /// `OnInitReport`, `OnPreReport` and `OnPostReport` as routine indices.
    pub on_init: Option<usize>,
    pub on_pre: Option<usize>,
    pub on_post: Option<usize>,
    /// The one reportextension (object index) that changes what a run does, if any.
    #[serde(default)]
    pub extension: Option<usize>,
    /// Global slot of the hidden flag `Report.UseRequestPage(x)` sets: 0 = not set, 1 = true, 2 = false.
    pub request_flag_slot: usize,
    /// Why the runtime cannot run this report yet; empty = it can.
    pub unsupported: Vec<String>,
}

/// What a reportextension adds to a run of the report it extends.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReportExt {
    /// Object index of the extended report.
    pub base: usize,
    /// `OnPreReport` and `OnPostReport` of the extension, as routine indices.
    pub on_pre: Option<usize>,
    pub on_post: Option<usize>,
    /// (base item index, global slot of the extension that names that item's record).
    pub aliases: Vec<(usize, usize)>,
    /// `modify(Item)` blocks.
    pub modifies: Vec<ReportExtModify>,
    /// Data items the extension adds with `addlast`.
    pub added: Vec<AddedItem>,
}

/// The triggers of a `modify(Item)` block of a reportextension.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReportExtModify {
    /// Index of the base item.
    pub item: usize,
    /// `OnBeforeAfterGetRecord` and `OnAfterAfterGetRecord` as routine indices.
    pub before: Option<usize>,
    pub after: Option<usize>,
}

/// A data item added by `addlast`: under a base item, or (`item.parent`) under another added item.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AddedItem {
    /// `item.parent` indexes `ReportExt::added`.
    pub item: ReportItem,
    /// The base item it is added under (top-level added items).
    pub base_parent: Option<usize>,
}

/// A `dataitem` of a report.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReportItem {
    pub name: String,
    pub parent: Option<usize>,
    /// Global slot of the item's record, and the object index of its table.
    pub slot: Option<usize>,
    pub table: Option<usize>,
    /// `DataItemTableView` as written (`sorting(..) where(..)`).
    pub view: Option<String>,
    /// `DataItemLink` pairs (own field position, parent item field position).
    pub link: Vec<(usize, usize)>,
    /// `OnPreDataItem`, `OnAfterGetRecord` and `OnPostDataItem` as routine indices.
    pub on_pre: Option<usize>,
    pub on_after_get: Option<usize>,
    pub on_post: Option<usize>,
}

/// The direction an xmlport declares (`Direction`, default `Both`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum XmlDirection {
    Import,
    Export,
    Both,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum XmlNodeKind {
    /// `textelement`: a container when it has children, else it carries a variable's text.
    Text,
    /// `textattribute`: an attribute carrying a variable's text.
    TextAttr,
    /// `tableelement`: one element per record of a table.
    Table,
    /// `fieldelement`: an element carrying a field of the enclosing table element's record.
    Field,
    /// `fieldattribute`: an attribute carrying such a field.
    FieldAttr,
}

/// An xmlport as the runtime sees it (`Format = Xml`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct XmlPort {
    pub direction: XmlDirection,
    /// Schema nodes in source order; `parent` makes the tree. Top-level nodes have no parent.
    pub nodes: Vec<XmlNode>,
    /// Global slots of the source `InStream` and destination `OutStream` set by
    /// `SetSource` / `SetDestination`.
    pub source_slot: usize,
    pub destination_slot: usize,
    /// Why the runtime cannot run this xmlport yet; empty = it can.
    pub unsupported: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct XmlNode {
    /// The name in the schema (the AL variable for text and table nodes).
    pub name: String,
    /// The element or attribute name in the document (`XmlName`, else `name`).
    pub xml_name: String,
    pub kind: XmlNodeKind,
    pub parent: Option<usize>,
    /// Global slot of the Text variable (text nodes) or the record buffer (table nodes).
    pub slot: Option<usize>,
    /// For table nodes: the object index of the table.
    pub table: Option<usize>,
    /// For field nodes: the table node holding the record, and the field's position in the table.
    pub table_node: Option<usize>,
    pub field: Option<usize>,
    /// For nested table nodes: the `LinkTable` node and (own field, link-table field) positions.
    pub link_node: Option<usize>,
    pub link_fields: Vec<(usize, usize)>,
    /// Table node properties (`AutoSave` defaults to true, the others to false, as in AL).
    pub auto_save: bool,
    pub auto_update: bool,
    pub auto_replace: bool,
    pub use_temporary: bool,
    /// Triggers by lower-case name (`onbeforepassvariable`, ...) to routine index.
    pub triggers: Vec<(String, usize)>,
}

impl XmlNode {
    pub fn trigger(&self, name: &str) -> Option<usize> {
        self.triggers.iter().find(|(n, _)| n == name).map(|(_, r)| *r)
    }
}

/// A query object (corpus query/): a chain of dataitems joined by `DataItemLink`, and the
/// columns they project. Execution is the VM's (`al-vm/src/queries.rs`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Query {
    /// Dataitems in source order; each nests in `parent`.
    pub items: Vec<QueryItem>,
    /// `column(...)` and `filter(...)` elements in source order. A query column is identified by
    /// its position here.
    pub columns: Vec<QueryColumn>,
    /// `OrderBy` as (column, ascending).
    pub order_by: Vec<(usize, bool)>,
    /// The `TopNumberOfRows` property; 0 = all rows.
    pub top: i32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QueryItem {
    pub name: String,
    /// Object index of the source table.
    pub table: usize,
    /// The enclosing dataitem (index into `Query::items`); `None` for the root.
    pub parent: Option<usize>,
    /// `SqlJoinType = LeftOuterJoin`, which is also the default of a nested dataitem.
    pub left_outer: bool,
    /// `DataItemLink`: (field position in this table, field position in the parent's table).
    pub link: Vec<(usize, usize)>,
    /// `DataItemTableFilter`: conditions on fields of this table.
    pub table_filter: Vec<CalcCond>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum QueryMethod {
    Sum,
    Count,
    Average,
    Min,
    Max,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QueryColumn {
    pub name: String,
    /// The `Caption` property when it is a plain text literal.
    pub caption: Option<String>,
    /// Index into `Query::items`.
    pub item: usize,
    /// Position of the source field in the dataitem's table; `None` for a `Count` column.
    pub field: Option<usize>,
    /// A `filter(...)` element: it filters its dataitem and is not part of the dataset.
    pub filter_element: bool,
    pub method: Option<QueryMethod>,
    pub reverse_sign: bool,
    /// Type of the value the column reads.
    pub ty: RtType,
    /// `ColumnFilter`: a `const` or `filter` condition on this column.
    pub static_filter: Option<CalcValue>,
}

/// A page object as the test client sees it (ADR-012). The AL code of a page
/// (triggers, procedures, and one synthetic routine per expression-valued
/// property) is ordinary routines of the same object; this is the layout.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Page {
    /// Controls in source order; `parent` makes the tree. `area(...)` sections
    /// are not controls.
    pub controls: Vec<Control>,
    /// Page triggers by lower-case name (`onopenpage`) to routine index.
    pub triggers: Vec<(String, usize)>,
    /// Global slot of `CurrPage`.
    pub curr_page_slot: usize,
    /// Why the test client cannot drive this page yet; empty = it can.
    pub unsupported: Vec<String>,
    /// The page extensions merged into `controls`, in the order they apply: each runs on an
    /// instance of its own that shares the page's `Rec`, `xRec` and `CurrPage`.
    #[serde(default)]
    pub extensions: Vec<usize>,
    /// Page triggers the extensions declare, by lower-case name: (name, extension object, routine).
    /// They run after the page's own trigger of that name.
    #[serde(default)]
    pub ext_triggers: Vec<(String, usize, usize)>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ControlKind {
    Field,
    Group,
    Repeater,
    Action,
    /// `systemaction(OK)` and friends.
    SystemAction,
    ActionGroup,
    Part,
}

/// Where a field control reads and writes its value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Binding {
    None,
    /// A field of the page's source record (position in the table).
    Field(usize),
    /// A global variable of the page.
    Var(usize),
    /// A read-only expression: a routine returning the value.
    Expr(usize),
}

/// A control property: written as a literal, or an expression compiled to a
/// routine of the page.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum PropVal {
    Lit(String),
    /// A routine of the control's owner (the page, or the extension that declares the control).
    Routine(usize),
    /// A routine of an extension that modified the control: (extension object, routine).
    Ext(usize, usize),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Control {
    pub name: String,
    pub kind: ControlKind,
    pub parent: Option<usize>,
    pub binding: Binding,
    /// Properties by lower-case name (`visible`, `caption`, `decimalplaces`, ...).
    pub props: Vec<(String, PropVal)>,
    /// Control triggers by lower-case name (`onvalidate`, `onaction`, ...) to routine index.
    pub triggers: Vec<(String, usize)>,
    /// For parts: the page object index.
    pub part_page: Option<usize>,
    /// For parts: how the part's rows follow the host's current row.
    #[serde(default)]
    pub part_link: Option<PartLink>,
    /// The page extension that declares the control; `None` for the page's own controls. The
    /// routines and global slots in `binding`, `triggers` and `PropVal::Routine` belong to it.
    #[serde(default)]
    pub owner: Option<usize>,
    /// Triggers that `modify` blocks of page extensions add to the control, by lower-case name
    /// (`onbeforevalidate`, `onaftervalidate`, `ondrilldown`, ...): (name, extension object,
    /// routine), in the order the extensions apply.
    #[serde(default)]
    pub ext_triggers: Vec<(String, usize, usize)>,
    /// For actions: what `RunObject` runs (ADR-012 follow-up).
    #[serde(default)]
    pub run_object: Option<RunObject>,
    /// For `actionref(Name; Target)` controls: the action they stand for.
    #[serde(default)]
    pub action_ref: Option<ActionRef>,
}

/// The action an `actionref` delegates to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ActionRef {
    /// Index of the action control on the same page.
    Target(usize),
    /// Why the target is not an action of this page the client can follow.
    Unresolved(String),
}

/// The kind of object a `RunObject` names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RunKind {
    Page,
    Codeunit,
    Report,
    XmlPort,
    Query,
}

/// `RunPageMode` of an action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum RunMode {
    #[default]
    Default,
    View,
    Edit,
    Create,
}

/// The `RunObject` property of an action, with the properties that go with it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunObject {
    pub kind: RunKind,
    /// The page or codeunit object; `None` for a report, xmlport or query (the test client
    /// refuses those before it looks at the target) and for a target that did not resolve.
    pub object: Option<usize>,
    /// `RunPageOnRec = true`.
    pub on_rec: bool,
    pub mode: RunMode,
    /// `RunPageLink` as entries (target field, host field or filter) and `RunPageView` as
    /// written; `provider` is unused.
    pub link: Option<PartLink>,
    /// Why the test client cannot run this action yet; `None` = it can.
    pub unsupported: Option<String>,
}

/// `SubPageLink`, `SubPageView` and `Provider` of a part control (ADR-012 follow-up).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct PartLink {
    /// `SubPageLink` entries, in source order.
    pub entries: Vec<LinkEntry>,
    /// `SubPageView` as written.
    pub view: Option<String>,
    /// `Provider = <part>`: the control index (on the same page) whose current row the link reads.
    pub provider: Option<usize>,
}

/// One `PartField = field(HostField)` / `const(..)` / `filter(..)` entry of a `SubPageLink`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum LinkEntry {
    /// Position of the part's field, position of the host (or provider) field.
    Field(usize, usize),
    /// Position of the part's field, the filter text (`const(x)` and `filter(x)` alike).
    Filter(usize, String),
}

impl Control {
    pub fn prop(&self, name: &str) -> Option<&PropVal> {
        self.props.iter().find(|(n, _)| n == name).map(|(_, v)| v)
    }
    pub fn trigger(&self, name: &str) -> Option<usize> {
        self.triggers.iter().find(|(n, _)| n == name).map(|(_, r)| *r)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Table {
    pub fields: Vec<Field>,
    pub keys: Vec<Vec<usize>>,
    /// `Unique = true` per key (parallel to `keys`).
    pub unique_keys: Vec<bool>,
    pub per_company: bool,
    /// Object index of each table extension (their triggers are not run yet).
    pub extensions: Vec<usize>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Field {
    pub id: u32,
    pub name: String,
    pub ty: RtType,
    /// Normal fields; FlowFields and FlowFilters are not stored.
    pub stored: bool,
    /// `InitValue`, if declared and understood.
    pub init: Option<Const>,
    /// `InitValue` as written; set with `init: None` means not understood yet.
    pub init_text: Option<String>,
    /// Lower-case name of the table a `TableRelation` points to.
    pub relation_table: Option<String>,
    /// Lower-case names of every table the relation names (all arms of a conditional one).
    pub relation_tables: Vec<String>,
    pub auto_increment: bool,
    /// The field declares a `TableRelation`.
    pub relation: bool,
    /// (object, routine) of the field's OnValidate trigger, if any.
    pub on_validate: Option<(usize, usize)>,
    /// (object, routine) of the field's table-side OnLookup trigger, if any.
    pub on_lookup: Option<(usize, usize)>,
    /// Every property of the field as written (`Caption`, `OptionCaption`, ...).
    pub properties: Vec<(String, String)>,
    /// The `TableRelation` arms in written order; empty when the field has none or its shape is
    /// not understood (`relation` tells the two apart).
    pub relation_arms: Vec<RelationArm>,
    /// `FieldClass = FlowFilter`: holds the caller's filter, never a value.
    pub flow_filter: bool,
    /// A FlowField's `CalcFormula`, when lowered. A FlowField with neither
    /// this nor `calc_refusal` declares no formula at all.
    pub calc: Option<CalcFormula>,
    /// Why a declared `CalcFormula` cannot be calculated yet.
    pub calc_refusal: Option<String>,
}

/// One arm of a `TableRelation`: `if (Kind = const(A)) Table.Field where(...)`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RelationArm {
    /// The arm applies when every condition holds (none: always, as the else arm does).
    pub conditions: Vec<RelationCond>,
    /// Object index of the related table; `None` when the workspace does not declare it.
    pub table: Option<usize>,
    /// The related table as written.
    pub table_name: String,
    /// Position of the related field in the related table; `None`: the first primary key field.
    pub field: Option<usize>,
    /// Which related rows are valid; `None` when a where() clause is there but not understood.
    pub wheres: Option<Vec<RelationWhere>>,
}

/// `Field = const(x)` on the referencing table: `field` is a position in its own field list.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RelationCond {
    pub field: usize,
    pub value: Const,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum RelationWhere {
    /// `related = const(text)` / `filter(text)`: `text` is a filter on the related field.
    Value { related: usize, text: String },
    /// `related = field(own)`: the related field equals the referencing row's field.
    Field { related: usize, own: usize },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CalcMethod {
    Sum,
    Count,
    Average,
    Min,
    Max,
    Exist,
    Lookup,
}

/// A FlowField's `CalcFormula` with every name resolved to positions.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CalcFormula {
    pub method: CalcMethod,
    /// A leading `-`: negates a sum, count, average, ... and `not`s an exist.
    pub negate: bool,
    /// Object index of the source table.
    pub table: usize,
    /// Position of the aggregated or looked-up field in the source table.
    pub field: Option<usize>,
    pub conds: Vec<CalcCond>,
}

/// One where-condition: a field of the source table against a value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CalcCond {
    /// Position in the source table.
    pub field: usize,
    pub value: CalcValue,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum CalcValue {
    /// `const(..)`, typed as the source field.
    Const(Const),
    /// `filter(..)`: a filter expression over the source field's type, with
    /// AL double-quoted names rewritten as single-quoted literals.
    Filter(String),
    /// `field(X)`: position of X in the table being calculated.
    Field(usize),
    /// `field(upperlimit(X))`.
    UpperLimit(usize),
    /// `field(filter(X))`: X's value read as a filter expression.
    FieldFilter(usize),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Slot {
    pub name: String,
    pub ty: RtType,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Routine {
    pub name: String,
    /// For field triggers: the field position in the table.
    pub field: Option<usize>,
    pub is_trigger: bool,
    pub attributes: Vec<Attribute>,
    /// Parameters occupy the first locals; `by_var[i]` for parameter i.
    pub params: usize,
    pub by_var: Vec<bool>,
    /// All locals: parameters, then the named return value (if any), then declared locals.
    pub locals: Vec<Slot>,
    pub ret: Option<RtType>,
    /// Local index of a named return value.
    pub ret_local: Option<usize>,
    pub body: Vec<Stmt>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Attribute {
    pub name: String,
    pub args: Vec<Const>,
    /// The attribute as written (`[EventSubscriber(ObjectType::Table, ...)]`).
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Const {
    Boolean(bool),
    Integer(i32),
    BigInteger(i64),
    /// Decimal as its exact text, to keep the IR free of float rounding.
    Decimal(String),
    Text(String),
    /// Day number (1 = 0001-01-01), 0 for `0D`.
    Date(i32),
    /// Milliseconds since midnight plus one, 0 for `0T`.
    Time(i64),
    DateTime(i64),
    Option(i32, Vec<String>),
    Enum(u32, i32),
    /// A field reference passed to a record method (`SetRange(Field, ...)`): its position.
    FieldPos(u32),
}

/// Where an assignment writes or a `var` argument points.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Place {
    Local(usize),
    Global(usize),
    /// Element of an array place.
    Index(Box<Place>, Vec<Expr>),
    /// Field at position `.1` of the record in place `.0`.
    Field(Box<Place>, usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    IntDiv,
    Mod,
    And,
    Or,
    Xor,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum UnOp {
    Neg,
    Plus,
    Not,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Arg {
    Value(Expr),
    /// Passed to a `var` parameter.
    Ref(Place),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Expr {
    Const(Const),
    Load(Place),
    Unary(UnOp, Box<Expr>),
    Binary(BinOp, Box<Expr>, Box<Expr>),
    /// `x in [a, b..c]`
    In(Box<Expr>, Vec<SetElem>),
    Ternary(Box<Expr>, Box<Expr>, Box<Expr>),
    /// Implicit or explicit conversion to a slot type.
    Convert(RtType, Box<Expr>),
    /// A procedure of object `object` (index into `Program::objects`) called
    /// on the current instance (`instance` = None) or on a codeunit value.
    /// `try_call`: the target is a `[TryFunction]` whose result is used, so an
    /// error is caught and the call yields `false`.
    Call {
        object: usize,
        routine: usize,
        instance: Option<Box<Expr>>,
        args: Vec<Arg>,
        try_call: bool,
    },
    /// A call through an interface: `receiver` evaluates to the codeunit instance the
    /// interface variable holds, and the call runs the target routine of that
    /// instance's object. `targets` are (object, routine) of every implementation.
    InterfaceCall {
        receiver: Box<Expr>,
        targets: Vec<(usize, usize)>,
        args: Vec<Arg>,
    },
    /// An enum value cast to an interface: a fresh instance of the codeunit that
    /// implements the interface for the value's ordinal. `by_ordinal` has every declared
    /// ordinal of the enum (extensions included), already resolved through the value's own
    /// `Implementation` and the enum's `DefaultImplementation`; `unknown` is the
    /// `UnknownValueImplementation` for ordinals the enum does not declare.
    EnumImpl {
        value: Box<Expr>,
        by_ordinal: Vec<(i32, usize)>,
        unknown: Option<usize>,
    },
    /// A control of the test page `page` evaluates to (`TP.Ctl`), by index
    /// into the page's control list (ADR-012).
    Control {
        page: Box<Expr>,
        control: usize,
    },
    /// The current row's value of column `column` of a query variable (`Q.Amount`).
    QueryColumn {
        query: Box<Expr>,
        column: usize,
    },
    /// A catalog builtin by overload id (`Record.Insert#2`), with its receiver.
    /// `ret_used`: the result is used, so methods with an optional Boolean
    /// result report failure as `false` instead of raising an error.
    Builtin {
        id: String,
        receiver: Option<Box<Arg>>,
        args: Vec<Arg>,
        ret_used: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum SetElem {
    One(Expr),
    Range(Option<Expr>, Option<Expr>),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Stmt {
    Assign(Place, Expr),
    Expr(Expr),
    If(Expr, Vec<Stmt>, Vec<Stmt>),
    While(Expr, Vec<Stmt>),
    Repeat(Vec<Stmt>, Expr),
    For {
        var: Place,
        from: Expr,
        to: Expr,
        down: bool,
        body: Vec<Stmt>,
    },
    Foreach {
        var: Place,
        collection: Expr,
        body: Vec<Stmt>,
    },
    Case {
        scrutinee: Expr,
        branches: Vec<(Vec<SetElem>, Vec<Stmt>)>,
        otherwise: Vec<Stmt>,
    },
    Exit(Option<Expr>),
    Break,
    AssertError(Vec<Stmt>),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn program_round_trips_through_serde() {
        let p = Program {
            apps: vec![],
            objects: vec![Object {
                kind: "codeunit".into(),
                id: Some(50100),
                name: "C".into(),
                globals: vec![Slot {
                    name: "G".into(),
                    ty: RtType::Integer,
                }],
                properties: vec![("Subtype".into(), "Test".into())],
                enum_values: vec![],
                enum_captions: vec![],
                table: None,
                rec_slot: None,
                xrec_slot: None,
                page: None,
                app: 0,
                query: None,
                xmlport: None,
                report: None,
                report_ext: None,
                routines: vec![Routine {
                    name: "P".into(),
                    field: None,
                    is_trigger: false,
                    attributes: vec![Attribute {
                        name: "Test".into(),
                        args: vec![],
                        text: "[Test]".into(),
                    }],
                    params: 0,
                    by_var: vec![],
                    locals: vec![Slot {
                        name: "L".into(),
                        ty: RtType::Text(None),
                    }],
                    ret: None,
                    ret_local: None,
                    body: vec![
                        Stmt::Assign(
                            Place::Global(0),
                            Expr::Binary(
                                BinOp::Add,
                                Box::new(Expr::Const(Const::Integer(1))),
                                Box::new(Expr::Load(Place::Global(0))),
                            ),
                        ),
                        Stmt::AssertError(vec![Stmt::Expr(Expr::Builtin {
                            id: "Dialog.Error#2".into(),
                            receiver: None,
                            args: vec![Arg::Value(Expr::Const(Const::Text("x".into())))],
                            ret_used: false,
                        })]),
                    ],
                }],
            }],
        };
        let json = serde_json::to_string(&p).unwrap();
        let back: Program = serde_json::from_str(&json).unwrap();
        assert_eq!(back, p);
    }
}
