//! Page and report extension code that names a member of its base object (ADR-023).
//!
//! The mechanism is built (checker scope, `Place::BaseGlobal`, `Expr::BaseInstance`, the VM link),
//! but what real BC does is pinned only by probes (ADR-023, "Probes"). Each probe turns one kind
//! of access on: its id goes into [`PINNED`] in the commit that cites the corpus probe test. Until
//! then the lowering refuses the access with a reason that names the probe.
//!
//! For measurement and development only, the environment variable `AL_UNPINNED_BASE_ACCESS`
//! switches the checker scope on and pins cases without a probe: `1` or `all` pins every case,
//! any other value is a comma-separated list of probe ids to pin (`none` pins nothing). A run with
//! it set is not a conformance result.

use al_syntax::ast;

/// Probe ids whose behavior a corpus test pins. The ids are the ones of ADR-023: `P1` to `P10` for
/// pages, `R1` to `R4` for reports.
pub const PINNED: &[&str] = &[];

/// The value of `AL_UNPINNED_BASE_ACCESS`, if set.
fn switch() -> Option<String> {
    std::env::var("AL_UNPINNED_BASE_ACCESS").ok()
}

/// Whether the behavior of probe `id` is pinned (or the measurement switch pins it).
pub fn pinned(id: &str) -> bool {
    PINNED.contains(&id)
        || switch().is_some_and(|v| {
            v == "1" || v.eq_ignore_ascii_case("all") || v.split(',').any(|x| x.trim().eq_ignore_ascii_case(id))
        })
}

/// Whether the checker puts the base object's members into an extension's scope. Off while
/// nothing is pinned, so that the compile reaches exactly what it did before.
pub fn scope_enabled() -> bool {
    !PINNED.is_empty() || switch().is_some()
}

/// The refusal for an access whose probe has not confirmed it.
pub fn unpinned_reason(what: &str, probe: &str, noun: &str) -> String {
    format!(
        "{what} is a member of the {noun} it extends: {noun} extension code on base {noun} members is not lowered yet \
         (not pinned: ADR-023 probe {probe})"
    )
}

/// The variables of the base object that extension code may name: those of its `protected var`
/// sections, as (lower-case name, declaration).
pub fn visible_vars(base: &ast::Object) -> Vec<(String, ast::VarDecl)> {
    base.protected_var_decls()
        .flat_map(|d| d.names().into_iter().map(move |n| (n.to_lowercase(), d.clone())))
        .collect()
}

/// Whether the base object has a variable `name` (any section), case-insensitive.
pub fn declares_var(base: &ast::Object, name: &str) -> bool {
    base.var_decls()
        .any(|v| v.names().iter().any(|x| x.eq_ignore_ascii_case(name)))
}

/// The routine `name` of the base object that extension code may call: a page's procedure that is
/// not `local`; a report's `protected` procedure (whether a public one is callable is probe R2).
pub fn visible_procs<'a>(
    base: &'a ast::Object,
    report: bool,
    name: &'a str,
) -> impl Iterator<Item = ast::Routine> + 'a {
    base.routines()
        .filter(move |r| r.name().eq_ignore_ascii_case(name) && if report { r.is_protected() } else { !r.is_local() })
}

/// The slot of variable `name` among the globals of the base object, from its syntax only (the
/// lowered base object is not needed, so the two lower independently). A page lays out its
/// variables in declaration order. A report lays out `items` (the number of distinct data item
/// names), then one slot for the request-page flag, then its variables.
pub fn slot_of(base: &ast::Object, report_items: Option<usize>, name: &str) -> Option<usize> {
    let before = report_items.map_or(0, |n| n + 1);
    base.var_decls()
        .flat_map(|v| v.names())
        .position(|n| n.eq_ignore_ascii_case(name))
        .map(|i| before + i)
}
