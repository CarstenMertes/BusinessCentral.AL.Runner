//! A workspace is a set of apps. Each app is its manifest plus its sources
//! (path, text). Loading parses every file and builds the object table.

use al_syntax::ast;
use rayon::prelude::*;
use std::collections::{BTreeSet, HashMap};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ObjKind {
    Table,
    TableExtension,
    Page,
    PageExtension,
    Codeunit,
    Report,
    ReportExtension,
    Query,
    XmlPort,
    Enum,
    EnumExtension,
    Interface,
    PermissionSet,
    PermissionSetExtension,
    Profile,
    Other,
}

#[derive(Debug, Clone)]
pub struct AppSource {
    pub id: String,
    pub name: String,
    pub dependencies: Vec<String>,
    pub files: Vec<(String, String)>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Diagnostic {
    pub file: String,
    pub offset: usize,
    pub code: &'static str,
    pub message: String,
}

impl ObjKind {
    pub fn from_keyword(kw: &str) -> ObjKind {
        match kw.to_ascii_lowercase().as_str() {
            "table" => ObjKind::Table,
            "tableextension" => ObjKind::TableExtension,
            "page" => ObjKind::Page,
            "pageextension" => ObjKind::PageExtension,
            "codeunit" => ObjKind::Codeunit,
            "report" => ObjKind::Report,
            "reportextension" => ObjKind::ReportExtension,
            "query" => ObjKind::Query,
            "xmlport" => ObjKind::XmlPort,
            "enum" => ObjKind::Enum,
            "enumextension" => ObjKind::EnumExtension,
            "interface" => ObjKind::Interface,
            "permissionset" => ObjKind::PermissionSet,
            "permissionsetextension" => ObjKind::PermissionSetExtension,
            "profile" => ObjKind::Profile,
            _ => ObjKind::Other,
        }
    }

    /// The object kind a variable type keyword refers to (`Record` → table).
    pub fn from_type_name(ty: &str) -> Option<ObjKind> {
        Some(match ty.to_ascii_lowercase().as_str() {
            "record" => ObjKind::Table,
            "codeunit" => ObjKind::Codeunit,
            "page" | "testpage" => ObjKind::Page,
            "report" | "testrequestpage" => ObjKind::Report,
            "query" => ObjKind::Query,
            "xmlport" => ObjKind::XmlPort,
            "enum" => ObjKind::Enum,
            "interface" => ObjKind::Interface,
            _ => return None,
        })
    }

    /// The kind an extension object extends.
    pub fn extended(self) -> Option<ObjKind> {
        Some(match self {
            ObjKind::TableExtension => ObjKind::Table,
            ObjKind::PageExtension => ObjKind::Page,
            ObjKind::EnumExtension => ObjKind::Enum,
            ObjKind::ReportExtension => ObjKind::Report,
            ObjKind::PermissionSetExtension => ObjKind::PermissionSet,
            _ => return None,
        })
    }

    pub fn label(self) -> &'static str {
        match self {
            ObjKind::Table => "table",
            ObjKind::TableExtension => "tableextension",
            ObjKind::Page => "page",
            ObjKind::PageExtension => "pageextension",
            ObjKind::Codeunit => "codeunit",
            ObjKind::Report => "report",
            ObjKind::ReportExtension => "reportextension",
            ObjKind::Query => "query",
            ObjKind::XmlPort => "xmlport",
            ObjKind::Enum => "enum",
            ObjKind::EnumExtension => "enumextension",
            ObjKind::Interface => "interface",
            ObjKind::PermissionSet => "permissionset",
            ObjKind::PermissionSetExtension => "permissionsetextension",
            ObjKind::Profile => "profile",
            ObjKind::Other => "object",
        }
    }
}

pub struct SourceFileData {
    pub path: String,
    pub app: usize,
    pub green: rowan::GreenNode,
    pub namespace: Option<String>,
    pub usings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolution {
    Found(usize),
    Missing,
    Ambiguous(Vec<usize>),
}

#[derive(Debug, Clone)]
pub struct ObjectSym {
    pub app: usize,
    pub file: usize,
    pub kind: ObjKind,
    pub id: Option<u32>,
    pub name: String,
    pub namespace: Option<String>,
    /// Offset of the object node in its file.
    pub offset: usize,
}

/// A `DotNet` alias declared by a `dotnet { }` block (ADR-014).
#[derive(Debug, Clone)]
pub struct DotNetSym {
    pub app: usize,
    pub file: usize,
    pub decl: ast::DotNetTypeDecl,
}

pub struct Workspace {
    pub apps: Vec<AppSource>,
    pub files: Vec<SourceFileData>,
    pub objects: Vec<ObjectSym>,
    /// Every declared `DotNet` alias, in file and source order. The index is the one
    /// `RtType::DotNet` and `ir::Program::dotnet_types` use.
    pub dotnet: Vec<DotNetSym>,
    pub diagnostics: Vec<Diagnostic>,
    /// For each app, the set of apps it can see (itself + transitive dependencies).
    visible: Vec<BTreeSet<usize>>,
    by_name: HashMap<(ObjKind, String), Vec<usize>>,
    by_id: HashMap<(ObjKind, u32), Vec<usize>>,
}

impl Workspace {
    pub fn load(apps: Vec<AppSource>, symbols: &BTreeSet<String>) -> Workspace {
        let mut ws = Workspace {
            visible: Vec::new(),
            files: Vec::new(),
            objects: Vec::new(),
            dotnet: Vec::new(),
            diagnostics: Vec::new(),
            by_name: HashMap::new(),
            by_id: HashMap::new(),
            apps: Vec::new(),
        };
        ws.visible = (0..apps.len()).map(|i| visible_from(&apps, i)).collect();
        // Parsing dominates loading and each file is independent, so it runs on
        // all cores. `collect` keeps the input order, so the sequential merge
        // below sees files, diagnostics and objects exactly as a serial load would.
        // Expression-valued page properties become routines first (ADR-012).
        let expanded: Vec<Vec<Option<String>>> = apps
            .iter()
            .map(|app| {
                app.files
                    .iter()
                    .map(|(_, src)| crate::pages::expand(src, symbols))
                    .collect()
            })
            .collect();
        let inputs: Vec<(usize, &str, &str)> =
            apps.iter()
                .enumerate()
                .flat_map(|(ai, app)| {
                    let expanded = &expanded[ai];
                    app.files.iter().enumerate().map(move |(fi, (path, src))| {
                        (ai, path.as_str(), expanded[fi].as_deref().unwrap_or(src.as_str()))
                    })
                })
                .collect();
        let parsed: Vec<ParsedFile> = inputs
            .par_iter()
            .map_init(al_syntax::TreeCache::default, |cache, &(_, _, src)| {
                parse_file(src, symbols, cache)
            })
            .collect();
        for ((ai, path, _), file) in inputs.into_iter().zip(parsed) {
            for e in &file.errors {
                ws.diagnostics.push(Diagnostic {
                    file: path.to_string(),
                    offset: e.offset,
                    code: "AR0000",
                    message: e.message.clone(),
                });
            }
            let fi = ws.files.len();
            ws.files.push(SourceFileData {
                path: path.to_string(),
                app: ai,
                green: file.green,
                namespace: file.namespace.clone(),
                usings: file.usings,
            });
            for decl in file.dotnet {
                ws.dotnet.push(DotNetSym {
                    app: ai,
                    file: fi,
                    decl,
                });
            }
            for (kind, id, name, offset) in file.objects {
                ws.add_object(ObjectSym {
                    app: ai,
                    file: fi,
                    kind,
                    id,
                    name,
                    namespace: file.namespace.clone(),
                    offset,
                });
            }
        }
        ws.apps = apps;
        ws.resolve_references();
        ws.diagnostics.sort_by_key(|d| {
            (
                ws.files.iter().position(|f| f.path == d.file).unwrap_or(usize::MAX),
                d.offset,
            )
        });
        ws
    }

    /// The `DotNet` alias `alias` as seen from `file`: the file's own app first, then the apps
    /// it depends on. Aliases are case-insensitive.
    pub fn resolve_dotnet(&self, file: usize, alias: &str) -> Option<usize> {
        let app = self.files[file].app;
        let find = |a: usize| {
            self.dotnet
                .iter()
                .position(|d| d.app == a && d.decl.alias.eq_ignore_ascii_case(alias))
        };
        find(app).or_else(|| self.visible[app].iter().find_map(|&a| find(a)))
    }

    fn add_object(&mut self, o: ObjectSym) {
        let oi = self.objects.len();
        let file = self.files[o.file].path.clone();
        if let Some(id) = o.id {
            let ids = self.by_id.entry((o.kind, id)).or_default();
            if !ids.is_empty() {
                self.diagnostics.push(Diagnostic {
                    file: file.clone(),
                    offset: o.offset,
                    code: "AR0001",
                    message: format!("duplicate {} id {id}", o.kind.label()),
                });
            }
            ids.push(oi);
        }
        // One object of a kind per name in a module, and per name in a namespace.
        let names = self.by_name.entry((o.kind, o.name.to_lowercase())).or_default();
        let clash = names.iter().any(|&j| {
            let other = &self.objects[j];
            other.app == o.app || eq_ns(&other.namespace, &o.namespace)
        });
        if clash {
            self.diagnostics.push(Diagnostic {
                file,
                offset: o.offset,
                code: "AR0001",
                message: format!("duplicate {} name `{}`", o.kind.label(), o.name),
            });
        }
        names.push(oi);
        self.objects.push(o);
    }

    /// How many apps `app` can see (itself and its transitive dependencies). A dependent sees
    /// more than anything it depends on, so this orders dependencies before their dependents.
    pub fn app_depth(&self, app: usize) -> usize {
        self.visible[app].len()
    }

    /// Resolves an object reference written in `file`. `parts` is the name
    /// split into namespace segments, or a single numeric id.
    ///
    /// Qualified: only the named namespace. Unqualified, closest scope first:
    /// the file's namespace, its `using`s, objects without a namespace, then a
    /// unique match in any namespace (corpus evidence: files with neither
    /// namespace nor `using` reference namespaced Base App tables and compile).
    pub fn resolve_ref(&self, file: usize, kind: ObjKind, parts: &[String]) -> Resolution {
        self.resolve_in_scope(file, kind, parts, false)
    }

    /// What a name means at run time (a `TableRelation` target), where BC resolves it against
    /// every installed app: this workspace has no Base Application, so a hit is only certain
    /// when no tier above it could hold a Base Application object of that name. That is the
    /// file's own namespace; the `using`s when the file has no namespace of its own; the global
    /// namespace when it has neither. Anything else is `Missing`, never a guess (corpus fieldref/
    /// TestRelationTargetNamespaceScope.al and TestRelationOwnNamespaceDependency.al: an
    /// imported or global workspace table must not stand in for a Base Application one).
    pub fn resolve_ref_certain(&self, file: usize, kind: ObjKind, parts: &[String]) -> Resolution {
        self.resolve_in_scope(file, kind, parts, true)
    }

    fn resolve_in_scope(&self, file: usize, kind: ObjKind, parts: &[String], certain_only: bool) -> Resolution {
        let f = &self.files[file];
        let visible = |i: &usize| self.visible[f.app].contains(&self.objects[*i].app);
        let Some(name) = parts.last() else {
            return Resolution::Missing;
        };
        if parts.len() == 1 {
            if let Ok(id) = name.parse::<u32>() {
                return pick(
                    self.by_id
                        .get(&(kind, id))
                        .into_iter()
                        .flatten()
                        .copied()
                        .filter(visible)
                        .collect(),
                );
            }
        }
        let cands: Vec<usize> = self
            .by_name
            .get(&(kind, name.to_lowercase()))
            .into_iter()
            .flatten()
            .copied()
            .filter(visible)
            .collect();
        if parts.len() > 1 {
            let ns = Some(parts[..parts.len() - 1].join("."));
            return pick(
                cands
                    .into_iter()
                    .filter(|&i| eq_ns(&self.objects[i].namespace, &ns))
                    .collect(),
            );
        }
        let in_ns = |ns: &Option<String>| {
            cands
                .iter()
                .copied()
                .filter(|&i| eq_ns(&self.objects[i].namespace, ns))
                .collect::<Vec<_>>()
        };
        let no_ns = f.namespace.is_none();
        let tiers = [
            f.namespace
                .as_ref()
                .map(|n| in_ns(&Some(n.clone())))
                .unwrap_or_default(),
            if certain_only && !no_ns {
                Vec::new()
            } else {
                f.usings.iter().flat_map(|u| in_ns(&Some(u.clone()))).collect()
            },
            if certain_only && !(no_ns && f.usings.is_empty()) {
                Vec::new()
            } else {
                in_ns(&None)
            },
            if certain_only { Vec::new() } else { cands.clone() },
        ];
        for tier in tiers {
            if !tier.is_empty() {
                return pick(tier);
            }
        }
        Resolution::Missing
    }

    /// Looks an object up by kind and name (case-insensitive) or by numeric id text.
    pub fn find(&self, kind: ObjKind, name_or_id: &str) -> Option<(String, Option<u32>)> {
        self.lookup(None, kind, name_or_id)
            .map(|i| (self.objects[i].name.clone(), self.objects[i].id))
    }

    /// Resolves a reference as seen from `from_app` (None = from anywhere).
    pub fn lookup(&self, from_app: Option<usize>, kind: ObjKind, name_or_id: &str) -> Option<usize> {
        let visible = |i: &&usize| from_app.is_none_or(|a| self.visible[a].contains(&self.objects[**i].app));
        let hits = match name_or_id.parse::<u32>() {
            Ok(id) => self.by_id.get(&(kind, id)),
            Err(_) => self.by_name.get(&(kind, name_or_id.to_lowercase())),
        };
        hits?.iter().find(visible).copied()
    }

    /// The syntax node of object `i`.
    pub fn object_ast(&self, i: usize) -> ast::Object {
        self.object_node(&self.objects[i])
            .expect("object node at recorded offset")
    }

    /// Objects are direct children of the file's root, so look there rather than
    /// walking every node of the file.
    fn object_node(&self, o: &ObjectSym) -> Option<ast::Object> {
        let root = al_syntax::SyntaxNode::new_root(self.files[o.file].green.clone());
        root.children()
            .filter_map(ast::Object::cast)
            .find(|n| usize::from(n.syntax().text_range().start()) == o.offset)
    }

    fn resolve_references(&mut self) {
        let mut diags = Vec::new();
        for o in &self.objects {
            let file = &self.files[o.file];
            let Some(obj) = self.object_node(o) else {
                continue;
            };
            let node = obj.syntax().clone();
            let mut check = |kind: ObjKind, parts: &[String], offset: usize| {
                let name = parts.join(".");
                let (code, message) = match self.resolve_ref(o.file, kind, parts) {
                    Resolution::Found(_) => return,
                    Resolution::Missing => ("AR0002", format!("unresolved {} `{name}`", kind.label())),
                    Resolution::Ambiguous(c) => (
                        "AR0003",
                        format!("ambiguous {} `{name}` ({} candidates)", kind.label(), c.len()),
                    ),
                };
                diags.push(Diagnostic {
                    file: file.path.clone(),
                    offset,
                    code,
                    message,
                });
            };
            let header_off = o.offset;
            if let (Some(target), Some(base)) = (o.kind.extended(), obj.extends_parts()) {
                check(target, &base, header_off);
            }
            for i in obj.implements_parts() {
                check(ObjKind::Interface, &i, header_off);
            }
            for t in node.descendants().filter_map(ast::TypeRef::cast) {
                if let Some(kind) = ObjKind::from_type_name(&t.type_name()) {
                    let parts = t.object_name_parts();
                    if !parts.is_empty() {
                        check(kind, &parts, t.syntax().text_range().start().into());
                    }
                }
            }
        }
        self.diagnostics.extend(diags);
    }
}

/// What loading needs from one parsed file; plain data, so it can be produced
/// on a worker thread (syntax nodes cannot cross threads, green trees can).
struct ParsedFile {
    green: rowan::GreenNode,
    errors: Vec<al_syntax::ParseError>,
    namespace: Option<String>,
    usings: Vec<String>,
    /// (kind, id, name, offset) of each object, in source order.
    objects: Vec<(ObjKind, Option<u32>, String, usize)>,
    /// The `type(..)` declarations of the file's `dotnet { }` blocks.
    dotnet: Vec<ast::DotNetTypeDecl>,
}

fn parse_file(src: &str, symbols: &BTreeSet<String>, cache: &mut al_syntax::TreeCache) -> ParsedFile {
    let parse = al_syntax::parse_with_cache(src, symbols, cache);
    let root = ast::SourceFile::cast(parse.syntax()).expect("root is a SourceFile");
    ParsedFile {
        namespace: root.namespace(),
        usings: root.usings(),
        objects: root
            .objects()
            .map(|o| {
                (
                    ObjKind::from_keyword(&o.kind()),
                    o.id(),
                    o.name(),
                    o.syntax().text_range().start().into(),
                )
            })
            .collect(),
        dotnet: root.dotnet_types(),
        green: parse.green,
        errors: parse.errors,
    }
}

fn eq_ns(a: &Option<String>, b: &Option<String>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => a.eq_ignore_ascii_case(b),
        _ => false,
    }
}

fn pick(c: Vec<usize>) -> Resolution {
    match c.len() {
        0 => Resolution::Missing,
        1 => Resolution::Found(c[0]),
        _ => Resolution::Ambiguous(c),
    }
}

fn visible_from(apps: &[AppSource], start: usize) -> BTreeSet<usize> {
    let mut seen = BTreeSet::new();
    let mut stack = vec![start];
    while let Some(i) = stack.pop() {
        if seen.insert(i) {
            for d in &apps[i].dependencies {
                if let Some(j) = apps.iter().position(|a| a.id.eq_ignore_ascii_case(d)) {
                    stack.push(j);
                }
            }
        }
    }
    seen
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(id: &str, deps: &[&str], files: &[(&str, &str)]) -> AppSource {
        AppSource {
            id: id.into(),
            name: id.into(),
            dependencies: deps.iter().map(|d| d.to_string()).collect(),
            files: files.iter().map(|(p, s)| (p.to_string(), s.to_string())).collect(),
        }
    }

    fn load(apps: Vec<AppSource>) -> Workspace {
        Workspace::load(apps, &al_version::corpus_symbols(27))
    }

    fn codes(ws: &Workspace) -> Vec<(&'static str, String)> {
        ws.diagnostics.iter().map(|d| (d.code, d.message.clone())).collect()
    }

    #[test]
    fn run_time_resolution_only_trusts_tiers_no_absent_app_could_shadow() {
        // corpus fieldref/TestRelationOwnNamespaceDependency.al, TestRelationTargetNamespaceScope.al:
        // a workspace table must not stand in for a Base Application one that is not here.
        let ws = load(vec![app(
            "a",
            &[],
            &[
                ("t.al", "namespace Imported; table 50900 \"Agent\" { }"),
                ("g.al", "table 50901 \"Global Agent\" { }"),
                ("own.al", "namespace Own; table 50902 \"Own Agent\" { }"),
                ("c1.al", "namespace Own; using Imported; codeunit 60100 C1 { }"),
                ("c2.al", "using Imported; codeunit 60101 C2 { }"),
                ("c3.al", "codeunit 60102 C3 { }"),
                ("c4.al", "namespace Other; codeunit 60103 C4 { }"),
            ],
        )]);
        let file = |name: &str| ws.objects.iter().find(|o| o.name == name).unwrap().file;
        let found = |r: Resolution| matches!(r, Resolution::Found(_));
        let one = |n: &str| vec![n.to_string()];
        // The ordinary compile-time resolution finds each of them.
        assert!(found(ws.resolve_ref(file("C1"), ObjKind::Table, &one("Agent"))));
        // Own namespace: certain.
        assert!(found(ws.resolve_ref_certain(
            file("C1"),
            ObjKind::Table,
            &one("Own Agent")
        )));
        // A `using` hit under a namespaced file could be shadowed by a Base Application table of the
        // file's own namespace: not certain. Without a namespace of its own it is.
        assert!(!found(ws.resolve_ref_certain(
            file("C1"),
            ObjKind::Table,
            &one("Agent")
        )));
        assert!(found(ws.resolve_ref_certain(file("C2"), ObjKind::Table, &one("Agent"))));
        // A global table is only certain for a file with neither namespace nor usings.
        assert!(!found(ws.resolve_ref_certain(
            file("C2"),
            ObjKind::Table,
            &one("Global Agent")
        )));
        assert!(found(ws.resolve_ref_certain(
            file("C3"),
            ObjKind::Table,
            &one("Global Agent")
        )));
        assert!(!found(ws.resolve_ref_certain(
            file("C4"),
            ObjKind::Table,
            &one("Global Agent")
        )));
        // Qualified names are exact.
        assert!(found(ws.resolve_ref_certain(
            file("C4"),
            ObjKind::Table,
            &["Imported".to_string(), "Agent".to_string()]
        )));
    }

    #[test]
    fn files_objects_and_diagnostics_keep_input_order_with_parallel_parsing() {
        // Many files so that rayon really splits the work; a parse error and a
        // duplicate id are planted in the middle and must be reported in file order.
        let mut files: Vec<(String, String)> = (0..200)
            .map(|i| (format!("f{i:03}.al"), format!("codeunit {} C{i} {{ }}", 50000 + i)))
            .collect();
        files[100].1 = "codeunit 50100 C100 { procedure ( }".into();
        files[150].1 = "codeunit 50007 Dup { }".into();
        let a = AppSource {
            id: "a".into(),
            name: "a".into(),
            dependencies: vec![],
            files,
        };
        let b = app("b", &["a"], &[("z.al", "codeunit 60000 Z { }")]);
        let ws = load(vec![a.clone(), b.clone()]);
        let paths: Vec<_> = ws.files.iter().map(|f| f.path.as_str()).collect();
        let want: Vec<String> = a.files.iter().chain(&b.files).map(|f| f.0.clone()).collect();
        assert_eq!(paths, want.iter().map(String::as_str).collect::<Vec<_>>());
        let names: Vec<_> = ws.objects.iter().map(|o| o.name.as_str()).collect();
        assert_eq!(names[..3], ["C0", "C1", "C2"]);
        assert_eq!(names.last(), Some(&"Z"));
        let diag_files: Vec<_> = ws.diagnostics.iter().map(|d| d.file.as_str()).collect();
        let mut sorted = diag_files.clone();
        sorted.sort();
        assert_eq!(diag_files, sorted, "diagnostics come in file order");
        assert!(diag_files.contains(&"f100.al") && diag_files.contains(&"f150.al"));
        // Loading is deterministic.
        let again = load(vec![a, b]);
        assert_eq!(codes(&ws), codes(&again));
        assert_eq!(
            ws.objects.iter().map(|o| (o.file, o.offset)).collect::<Vec<_>>(),
            again.objects.iter().map(|o| (o.file, o.offset)).collect::<Vec<_>>()
        );
    }

    #[test]
    fn finds_objects_by_name_case_insensitively_and_by_id() {
        let ws = load(vec![app("a", &[], &[("t.al", "table 50900 \"ALT Universal\" { }")])]);
        assert_eq!(
            ws.find(ObjKind::Table, "alt universal"),
            Some(("ALT Universal".into(), Some(50900)))
        );
        assert_eq!(
            ws.find(ObjKind::Table, "50900"),
            Some(("ALT Universal".into(), Some(50900)))
        );
        assert_eq!(ws.find(ObjKind::Codeunit, "ALT Universal"), None);
    }

    #[test]
    fn resolved_type_references_produce_no_diagnostics() {
        let ws = load(vec![app(
            "a",
            &[],
            &[
                ("t.al", "table 50900 \"ALT Universal\" { }"),
                (
                    "c.al",
                    "codeunit 60100 T { var R: Record \"ALT Universal\"; L: List of [Integer]; procedure P(x: Codeunit T) begin end; }",
                ),
            ],
        )]);
        assert_eq!(codes(&ws), vec![]);
    }

    #[test]
    fn unresolved_object_reference_is_diagnosed() {
        let ws = load(vec![app(
            "a",
            &[],
            &[("c.al", "codeunit 1 C { var R: Record Customer; }")],
        )]);
        assert_eq!(codes(&ws), vec![("AR0002", "unresolved table `Customer`".to_string())]);
    }

    #[test]
    fn references_inside_routines_and_nested_type_args_are_checked() {
        let ws = load(vec![app(
            "a",
            &[],
            &[(
                "c.al",
                "codeunit 1 C { procedure P(): Record X var L: List of [Codeunit Y]; begin end; }",
            )],
        )]);
        assert_eq!(
            codes(&ws),
            vec![
                ("AR0002", "unresolved table `X`".to_string()),
                ("AR0002", "unresolved codeunit `Y`".to_string())
            ]
        );
    }

    #[test]
    fn objects_of_a_dependency_are_visible_but_not_the_reverse() {
        let ws = load(vec![
            app("base", &[], &[("t.al", "table 1 T { var C: Codeunit Top; }")]),
            app("top", &["base"], &[("c.al", "codeunit 2 Top { var R: Record T; }")]),
        ]);
        assert_eq!(codes(&ws), vec![("AR0002", "unresolved codeunit `Top`".to_string())]);
    }

    #[test]
    fn duplicate_object_ids_and_names_are_diagnosed() {
        let ws = load(vec![app(
            "a",
            &[],
            &[("x.al", "codeunit 1 A { }\ncodeunit 1 B { }\ncodeunit 2 a { }")],
        )]);
        assert_eq!(
            codes(&ws),
            vec![
                ("AR0001", "duplicate codeunit id 1".to_string()),
                ("AR0001", "duplicate codeunit name `a`".to_string())
            ]
        );
    }

    #[test]
    fn extends_and_implements_are_resolved() {
        let ws = load(vec![app(
            "a",
            &[],
            &[(
                "x.al",
                "interface I { }\nenum 1 E implements I, J { }\ntableextension 2 X extends Missing { }",
            )],
        )]);
        assert_eq!(
            codes(&ws),
            vec![
                ("AR0002", "unresolved interface `J`".to_string()),
                ("AR0002", "unresolved table `Missing`".to_string())
            ]
        );
    }

    #[test]
    fn namespace_qualified_reference_resolves_in_that_namespace() {
        let ws = load(vec![app(
            "a",
            &[],
            &[
                ("b.al", "namespace System.Utilities;\ncodeunit 1 \"Temp Blob\" { }"),
                (
                    "u.al",
                    "codeunit 2 U { var B: Codeunit System.Utilities.\"Temp Blob\"; }",
                ),
            ],
        )]);
        assert_eq!(codes(&ws), vec![]);
    }

    #[test]
    fn qualified_reference_does_not_fall_back_to_an_object_without_namespace() {
        let ws = load(vec![app(
            "a",
            &[],
            &[(
                "x.al",
                "codeunit 1 \"Temp Blob\" { }\ncodeunit 2 U { var B: Codeunit System.Utilities.\"Temp Blob\"; }",
            )],
        )]);
        assert_eq!(
            codes(&ws),
            vec![("AR0002", "unresolved codeunit `System.Utilities.Temp Blob`".to_string())]
        );
    }

    #[test]
    fn namespace_qualified_reference_to_a_name_containing_a_dot() {
        let ws = load(vec![app(
            "a",
            &[],
            &[
                ("t.al", "namespace A.B;\ntable 1 \"Config. Package Table\" { }"),
                ("u.al", "codeunit 2 U { var R: Record A.B.\"Config. Package Table\"; }"),
            ],
        )]);
        assert_eq!(codes(&ws), vec![]);
    }

    fn resolved(ws: &Workspace, file: &str, kind: ObjKind, parts: &[&str]) -> Option<(Option<String>, u32)> {
        let fi = ws.files.iter().position(|f| f.path == file).unwrap();
        let parts: Vec<String> = parts.iter().map(|p| p.to_string()).collect();
        match ws.resolve_ref(fi, kind, &parts) {
            Resolution::Found(i) => Some((ws.objects[i].namespace.clone(), ws.objects[i].id.unwrap())),
            _ => None,
        }
    }

    fn ns_app() -> Workspace {
        load(vec![app(
            "a",
            &[],
            &[
                (
                    "local.al",
                    "namespace X.Local;\ntable 1 \"Ship\" { }\ntable 4 \"Only Local\" { }",
                ),
                ("base.al", "namespace X.Base;\ntable 2 \"Ship\" { }"),
                ("global.al", "table 3 \"Glob\" { }"),
                ("user_local.al", "namespace X.Local;\ncodeunit 10 A { }"),
                ("user_using.al", "namespace X.Other;\nusing X.Base;\ncodeunit 11 B { }"),
                ("user_none.al", "codeunit 12 C { }"),
            ],
        )])
    }

    #[test]
    fn qualified_reference_resolves_only_in_the_named_namespace() {
        let ws = ns_app();
        assert_eq!(
            resolved(&ws, "user_none.al", ObjKind::Table, &["X", "Base", "Ship"]),
            Some((Some("X.Base".into()), 2))
        );
        assert_eq!(
            resolved(&ws, "user_none.al", ObjKind::Table, &["X", "Local", "Ship"]),
            Some((Some("X.Local".into()), 1))
        );
        assert_eq!(
            resolved(&ws, "user_none.al", ObjKind::Table, &["X", "Base", "Only Local"]),
            None
        );
    }

    #[test]
    fn unqualified_reference_prefers_the_own_namespace_then_usings() {
        let ws = ns_app();
        assert_eq!(
            resolved(&ws, "user_local.al", ObjKind::Table, &["Ship"]),
            Some((Some("X.Local".into()), 1))
        );
        assert_eq!(
            resolved(&ws, "user_using.al", ObjKind::Table, &["Ship"]),
            Some((Some("X.Base".into()), 2))
        );
    }

    #[test]
    fn unqualified_reference_falls_back_to_a_unique_match_anywhere() {
        // Corpus evidence: files without namespace or using reference Base App
        // tables such as `Record Customer` (namespace Microsoft.Sales.Customer) and compile.
        let ws = ns_app();
        assert_eq!(
            resolved(&ws, "user_none.al", ObjKind::Table, &["Glob"]),
            Some((None, 3))
        );
        assert_eq!(
            resolved(&ws, "user_none.al", ObjKind::Table, &["Only Local"]),
            Some((Some("X.Local".into()), 4))
        );
    }

    #[test]
    fn unqualified_reference_matching_several_namespaces_is_ambiguous() {
        let ws = ns_app();
        let fi = ws.files.iter().position(|f| f.path == "user_none.al").unwrap();
        assert!(matches!(
            ws.resolve_ref(fi, ObjKind::Table, &["Ship".to_string()]),
            Resolution::Ambiguous(_)
        ));
    }

    #[test]
    fn qualified_extends_resolves_by_namespace() {
        let ws = load(vec![app(
            "a",
            &[],
            &[
                ("p.al", "namespace N.Local;\npage 1 \"Svc\" { }"),
                (
                    "e.al",
                    "pageextension 2 E1 extends N.Local.\"Svc\" { }\npageextension 3 E2 extends N.Other.\"Svc\" { }",
                ),
            ],
        )]);
        assert_eq!(
            codes(&ws),
            vec![("AR0002", "unresolved page `N.Other.Svc`".to_string())]
        );
    }

    #[test]
    fn parse_errors_are_reported_as_diagnostics() {
        let ws = load(vec![app("a", &[], &[("x.al", "codeunit 1 A { 42 }")])]);
        assert_eq!(ws.diagnostics.len(), 1);
        assert_eq!(ws.diagnostics[0].code, "AR0000");
    }
}
