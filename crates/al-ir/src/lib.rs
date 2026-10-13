//! Typed IR (ADR-011): every name resolved, every node typed, conversions
//! explicit. Plain data, serde-serialisable, no syntax nodes.

use al_builtins::value::RtType;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Program {
    pub objects: Vec<Object>,
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
    Routine(usize),
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
    /// A control of the test page `page` evaluates to (`TP.Ctl`), by index
    /// into the page's control list (ADR-012).
    Control {
        page: Box<Expr>,
        control: usize,
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
