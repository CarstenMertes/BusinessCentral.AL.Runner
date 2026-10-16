//! The executable form of a routine: register bytecode (ADR-034, `docs/reviews/2026-10-bytecode-design.md`).
//!
//! A routine's registers are its locals followed by temporaries. Instructions call the helpers the tree walker
//! calls, so a result is the same by construction. A node the compiler does not cover is kept as IR and run by the tree
//! walker from inside the bytecode (`TreeExpr`, `TreeStmt`), so every routine compiles and the share of such nodes is
//! the fallback share. Capture runs on the tree walker only.

mod compile;
mod run;

use al_ir as ir;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

pub(crate) use compile::compile;

/// Which form runs routines (`AL_EXEC=tree|bytecode|dual`; `dual` is the hook of the dual-run harness).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ExecMode {
    #[default]
    Tree,
    Bytecode,
    Dual,
}

impl ExecMode {
    /// The mode `AL_EXEC` names, read once.
    pub fn from_env() -> ExecMode {
        static MODE: OnceLock<ExecMode> = OnceLock::new();
        *MODE.get_or_init(|| match std::env::var("AL_EXEC").as_deref() {
            Ok("bytecode") => ExecMode::Bytecode,
            Ok("dual") => ExecMode::Dual,
            _ => ExecMode::Tree,
        })
    }
}

/// An instruction. Registers are `u16`, side-table indexes `u32`. `NONE` marks "no target".
#[derive(Debug, Clone, Copy)]
pub(crate) enum Op {
    /// `d := a` (a clone).
    Move {
        d: u16,
        a: u16,
    },
    /// `d := constant k`.
    Const {
        d: u16,
        k: u32,
    },
    /// `d := Global i`.
    LoadGlobal {
        d: u16,
        i: u32,
    },
    /// `d := read(place p)` (any place: fields, array elements, base globals).
    ReadPlace {
        d: u16,
        p: u32,
    },
    /// Local `i := a`; a Record or a Library value goes through `write` (copy semantics).
    StoreLocal {
        i: u16,
        a: u16,
        p: u32,
    },
    /// `write(place p, a)`.
    StorePlace {
        p: u32,
        a: u16,
    },
    Bin {
        op: ir::BinOp,
        d: u16,
        a: u16,
        b: u16,
    },
    /// `Bin` for an operator with an Integer fast path: two Integers are done here, anything else goes the general way.
    BinI {
        op: ir::BinOp,
        d: u16,
        a: u16,
        b: u16,
    },
    /// `d := Integer v`.
    ConstI {
        d: u16,
        v: i32,
    },
    Un {
        op: ir::UnOp,
        d: u16,
        a: u16,
    },
    Convert {
        d: u16,
        a: u16,
        t: u32,
    },
    Jump {
        t: u32,
    },
    JumpIfNot {
        a: u16,
        t: u32,
    },
    /// The stop flag is checked at each round of a loop.
    CheckStop,
    /// `d := the position of the call stack's top` (a loop remembers its line).
    SavePos {
        d: u16,
    },
    RestorePos {
        a: u16,
    },
    Pos {
        p: u32,
    },
    ForInit {
        s: u32,
    },
    ForTest {
        s: u32,
        t: u32,
    },
    ForSet {
        s: u32,
    },
    ForNext {
        s: u32,
    },
    Call {
        s: u32,
        d: u16,
    },
    Builtin {
        s: u32,
        d: u16,
    },
    /// A node the compiler does not cover: the tree walker evaluates the IR expression.
    TreeExpr {
        e: u32,
        d: u16,
    },
    /// A statement the compiler does not cover. A `Break` or `Continue` that leaves it goes to `brk` / `cont`.
    TreeStmt {
        s: u32,
        brk: u32,
        cont: u32,
    },
    /// Lets go of a temporary whose value nobody reads.
    Clear {
        a: u16,
    },
    /// `Exit()`.
    Ret,
    /// `Exit(a)`.
    RetVal {
        a: u16,
    },
    /// A `break` or `continue` outside every loop of this routine: the signal goes to the caller as in the tree walker.
    RaiseBreak,
    RaiseContinue,
}

pub(crate) const NONE: u32 = u32::MAX;

pub(crate) struct CallSite {
    pub object: usize,
    pub routine: usize,
    pub instance: Option<u16>,
    pub arg_regs: Vec<u16>,
    /// The IR arguments with the value expressions blanked: only the by-reference places matter after the call.
    pub args: Vec<ir::Arg>,
    pub try_call: bool,
}

pub(crate) struct BuiltinSite {
    pub native: al_builtins::native::NativeFn,
    pub class: al_builtins::builtin_id::Class,
    pub receiver: Option<u16>,
    pub receiver_arg: Option<ir::Arg>,
    pub arg_regs: Vec<u16>,
    pub args: Vec<ir::Arg>,
    pub ret_used: bool,
}

pub(crate) struct ForSite {
    pub var: ir::Place,
    pub down: bool,
    pub from: u16,
    pub to: u16,
    /// Registers that hold the counter and the last value, as `Value::BigInteger`.
    pub i: u16,
    pub end: u16,
    /// The counter is a local of type Integer: the round-trip through `write` and `read` is a plain store and load.
    pub int_local: Option<u16>,
}

/// The compiled form of one routine.
pub(crate) struct Chunk {
    pub code: Vec<Op>,
    pub consts: Vec<ir::Const>,
    pub places: Vec<ir::Place>,
    pub types: Vec<al_builtins::value::RtType>,
    pub exprs: Vec<ir::Expr>,
    pub stmts: Vec<ir::Stmt>,
    /// The node kind of each tree expression and statement, for the fallback report.
    pub expr_kinds: Vec<&'static str>,
    pub stmt_kinds: Vec<&'static str>,
    pub calls: Vec<CallSite>,
    pub builtins: Vec<BuiltinSite>,
    pub fors: Vec<ForSite>,
    /// Registers `0..nlocals` are the routine's locals.
    pub nlocals: u16,
    /// Temporaries after the locals.
    pub temps: u16,
    /// How many nodes run on the tree walker from inside this chunk.
    pub tree_nodes: u32,
}

/// The chunks of a program, compiled the first time a routine is called.
pub(crate) struct ExecProgram {
    chunks: Vec<Vec<OnceLock<Option<Box<Chunk>>>>>,
}

impl ExecProgram {
    pub(crate) fn new(program: &ir::Program) -> ExecProgram {
        ExecProgram {
            chunks: program
                .objects
                .iter()
                .map(|o| o.routines.iter().map(|_| OnceLock::new()).collect())
                .collect(),
        }
    }

    /// The chunk of routine `routine` of object `object`, if it compiles.
    pub(crate) fn chunk(&self, program: &ir::Program, object: usize, routine: usize) -> Option<&Chunk> {
        self.chunks[object][routine]
            .get_or_init(|| {
                let c = compile(&program.objects[object].routines[routine])?;
                STATS.routines.fetch_add(1, Ordering::Relaxed);
                if c.tree_nodes > 0 {
                    STATS.routines_with_tree.fetch_add(1, Ordering::Relaxed);
                }
                Some(Box::new(c))
            })
            .as_deref()
    }
}

/// Whether the counters count (`AL_EXEC_REPORT=1`), read once.
pub(crate) fn counting() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("AL_EXEC_REPORT").is_some())
}

/// Process-wide counters for the fallback report (`AL_EXEC_REPORT=1`).
pub(crate) struct Stats {
    pub routines: AtomicU64,
    pub routines_with_tree: AtomicU64,
    pub bytecode_calls: AtomicU64,
    pub tree_calls: AtomicU64,
    pub tree_exprs_run: AtomicU64,
    pub tree_stmts_run: AtomicU64,
}

/// How often each kind of node ran on the tree walker from inside bytecode (`AL_EXEC_REPORT=1`).
pub(crate) static TREE_KINDS: std::sync::Mutex<Vec<(String, u64)>> = std::sync::Mutex::new(Vec::new());

pub(crate) fn note_tree(kind: &'static str, stmt: bool) {
    let key = format!("{}{kind}", if stmt { "stmt " } else { "expr " });
    if let Ok(mut v) = TREE_KINDS.lock() {
        match v.iter_mut().find(|(k, _)| *k == key) {
            Some(e) => e.1 += 1,
            None => v.push((key, 1)),
        }
    }
}

pub(crate) static STATS: Stats = Stats {
    routines: AtomicU64::new(0),
    routines_with_tree: AtomicU64::new(0),
    bytecode_calls: AtomicU64::new(0),
    tree_calls: AtomicU64::new(0),
    tree_exprs_run: AtomicU64::new(0),
    tree_stmts_run: AtomicU64::new(0),
};

/// One line for the fallback report: routines compiled, how many keep a tree node, and the share of executed work.
pub fn report() -> String {
    let g = |a: &AtomicU64| a.load(Ordering::Relaxed);
    let mut kinds = TREE_KINDS.lock().map(|v| v.clone()).unwrap_or_default();
    kinds.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
    let top: Vec<String> = kinds.iter().take(12).map(|(k, n)| format!("{k} {n}")).collect();
    format!(
        "exec: {} routines compiled ({} with tree nodes); calls: {} bytecode, {} tree; tree nodes run: {} exprs, {} stmts; most run: {}",
        g(&STATS.routines),
        g(&STATS.routines_with_tree),
        g(&STATS.bytecode_calls),
        g(&STATS.tree_calls),
        g(&STATS.tree_exprs_run),
        g(&STATS.tree_stmts_run),
        top.join(", "),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_instruction_is_small() {
        assert!(std::mem::size_of::<Op>() <= 16, "{}", std::mem::size_of::<Op>());
    }
}
