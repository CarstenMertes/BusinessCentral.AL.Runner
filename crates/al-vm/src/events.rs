//! Event publishers and subscribers: the registry built once per program
//! (ADR-001: it is part of the immutable `Engine`) and the pure parsing of
//! the attributes that feed it. Dispatch lives in `Session` (`dispatch.rs`).
//!
//! Resolution is by name against the workspace only. A subscriber whose
//! publisher is not in the workspace (Base App, platform codeunits) attaches
//! to nothing: the publisher cannot run here, so it cannot fire.

use al_ir as ir;
use std::collections::HashMap;

/// One `[EventSubscriber(...)]` routine bound to a publisher in the workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Subscriber {
    pub object: usize,
    pub routine: usize,
    /// The subscriber codeunit declares `EventSubscriberInstance = Manual`:
    /// it only receives events while an instance is bound.
    pub manual: bool,
    /// `ElementName`, as written (a field name for field events, else empty).
    pub element: String,
}

/// An `[IntegrationEvent]` / `[BusinessEvent]` / `[InternalEvent]` routine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Publisher {
    /// Object that subscribers name: the routine's own object, or the base
    /// table for an event declared in a table extension.
    pub object: usize,
    /// Lower-case event name.
    pub event: String,
    pub include_sender: bool,
    pub isolated: bool,
}

#[derive(Debug, Default)]
pub struct Registry {
    /// (publisher object, lower-case event name) → subscribers, in program order.
    pub subscribers: HashMap<(usize, String), Vec<Subscriber>>,
    /// (object, routine) → the event the routine publishes.
    pub publishers: HashMap<(usize, usize), Publisher>,
    /// Whether any subscriber of the platform codeunit `Global Triggers` exists.
    /// That codeunit is not in the workspace, so those subscribers never fire
    /// here; writes to the tables they name are refused instead.
    pub has_global_triggers: bool,
    /// Tables named by the subscribers that enable those platform triggers
    /// (`GetGlobalTableTriggerMask`, `GetDatabaseTableTriggerSetup`). When no
    /// table can be read off them, every table counts.
    pub global_trigger_tables: std::collections::HashSet<usize>,
}

fn is_manual(o: &ir::Object) -> bool {
    o.properties
        .iter()
        .any(|(k, v)| k.eq_ignore_ascii_case("EventSubscriberInstance") && v.trim().eq_ignore_ascii_case("Manual"))
}

fn flag(args: &[String], i: usize) -> bool {
    args.get(i).is_some_and(|a| a.eq_ignore_ascii_case("true"))
}

impl Registry {
    pub fn build(program: &ir::Program) -> Registry {
        let mut reg = Registry::default();
        // Events declared in a table extension are subscribed to on the base table.
        let mut base_of: HashMap<usize, usize> = HashMap::new();
        for (oi, o) in program.objects.iter().enumerate() {
            if let Some(t) = &o.table {
                for &e in &t.extensions {
                    base_of.insert(e, oi);
                }
            }
        }
        let mut by_name: HashMap<(&str, String), usize> = HashMap::new();
        let mut by_id: HashMap<(&str, u32), usize> = HashMap::new();
        for (oi, o) in program.objects.iter().enumerate() {
            by_name.entry((o.kind.as_str(), o.name.to_lowercase())).or_insert(oi);
            if let Some(id) = o.id {
                by_id.entry((o.kind.as_str(), id)).or_insert(oi);
            }
        }
        for (oi, o) in program.objects.iter().enumerate() {
            for (ri, r) in o.routines.iter().enumerate() {
                for a in &r.attributes {
                    let name = a.name.to_ascii_lowercase();
                    let args = split_args(&a.text);
                    match name.as_str() {
                        "integrationevent" | "businessevent" | "internalevent" => {
                            // IntegrationEvent(IncludeSender, GlobalVarAccess[, Isolated]);
                            // BusinessEvent / InternalEvent(IncludeSender[, Isolated]).
                            let isolated = if name == "integrationevent" {
                                flag(&args, 2)
                            } else {
                                flag(&args, 1)
                            };
                            reg.publishers.insert(
                                (oi, ri),
                                Publisher {
                                    object: base_of.get(&oi).copied().unwrap_or(oi),
                                    event: r.name.to_lowercase(),
                                    include_sender: flag(&args, 0),
                                    isolated,
                                },
                            );
                        }
                        "eventsubscriber" if args.len() >= 3 => {
                            let kind = match unquote(args[0].rsplit("::").next().unwrap_or(""))
                                .to_lowercase()
                                .as_str()
                            {
                                "codeunit" => "codeunit",
                                "table" => "table",
                                "page" => "page",
                                "report" => "report",
                                "query" => "query",
                                "xmlport" => "xmlport",
                                _ => continue,
                            };
                            let target = unquote(args[1].rsplit("::").next().unwrap_or(""));
                            let found = by_name.get(&(kind, target.to_lowercase())).copied().or_else(|| {
                                target
                                    .parse::<u32>()
                                    .ok()
                                    .and_then(|id| by_id.get(&(kind, id)).copied())
                            });
                            let event = unquote(&args[2]).to_lowercase();
                            match found {
                                Some(t) => reg.subscribers.entry((t, event)).or_default().push(Subscriber {
                                    object: oi,
                                    routine: ri,
                                    manual: is_manual(o),
                                    element: args.get(3).map(|e| unquote(e)).unwrap_or_default(),
                                }),
                                None if kind == "codeunit" && target.eq_ignore_ascii_case("Global Triggers") => {
                                    reg.has_global_triggers = true;
                                    if matches!(
                                        event.as_str(),
                                        "getglobaltabletriggermask" | "getdatabasetabletriggersetup"
                                    ) {
                                        let mut ids = Vec::new();
                                        int_consts(&r.body, &mut ids);
                                        let tables: Vec<usize> = ids
                                            .iter()
                                            .filter_map(|&id| by_id.get(&("table", id as u32)).copied())
                                            .collect();
                                        reg.global_trigger_tables.extend(tables);
                                    }
                                }
                                None => {}
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
        reg
    }

    /// Whether any subscriber (manual or not) listens to `event` of `object`.
    pub fn is_subscribed(&self, object: usize, event: &str) -> bool {
        self.subscribers
            .get(&(object, event.to_string()))
            .is_some_and(|v| !v.is_empty())
    }

    /// Whether a write to table `object` would raise `Global Triggers` events in BC.
    pub fn global_triggers_apply(&self, object: usize) -> bool {
        // Without a table named by a setup subscriber, any table could be affected.
        self.has_global_triggers
            && (self.global_trigger_tables.is_empty() || self.global_trigger_tables.contains(&object))
    }
}

/// Integer literals anywhere in `stmts` (object ids written as `Database::X`
/// lower to integers).
fn int_consts(stmts: &[ir::Stmt], out: &mut Vec<i64>) {
    use ir::Stmt as S;
    for s in stmts {
        match s {
            S::Assign(_, e) | S::Expr(e) => int_consts_expr(e, out),
            S::If(c, t, e) => {
                int_consts_expr(c, out);
                int_consts(t, out);
                int_consts(e, out);
            }
            S::While(c, b) | S::Repeat(b, c) => {
                int_consts_expr(c, out);
                int_consts(b, out);
            }
            S::For { from, to, body, .. } => {
                int_consts_expr(from, out);
                int_consts_expr(to, out);
                int_consts(body, out);
            }
            S::Foreach { collection, body, .. } => {
                int_consts_expr(collection, out);
                int_consts(body, out);
            }
            S::Case {
                scrutinee,
                branches,
                otherwise,
            } => {
                int_consts_expr(scrutinee, out);
                for (labels, b) in branches {
                    for l in labels {
                        match l {
                            ir::SetElem::One(e) => int_consts_expr(e, out),
                            ir::SetElem::Range(a, b) => {
                                a.iter().chain(b).for_each(|e| int_consts_expr(e, out));
                            }
                        }
                    }
                    int_consts(b, out);
                }
                int_consts(otherwise, out);
            }
            S::Exit(e) => e.iter().for_each(|e| int_consts_expr(e, out)),
            S::Break => {}
            S::AssertError(b) => int_consts(b, out),
        }
    }
}

fn int_consts_expr(e: &ir::Expr, out: &mut Vec<i64>) {
    use ir::Expr as E;
    match e {
        E::Const(ir::Const::Integer(i)) => out.push(i64::from(*i)),
        E::Const(ir::Const::BigInteger(i)) => out.push(*i),
        E::Const(_) | E::Load(_) => {}
        E::Unary(_, e)
        | E::Convert(_, e)
        | E::Control { page: e, .. }
        | E::QueryColumn { query: e, .. }
        | E::EnumImpl { value: e, .. } => int_consts_expr(e, out),
        E::Binary(_, a, b) => {
            int_consts_expr(a, out);
            int_consts_expr(b, out);
        }
        E::In(v, set) => {
            int_consts_expr(v, out);
            for s in set {
                match s {
                    ir::SetElem::One(e) => int_consts_expr(e, out),
                    ir::SetElem::Range(a, b) => a.iter().chain(b).for_each(|e| int_consts_expr(e, out)),
                }
            }
        }
        E::Ternary(a, b, c) => {
            int_consts_expr(a, out);
            int_consts_expr(b, out);
            int_consts_expr(c, out);
        }
        E::InterfaceCall { receiver, args, .. } => {
            int_consts_expr(receiver, out);
            for a in args {
                if let ir::Arg::Value(e) = a {
                    int_consts_expr(e, out);
                }
            }
        }
        E::Call { instance, args, .. } => {
            instance.iter().for_each(|e| int_consts_expr(e, out));
            for a in args {
                if let ir::Arg::Value(e) = a {
                    int_consts_expr(e, out);
                }
            }
        }
        E::Builtin { receiver, args, .. } => {
            if let Some(ir::Arg::Value(e)) = receiver.as_deref() {
                int_consts_expr(e, out);
            }
            for a in args {
                if let ir::Arg::Value(e) = a {
                    int_consts_expr(e, out);
                }
            }
        }
    }
}

/// Splits the argument list of an attribute written as `[Name(a, 'b,c', "d")]`
/// at top-level commas. Quotes (`'…'` with `''` escapes, `"…"`) and brackets
/// protect their commas; each piece is trimmed.
pub fn split_args(attribute: &str) -> Vec<String> {
    let Some(open) = attribute.find('(') else {
        return Vec::new();
    };
    let close = attribute.rfind(')').unwrap_or(attribute.len());
    let inner = &attribute[open + 1..close.max(open + 1)];
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut depth = 0usize;
    for c in inner.chars() {
        match quote {
            Some(q) => {
                cur.push(c);
                // A doubled quote re-enters the quoted state on the next character.
                if c == q {
                    quote = None;
                }
            }
            None => match c {
                '\'' | '"' => {
                    quote = Some(c);
                    cur.push(c);
                }
                '(' | '[' => {
                    depth += 1;
                    cur.push(c);
                }
                ')' | ']' => {
                    depth = depth.saturating_sub(1);
                    cur.push(c);
                }
                ',' if depth == 0 => out.push(std::mem::take(&mut cur).trim().to_string()),
                _ => cur.push(c),
            },
        }
    }
    let last = cur.trim().to_string();
    if !last.is_empty() || !out.is_empty() {
        out.push(last);
    }
    out
}

/// The text of a name argument: `'On Foo'` → `On Foo` (`''` is an
/// apostrophe), `"Name"` → `Name`, a bare identifier as it is.
pub fn unquote(arg: &str) -> String {
    let a = arg.trim();
    if let Some(inner) = a.strip_prefix('\'').and_then(|x| x.strip_suffix('\'')) {
        return inner.replace("''", "'");
    }
    if let Some(inner) = a.strip_prefix('"').and_then(|x| x.strip_suffix('"')) {
        return inner.to_string();
    }
    a.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_at_top_level_commas_only() {
        assert_eq!(
            split_args(
                "[EventSubscriber(ObjectType::Codeunit, Codeunit::\"A, B\", 'On-Check.Value (Qty) & Amt', '', false, false)]"
            ),
            vec![
                "ObjectType::Codeunit",
                "Codeunit::\"A, B\"",
                "'On-Check.Value (Qty) & Amt'",
                "''",
                "false",
                "false"
            ]
        );
        assert_eq!(
            split_args("[IntegrationEvent(false, 'it''s, ok')]"),
            vec!["false", "'it''s, ok'"]
        );
        assert_eq!(split_args("[Test]"), Vec::<String>::new());
        assert_eq!(split_args("[Foo()]"), Vec::<String>::new());
    }
}
