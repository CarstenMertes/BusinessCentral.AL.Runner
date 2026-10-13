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
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Slot {
    pub name: String,
    pub ty: RtType,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Routine {
    pub name: String,
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
}

/// Where an assignment writes or a `var` argument points.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Place {
    Local(usize),
    Global(usize),
    /// Element of an array place.
    Index(Box<Place>, Vec<Expr>),
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
    Call {
        object: usize,
        routine: usize,
        instance: Option<Box<Expr>>,
        args: Vec<Arg>,
    },
    /// A catalog builtin by overload id (`Record.Insert#2`), with its receiver.
    Builtin {
        id: String,
        receiver: Option<Box<Arg>>,
        args: Vec<Arg>,
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
                routines: vec![Routine {
                    name: "P".into(),
                    is_trigger: false,
                    attributes: vec![Attribute {
                        name: "Test".into(),
                        args: vec![],
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
