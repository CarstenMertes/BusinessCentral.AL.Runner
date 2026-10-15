//! Lowering resolved syntax to the typed IR (ADR-011). Anything the IR or the
//! builtins registry cannot express yet becomes a reason on the routine
//! (`Routine::unsupported` in `Lowered`), never a guess.

use crate::check::{CheckResult, Res};
use crate::members::MemberTable;
use crate::types::Ty;
use crate::workspace::{ObjKind, Resolution, Workspace};
use al_builtins::value::RtType;
use al_ir as ir;
use al_syntax::{SyntaxKind, SyntaxNode, ast};
use std::collections::HashMap;

/// Wall time of the parts of a compile (`--timings`).
#[derive(Debug, Clone, Copy, Default)]
pub struct CompileTimings {
    /// Reading the apps: parsing the workspace's files, scanning the dependencies'.
    pub load: std::time::Duration,
    pub members: std::time::Duration,
    pub check: crate::check::CheckTimings,
    pub lower: std::time::Duration,
    /// With the compile cache: checking the cache against the apps and reading its blocks.
    pub cache_blocks: std::time::Duration,
}

pub struct Lowered {
    /// Zero unless the compile ran through `compile_apps_with`.
    pub timings: CompileTimings,
    /// Shared with the engine that runs it: the program is big, and copying it cost as much as
    /// lowering the workspace's own code.
    pub program: std::sync::Arc<ir::Program>,
    /// Per object, per routine: why it cannot run (empty = it can).
    pub unsupported: Vec<Vec<Vec<String>>>,
    /// Per object, per routine: the routines it calls directly, as (object, routine).
    pub calls: Vec<Vec<Vec<(usize, usize)>>>,
    /// Per object: it belongs to a dependency app, so no test or install code of it runs (ADR-013).
    pub dependency: Vec<bool>,
    /// Per object: its lowered form came from a compile cache (`provided.rs`). `calls` holds only
    /// the direct calls of such an object; `calls_of` adds the rest.
    pub provided: Vec<bool>,
    /// What a variable of a page, report or xmlport can drive (see `Driven`); set with `provided`.
    driven: Option<DrivenCtx>,
    /// With `lower_for_cache`: the lowered dependency objects and what the lowering of others
    /// reads about them, ready to be stored.
    pub exported: Option<crate::provided::DepSet>,
    /// The diagnostics with the AL compiler's own codes (`alcodes.rs`) that the check found in the
    /// workspace's apps. Code that BC rejects; the compile goes on.
    pub al_diagnostics: Vec<crate::workspace::Diagnostic>,
    /// Per object: where its source positions are in `Program::positions` (start, count). What the
    /// incremental compile (`incremental.rs`) needs to renew the lowering of some objects.
    pub(crate) pos_ranges: Vec<(u32, u32)>,
    /// What the checker raised about the objects (not the workspace-wide codes), for the same.
    pub(crate) checker_diags: Vec<crate::workspace::Diagnostic>,
    /// `CheckResult::skipped` of the compile, for the same.
    pub(crate) skipped: Vec<bool>,
}

/// The routines a variable of a page, report or xmlport type can run besides the ones the code
/// calls: a test page drives every routine of its page, the page's extensions and its parts.
#[derive(Clone)]
pub(crate) struct DrivenCtx {
    page_extensions: HashMap<usize, Vec<usize>>,
    page_parts: Vec<Vec<usize>>,
    page_runs: Vec<Vec<usize>>,
    /// Per object: an xmlport or report, whose triggers Import, Export and Run call.
    runs_triggers: Vec<bool>,
    report_extension: Vec<Vec<usize>>,
    /// Per object: whether it belongs to a dependency app.
    dependency: Vec<bool>,
    /// Per object: whether it is a codeunit (a `RunObject` target that runs its `OnRun`).
    codeunit: Vec<bool>,
    /// Per object: whether it is a report, report extension, xmlport or query. Using one reaches
    /// its triggers and the routines made for its properties, not its other procedures.
    triggers_only: Vec<bool>,
    /// Per object and routine: the `ROLE_*` bits of the routine. Structure only: the same for
    /// every compile of the same sources, whatever of them was compiled.
    routine_roles: Vec<Vec<u8>>,
}

/// Routine roles (`DrivenCtx::routine_roles`).
pub(crate) const ROLE_TRIGGER: u8 = 1;
/// A routine `pages::expand` generated for a control property (its name starts `__`).
pub(crate) const ROLE_SYNTHETIC: u8 = 2;
pub(crate) const ROLE_ON_RUN: u8 = 4;

/// The role bits of a routine.
pub(crate) fn role_of(r: &ast::Routine) -> u8 {
    let name = r.name();
    let mut role = 0;
    if r.is_trigger() {
        role |= ROLE_TRIGGER;
        if name.eq_ignore_ascii_case("OnRun") {
            role |= ROLE_ON_RUN;
        }
    }
    if name.starts_with("__") {
        role |= ROLE_SYNTHETIC;
    }
    role
}

impl Lowered {
    /// The routines routine `r` of object `o` can call: the ones its code names and the ones a
    /// variable of a page, report or xmlport type drives.
    pub fn calls_of(&self, o: usize, r: usize) -> Vec<(usize, usize)> {
        let mut out = Vec::new();
        self.calls_into(o, r, &mut out);
        out
    }

    /// [`calls_of`](Self::calls_of), appended to `out`.
    pub fn calls_into(&self, o: usize, r: usize, out: &mut Vec<(usize, usize)>) {
        out.extend(self.calls[o][r].iter().copied());
        if !self.provided.get(o).copied().unwrap_or(false) {
            return;
        }
        if let Some(d) = &self.driven {
            let object = &self.program.objects[o];
            let routine = &object.routines[r];
            driven_edges(d, routine.locals.iter().chain(&object.globals), out);
        }
    }
}

pub(crate) type L<T> = Result<T, String>;

pub(crate) struct Lowerer<'a> {
    /// Global slot of the implicit `Rec` of the current object.
    rec_slot: Option<usize>,
    table_of: &'a [Option<usize>],
    ws: &'a Workspace,
    members: &'a MemberTable<'a>,
    checked: &'a CheckResult,
    catalog: &'a al_builtins::catalog::Catalog,
    routine_at: &'a RoutineAt,
    oi: usize,
    file: usize,
    globals: HashMap<String, usize>,
    locals: HashMap<String, usize>,
    /// Declared text of Label variables, by lowercase name; reads lower to that constant.
    global_labels: HashMap<String, String>,
    local_labels: HashMap<String, String>,
    reasons: Vec<String>,
    calls: Vec<(usize, usize)>,
    /// Per object: why the test client cannot drive it yet (pages).
    page_reasons: &'a [Vec<String>],
    layouts: &'a Seq<'a>,
    /// Per object: why a variable of an xmlport is refused.
    xml_reasons: &'a [Option<String>],
    /// Per object: why a variable of a report is refused.
    report_reasons: &'a [Option<String>],
    /// The objects whose extensions or implementers the lowering read (`provided.rs`).
    embeds: std::cell::RefCell<std::collections::BTreeSet<usize>>,
    /// Answers many routines ask for, computed once per program.
    memo: &'a Memo,
    /// Return type of the routine being lowered (a DotNet result in `exit` is coerced to it).
    ret_ty: Option<RtType>,
    /// The declared return type of the routine being lowered (an interface result converts in `exit`).
    ret_decl: Option<Ty>,
    /// The routine nodes of every object, numbered as `routine_at` numbers them.
    routine_nodes: &'a Nodes<'a>,
    /// The global slot of each variable of a page extension (see `lower`).
    ext_slots: &'a [HashMap<String, usize>],
    /// The page extensions of each page, in the order they apply.
    page_extensions: &'a HashMap<usize, Vec<usize>>,
    /// `Program::positions` while the object is lowered (taken from and given back to the program).
    positions: Vec<ir::SrcPos>,
    /// The line table of the object's file; `None` when positions are off.
    lines: Option<std::rc::Rc<FileLines>>,
    /// Statements of the routine being lowered get position markers (off for made-up routines).
    emit_pos: bool,
    /// Index of the routine being lowered in its object.
    routine_idx: usize,
    /// The assumptions the lowering of each statement being lowered rests on (innermost last).
    assumed: Vec<Vec<al_builtins::assumptions::Assumption>>,
}

/// What lowering asks about an object again and again. Each answer is computed on first use and
/// shared by every routine of the program: walking the syntax tree for it per use was a third of
/// the lowering time.
#[derive(Default)]
pub(crate) struct Memo {
    /// The codeunits that implement an interface (or one that extends it), in object order.
    implementers: std::cell::RefCell<HashMap<usize, std::rc::Rc<Vec<usize>>>>,
    /// The `TableType` property of a table, trimmed.
    table_type: std::cell::RefCell<HashMap<usize, std::rc::Rc<Vec<String>>>>,
}

/// Line starts of a source file, for turning offsets into the author's line and column.
pub(crate) struct FileLines {
    file: u32,
    /// Offset of the first byte of every line of the file as the author wrote it.
    starts: Vec<u32>,
    /// Byte ranges the compiler added to the parsed text (page expression routines).
    synthetic: Vec<(usize, usize)>,
}

impl FileLines {
    fn new(ws: &Workspace, file: usize) -> FileLines {
        let text = ws.text(file).as_bytes();
        let mut starts = vec![0u32];
        starts.extend(
            text.iter()
                .enumerate()
                .filter(|(_, b)| **b == b'\n')
                .map(|(i, _)| i as u32 + 1),
        );
        FileLines {
            file: file as u32,
            starts,
            synthetic: ws.synthetic(file).to_vec(),
        }
    }

    /// The 1-based (line, column) the author sees at `offset` of the parsed text.
    fn locate(&self, offset: usize) -> (u32, u32) {
        let added: usize = self
            .synthetic
            .iter()
            .take_while(|(_, b)| *b <= offset)
            .map(|(a, b)| b - a)
            .sum();
        let offset = offset - added;
        let line = self.starts.partition_point(|&s| s as usize <= offset) - 1;
        (line as u32 + 1, (offset - self.starts[line] as usize) as u32 + 1)
    }
}

/// The first and last token of `n` that are not whitespace or comments.
fn significant_span(n: &SyntaxNode) -> Option<(usize, usize)> {
    let mut first = n.first_token()?;
    while first.kind().is_trivia() {
        first = first.next_token()?;
    }
    let mut last = n.last_token()?;
    while last.kind().is_trivia() {
        last = last.prev_token()?;
    }
    Some((
        usize::from(first.text_range().start()),
        usize::from(last.text_range().end()).max(usize::from(first.text_range().start())),
    ))
}

fn key(file: usize, n: &SyntaxNode) -> (usize, usize, usize) {
    let r = n.text_range();
    (file, r.start().into(), r.end().into())
}

pub(crate) fn is_stmt(k: SyntaxKind) -> bool {
    use SyntaxKind::*;
    matches!(
        k,
        Block
            | AssignStmt
            | ExprStmt
            | IfStmt
            | CaseStmt
            | WhileStmt
            | RepeatStmt
            | ForStmt
            | ForeachStmt
            | ExitStmt
            | BreakStmt
            | AssertErrorStmt
            | WithStmt
            | ContinueStmt
    )
}

/// The builtin members AL writes as a property: `X.Name := v` is the call `X.Name(v)`. Each is a
/// property of its type in the AL language, so valid code assigns it. Listed by name, not by
/// shape: a method with one optional parameter is not a property (`Rec.Find(Which)`).
///
/// - `FieldRef.Value`: corpus recordref/TestRecordRefCRUD.al:20-23.
/// - `Page.Editable`, `Page.PromptMode`: handlers/TestPageModalNewRowLine_Objects.al:69;
///   `Page.Caption`: handlers/TestPageCaption_Page.al:44; `Page.LookupMode`: the property of the
///   same name on a page.
/// - `Notification.Id`, `.Message`, `.Scope`: handlers/TestNotificationHandler.al:21 and :55.
/// - `Record.ReadIsolation`, `RecordRef.ReadIsolation` and `.SecurityFiltering`: Base
///   Application (`Item.ReadIsolation := IsolationLevel::ReadUncommitted`).
/// - `ErrorInfo.Message`, `.DetailedMessage`, `.Title`, `.ControlName`, `.FieldNo`, `.PageNo`,
///   `.TableId`, `.Verbosity`, `.DataClassification`, `.Collectible`, `.ErrorType`,
///   `.CustomDimensions`: the call form pins getter and storage (errorinfo/TestErrorInfoType.al:
///   249-271, :284-307, :313-345; TestErrorInfoErrorType.al:25-45). Nothing sets `RecordId` in
///   the corpus (only `Create(.., Record)` does, :360-377): its native raises `Unsupported` when
///   run, so the assignment is refused when it is reached, not before. `SystemId` has no native.
/// - `Record.Ascending`, `.Mark`, `.MarkedOnly`: record/TestRecordFind.al:237-284 (`Mark`,
///   `MarkedOnly`), handlers/TestPartOwnSortVsSubPageView.al:187 and :300 (`Ascending(false)`),
///   handlers/TestPageActionRunPageView.al:134 (`Ascending()`).
/// - `Report.Language`, `.FormatRegion`: handlers/TestReportCurrReportLanguage.al:58-103.
/// - `Report.UseRequestPage`: handlers/TestReportGetFilterInTriggers.al:66 and
///   codeunit/TestWriteTxTestBoundary.al:272 (`UseRequestPage(false)`).
/// - `System.WorkDate`: session/TestSessionFunctions.al:63-65 (`WorkDate(20240101D)`).
/// - `HttpRequestMessage.Method`: network/TestHttpRequestMessage.al:23; `.Content`: :39.
/// - `File.TextMode`, `File.WriteMode`: Base Application's "Export Consolidation" report.
fn is_property_setter(receiver: &str, name: &str) -> bool {
    matches!(
        (receiver, name),
        ("FieldRef" | "TestField", "Value")
            | ("InStream" | "OutStream", "Position")
            | ("Page", "Editable" | "PromptMode" | "Caption" | "LookupMode")
            | ("Notification", "Id" | "Message" | "Scope")
            | (
                "ErrorInfo",
                "Message"
                    | "DetailedMessage"
                    | "Title"
                    | "ControlName"
                    | "FieldNo"
                    | "PageNo"
                    | "TableId"
                    | "Verbosity"
                    | "DataClassification"
                    | "Collectible"
                    | "ErrorType"
                    | "CustomDimensions"
                    | "RecordId"
            )
            | ("Record" | "RecordRef", "ReadIsolation" | "SecurityFiltering")
            | ("Record", "FilterGroup" | "Ascending" | "Mark" | "MarkedOnly")
            | ("RecordRef", "FilterGroup")
            | ("Report", "UseRequestPage" | "Language" | "FormatRegion")
            | ("System", "WorkDate")
            | ("HttpRequestMessage", "Method" | "Content")
            | ("File", "TextMode" | "WriteMode")
    )
}

fn sig_tokens(n: &SyntaxNode) -> Vec<al_syntax::SyntaxToken> {
    n.children_with_tokens()
        .filter_map(|e| e.into_token())
        .filter(|t| !t.kind().is_trivia())
        .collect()
}

mod assemble;
mod call;
mod compile;
mod convert;
mod dotnet;
mod expr;
mod pages;
mod shared;
mod stmt;
mod tables;
#[cfg(test)]
mod tests;

pub use self::assemble::*;
pub use self::compile::*;
pub(crate) use self::shared::*;
