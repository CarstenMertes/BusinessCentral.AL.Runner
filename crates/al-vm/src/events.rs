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

#[derive(Debug)]
pub struct Registry {
    /// (publisher object, lower-case event name) → subscribers, in program order.
    pub subscribers: HashMap<(usize, String), Vec<Subscriber>>,
    /// (object, routine) → the event the routine publishes.
    pub publishers: HashMap<(usize, usize), Publisher>,
    /// The object key under which subscribers to the platform codeunit `Global Triggers`
    /// (2000000002) are registered: the codeunit itself when a loaded app declares it
    /// (the platform `System.app`), else [`GLOBAL_TRIGGERS`].
    pub global_key: usize,
    /// Whether any subscriber of `Global Triggers` exists.
    pub has_global_triggers: bool,
    /// Subscribers (object, routine) of `Global Triggers` in a dependency app that cannot run
    /// (they reach something refused, such as a platform table with no provider). The host that
    /// sets this must report as not run every test that could observe what they would do; the
    /// events skip them (al-test-cli `runtime.rs`, ADR-013 as built, "bridge").
    pub unavailable_global: std::collections::HashSet<(usize, usize)>,
}

impl Default for Registry {
    fn default() -> Self {
        Registry {
            subscribers: HashMap::new(),
            publishers: HashMap::new(),
            global_key: GLOBAL_TRIGGERS,
            has_global_triggers: false,
            unavailable_global: Default::default(),
        }
    }
}

fn is_manual(o: &ir::Object) -> bool {
    o.properties
        .iter()
        .any(|(k, v)| k.eq_ignore_ascii_case("EventSubscriberInstance") && v.trim().eq_ignore_ascii_case("Manual"))
}

fn flag(args: &[String], i: usize) -> bool {
    args.get(i).is_some_and(|a| a.eq_ignore_ascii_case("true"))
}

/// The object key under which subscribers to the Caption Class codeunit's OnResolveCaptionClass
/// are registered (the codeunit itself is not in the workspace).
pub const CAPTION_CLASS: usize = usize::MAX;

/// The object key under which subscribers to the platform codeunit `Global Triggers` are
/// registered while no loaded app declares that codeunit. Its events have no body: the runtime
/// raises them itself (`globaltriggers.rs`).
pub const GLOBAL_TRIGGERS: usize = usize::MAX - 1;

fn is_global_triggers(target: &str) -> bool {
    target.eq_ignore_ascii_case("Global Triggers") || target == "2000000002"
}

impl Registry {
    pub fn build(program: &ir::Program) -> Registry {
        let mut reg = Registry::default();
        // Events declared in a table or page extension are subscribed to on the base object
        // (corpus extensionevents/TestExtensionDeclaredEvents.al:92 PageExtEvent_SubscriberBoundToBasePage_...).
        let mut base_of: HashMap<usize, usize> = HashMap::new();
        for (oi, o) in program.objects.iter().enumerate() {
            if let Some(x) = &o.report_ext {
                base_of.insert(oi, x.base);
            }
            if let Some(t) = &o.table {
                for &e in &t.extensions {
                    base_of.insert(e, oi);
                }
            }
            if let Some(p) = &o.page {
                for &e in &p.extensions {
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
        if let Some(&gt) = by_name
            .get(&("codeunit", "global triggers".to_string()))
            .or_else(|| by_id.get(&("codeunit", 2_000_000_002)))
        {
            reg.global_key = gt;
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
                                // The System Application's Caption Class codeunit is not in the workspace:
                                // its subscribers are kept under a key of their own for the page client.
                                None if kind == "codeunit"
                                    && (target.eq_ignore_ascii_case("Caption Class") || target == "42")
                                    && event == "onresolvecaptionclass" =>
                                {
                                    reg.subscribers
                                        .entry((CAPTION_CLASS, event))
                                        .or_default()
                                        .push(Subscriber {
                                            object: oi,
                                            routine: ri,
                                            manual: is_manual(o),
                                            element: String::new(),
                                        })
                                }
                                None if kind == "codeunit" && is_global_triggers(&target) => {
                                    reg.has_global_triggers = true;
                                    reg.subscribers
                                        .entry((reg.global_key, event))
                                        .or_default()
                                        .push(Subscriber {
                                            object: oi,
                                            routine: ri,
                                            manual: is_manual(o),
                                            element: String::new(),
                                        })
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
