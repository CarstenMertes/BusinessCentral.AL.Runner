//! A workspace is a set of apps. Each app is its manifest plus its sources
//! (path, text). Loading parses every file and builds the object table.

use al_syntax::ast;
use rayon::prelude::*;
use std::collections::{BTreeSet, HashMap};
use std::sync::OnceLock;

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

/// Whether an app is the code being tested or a dependency it was compiled against.
/// No test is discovered or run in a dependency (ADR-013 decision 4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AppKind {
    #[default]
    Workspace,
    Dependency,
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
    /// Position of the file in `apps[app].files`.
    pub file_in_app: usize,
    /// The parse tree. A file of a dependency app is only scanned at load and parsed
    /// the first time something needs it (`Workspace::green`).
    green: OnceLock<rowan::GreenNode>,
    /// For a lazily parsed file: why its objects cannot be used, if its source does not
    /// parse cleanly (the first parse error, with its line).
    problem: OnceLock<Option<String>>,
    /// Whether the file was only scanned at load (a dependency app's file).
    pub lazy: bool,
    /// Whether the file declares a page, the only objects `pages::expand` changes.
    has_page: bool,
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
    /// Offset of the object node in its file (in the original text for a scanned file).
    pub offset: usize,
    /// Position among the objects of its file, which finds its node after a (lazy) parse.
    pub ordinal: usize,
    /// `extends X` of an extension object, as name parts.
    pub extends: Option<Vec<String>>,
    pub implements: Vec<Vec<String>>,
    /// Event subscriptions it declares, for objects of scanned files.
    pub subscribers: Vec<al_syntax::scan::ScannedSubscriber>,
}

pub struct Workspace {
    pub apps: Vec<AppSource>,
    pub files: Vec<SourceFileData>,
    pub objects: Vec<ObjectSym>,
    pub diagnostics: Vec<Diagnostic>,
    /// The kind of each app, by index in `apps`.
    pub kinds: Vec<AppKind>,
    /// For each app, the set of apps it can see (itself + transitive dependencies).
    visible: Vec<BTreeSet<usize>>,
    by_name: HashMap<(ObjKind, String), Vec<usize>>,
    by_id: HashMap<(ObjKind, u32), Vec<usize>>,
    /// The preprocessor symbols, for parsing files later.
    symbols: BTreeSet<String>,
    /// For an extension object, the object it extends (found without parsing its target).
    ext_target: Vec<Option<usize>>,
    /// For an extended object, its extensions in object order.
    ext_of: HashMap<usize, Vec<usize>>,
    /// Event subscribers of scanned objects by publisher: (kind, lower-case name or id) to
    /// (subscriber object, index in its `subscribers`).
    subscribers_of: HashMap<(ObjKind, String), Vec<(usize, usize)>>,
    /// Scanned objects that implement an interface, by lower-case interface name.
    implementers_of: HashMap<String, Vec<usize>>,
    /// Per object: whether it can be used (see `usable`); filled on first ask.
    usable_cache: Vec<OnceLock<bool>>,
    /// Once set, the dependency objects that take part in the compile (see `restrict_to`).
    compiled: OnceLock<Vec<bool>>,
}

impl Workspace {
    pub fn load(apps: Vec<AppSource>, symbols: &BTreeSet<String>) -> Workspace {
        let kinds = vec![AppKind::Workspace; apps.len()];
        Workspace::load_apps(apps, kinds, symbols)
    }

    /// Like `load`, with the kind of each app. The files of a dependency app are not parsed
    /// here: a cheap scan indexes their objects, and a file is parsed when something needs
    /// one of its objects (ADR-013 decision 6).
    pub fn load_apps(apps: Vec<AppSource>, kinds: Vec<AppKind>, symbols: &BTreeSet<String>) -> Workspace {
        assert_eq!(apps.len(), kinds.len(), "one kind per app");
        let mut ws = Workspace {
            kinds,
            visible: Vec::new(),
            files: Vec::new(),
            objects: Vec::new(),
            diagnostics: Vec::new(),
            by_name: HashMap::new(),
            by_id: HashMap::new(),
            apps: Vec::new(),
            symbols: symbols.clone(),
            ext_target: Vec::new(),
            ext_of: HashMap::new(),
            subscribers_of: HashMap::new(),
            implementers_of: HashMap::new(),
            usable_cache: Vec::new(),
            compiled: OnceLock::new(),
        };
        ws.visible = (0..apps.len()).map(|i| visible_from(&apps, i)).collect();
        // Parsing dominates loading and each file is independent, so it runs on
        // all cores. `collect` keeps the input order, so the sequential merge
        // below sees files, diagnostics and objects exactly as a serial load would.
        // Expression-valued page properties become routines first (ADR-012).
        let inputs: Vec<(usize, usize, bool, &str, &str)> = apps
            .iter()
            .enumerate()
            .flat_map(|(ai, app)| {
                let lazy = ws.kinds[ai] == AppKind::Dependency;
                app.files
                    .iter()
                    .enumerate()
                    .map(move |(fi, (path, src))| (ai, fi, lazy, path.as_str(), src.as_str()))
            })
            .collect();
        let parsed: Vec<ParsedFile> = inputs
            .par_iter()
            .map_init(al_syntax::TreeCache::default, |cache, &(_, _, lazy, _, src)| {
                if lazy {
                    scanned_file(src, symbols)
                } else {
                    let expanded = crate::pages::expand(src, symbols);
                    parse_file(expanded.as_deref().unwrap_or(src), symbols, cache)
                }
            })
            .collect();
        for ((ai, file_in_app, lazy, path, _), file) in inputs.into_iter().zip(parsed) {
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
                file_in_app,
                green: file.green.map(OnceLock::from).unwrap_or_default(),
                problem: OnceLock::new(),
                lazy,
                has_page: file.objects.iter().any(|o| o.kind == ObjKind::Page),
                namespace: file.namespace.clone(),
                usings: file.usings,
            });
            for (ordinal, o) in file.objects.into_iter().enumerate() {
                ws.add_object(ObjectSym {
                    app: ai,
                    file: fi,
                    kind: o.kind,
                    id: o.id,
                    name: o.name,
                    namespace: file.namespace.clone(),
                    offset: o.offset,
                    ordinal,
                    extends: o.extends,
                    implements: o.implements,
                    subscribers: o.subscribers,
                });
            }
        }
        ws.apps = apps;
        ws.usable_cache = (0..ws.objects.len()).map(|_| OnceLock::new()).collect();
        ws.index_extensions_and_subscribers();
        ws.resolve_references();
        let positions: HashMap<&str, usize> = ws.files.iter().enumerate().map(|(i, f)| (f.path.as_str(), i)).collect();
        let key = |d: &Diagnostic| (positions.get(d.file.as_str()).copied().unwrap_or(usize::MAX), d.offset);
        let mut diagnostics = std::mem::take(&mut ws.diagnostics);
        diagnostics.sort_by_key(key);
        ws.diagnostics = diagnostics;
        ws
    }

    /// Extension targets and event subscribers, found from the object index alone.
    fn index_extensions_and_subscribers(&mut self) {
        self.ext_target = vec![None; self.objects.len()];
        for i in 0..self.objects.len() {
            let o = &self.objects[i];
            if let (Some(kind), Some(parts)) = (o.kind.extended(), &o.extends) {
                // Not `resolve_ref`: that would parse the target's file, and an index of
                // thousands of extensions must not parse their targets.
                if let Resolution::Found(t) = self.resolve_in_scope(o.file, kind, parts, false, false) {
                    self.ext_target[i] = Some(t);
                    self.ext_of.entry(t).or_default().push(i);
                }
            }
        }
        for (i, o) in self.objects.iter().enumerate() {
            for (si, sub) in o.subscribers.iter().enumerate() {
                let kind = ObjKind::from_keyword(&sub.publisher_kind);
                let name = sub.publisher.join(".").to_lowercase();
                self.subscribers_of.entry((kind, name)).or_default().push((i, si));
            }
            for imp in &o.implements {
                if let Some(last) = imp.last() {
                    self.implementers_of.entry(last.to_lowercase()).or_default().push(i);
                }
            }
        }
    }

    /// The object `i` extends, if it is an extension and the target is in the workspace.
    pub fn extension_target(&self, i: usize) -> Option<usize> {
        self.ext_target[i]
    }

    /// The extensions of object `i`, in object order.
    pub fn extensions_of(&self, i: usize) -> &[usize] {
        self.ext_of.get(&i).map_or(&[], Vec::as_slice)
    }

    /// Subscribers (object, index in its `subscribers`) of scanned objects that name `publisher`
    /// as their publisher, by name or by id.
    pub fn subscribers_of(&self, publisher: usize) -> Vec<(usize, usize)> {
        let o = &self.objects[publisher];
        let mut keys = vec![o.name.to_lowercase()];
        if let Some(id) = o.id {
            keys.push(id.to_string());
        }
        // `Codeunit::Microsoft.Foo."Name"` writes the namespace; the index key is the written text.
        if let Some(ns) = &o.namespace {
            keys.push(format!("{ns}.{}", o.name).to_lowercase());
        }
        keys.into_iter()
            .flat_map(|k| self.subscribers_of.get(&(o.kind, k)).into_iter().flatten().copied())
            .collect()
    }

    /// Objects of scanned files that list `interface` (by its last name part) in `implements`.
    pub fn implementers_of(&self, interface: &str) -> &[usize] {
        self.implementers_of
            .get(&interface.to_lowercase())
            .map_or(&[], Vec::as_slice)
    }

    /// The parse tree of file `fi`, parsing it now if it was only scanned at load.
    pub fn green(&self, fi: usize) -> rowan::GreenNode {
        let f = &self.files[fi];
        f.green
            .get_or_init(|| {
                thread_local! {
                    static CACHE: std::cell::RefCell<al_syntax::TreeCache> = Default::default();
                }
                let src = &self.apps[f.app].files[f.file_in_app].1;
                // Only a page can have properties to turn into routines; skip the extra parse otherwise.
                let expanded = f.has_page.then(|| crate::pages::expand(src, &self.symbols)).flatten();
                let src = expanded.as_deref().unwrap_or(src);
                let parsed = CACHE.with(|c| match c.try_borrow_mut() {
                    Ok(mut cache) => parse_file(src, &self.symbols, &mut cache),
                    Err(_) => parse_file(src, &self.symbols, &mut al_syntax::TreeCache::default()),
                });
                let problem = parsed.errors.first().map(|e| {
                    let line = src[..e.offset.min(src.len())].matches('\n').count() + 1;
                    let text = src.lines().nth(line - 1).unwrap_or("").trim();
                    format!("{}:{line}: {} (`{text}`)", f.path, e.message)
                });
                let _ = f.problem.set(problem);
                parsed.green.expect("a parsed file has a tree")
            })
            .clone()
    }

    /// Why the objects of file `fi` cannot be used: its source does not parse cleanly.
    /// Only a scanned (dependency) file can have such a problem; the workspace's own files
    /// report parse errors as diagnostics. Parses the file if that has not happened yet.
    pub fn file_problem(&self, fi: usize) -> Option<&str> {
        let f = &self.files[fi];
        if !f.lazy {
            return None;
        }
        self.green(fi);
        f.problem.get().and_then(|p| p.as_deref())
    }

    /// From now on only the dependency objects marked in `compiled` resolve. The checker has
    /// decided which of them the workspace's code reaches; the others are not compiled, so
    /// code that needs one anyway is refused as outside the workspace instead of finding it
    /// half there. Set once; a second call changes nothing.
    pub fn restrict_to(&self, compiled: Vec<bool>) {
        let _ = self.compiled.set(compiled);
    }

    /// Whether object `i` takes part in the compile: not a dependency's, or not left out by `restrict_to`.
    fn in_compile(&self, i: usize) -> bool {
        match self.compiled.get() {
            Some(c) => c[i] || self.kinds[self.objects[i].app] != AppKind::Dependency,
            None => true,
        }
    }

    /// Whether object `i` can be used: its file is a workspace file, or parses cleanly.
    pub fn usable(&self, i: usize) -> bool {
        let o = &self.objects[i];
        if !self.files[o.file].lazy {
            return true;
        }
        *self.usable_cache[i].get_or_init(|| self.file_problem(o.file).is_none() && self.object_node(o).is_some())
    }

    /// Why object `i` is unusable, for messages.
    pub fn unusable_reason(&self, i: usize) -> Option<String> {
        let fi = self.objects[i].file;
        match self.file_problem(fi) {
            Some(p) => Some(format!("dependency source does not parse: {p}")),
            None if self.files[fi].lazy && self.object_node(&self.objects[i]).is_none() => Some(format!(
                "{}: the object was not found after parsing",
                self.files[fi].path
            )),
            None => None,
        }
    }

    /// The files that were parsed after load and did not parse cleanly, as (path, message).
    pub fn lazy_parse_problems(&self) -> Vec<(String, String)> {
        self.files
            .iter()
            .filter_map(|f| Some((f.path.clone(), f.problem.get()?.clone()?)))
            .collect()
    }

    fn add_object(&mut self, o: ObjectSym) {
        let oi = self.objects.len();
        let file = self.files[o.file].path.clone();
        // The AL compiler has already vetted a dependency app; its clashes are not ours to report.
        let report = self.kinds[o.app] != AppKind::Dependency;
        if let Some(id) = o.id {
            let ids = self.by_id.entry((o.kind, id)).or_default();
            if !ids.is_empty() && report {
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
        if clash && report {
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

    /// Resolves an object reference written in `file`. `parts` is the name
    /// split into namespace segments, or a single numeric id.
    ///
    /// Qualified: only the named namespace. Unqualified, closest scope first:
    /// the file's namespace, its `using`s, objects without a namespace, then a
    /// unique match in any namespace (corpus evidence: files with neither
    /// namespace nor `using` reference namespaced Base App tables and compile).
    pub fn resolve_ref(&self, file: usize, kind: ObjKind, parts: &[String]) -> Resolution {
        self.resolve_in_scope(file, kind, parts, false, true)
    }

    /// What a name means at run time (a `TableRelation` target), where BC resolves it against
    /// every installed app: this workspace has no Base Application, so a hit is only certain
    /// when no tier above it could hold a Base Application object of that name. That is the
    /// file's own namespace; the `using`s when the file has no namespace of its own; the global
    /// namespace when it has neither. Anything else is `Missing`, never a guess (corpus fieldref/
    /// TestRelationTargetNamespaceScope.al and TestRelationOwnNamespaceDependency.al: an
    /// imported or global workspace table must not stand in for a Base Application one).
    pub fn resolve_ref_certain(&self, file: usize, kind: ObjKind, parts: &[String]) -> Resolution {
        self.resolve_in_scope(file, kind, parts, true, true)
    }

    /// `check_usable`: leave out objects of a dependency file that does not parse. That parses
    /// the file of every candidate, which the extension index must not trigger.
    fn resolve_in_scope(
        &self,
        file: usize,
        kind: ObjKind,
        parts: &[String],
        certain_only: bool,
        check_usable: bool,
    ) -> Resolution {
        let f = &self.files[file];
        let visible = |i: &usize| {
            self.visible[f.app].contains(&self.objects[*i].app)
                && (!check_usable || (self.in_compile(*i) && self.usable(*i)))
        };
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
        let visible = |i: &&usize| {
            from_app.is_none_or(|a| self.visible[a].contains(&self.objects[**i].app))
                && self.in_compile(**i)
                && self.usable(**i)
        };
        let hits = match name_or_id.parse::<u32>() {
            Ok(id) => self.by_id.get(&(kind, id)),
            Err(_) => self.by_name.get(&(kind, name_or_id.to_lowercase())),
        };
        hits?.iter().find(visible).copied()
    }

    /// Whether object `i` belongs to a dependency app.
    pub fn is_dependency(&self, i: usize) -> bool {
        self.kinds[self.objects[i].app] == AppKind::Dependency
    }

    /// The syntax node of object `i`.
    pub fn object_ast(&self, i: usize) -> ast::Object {
        self.object_node(&self.objects[i])
            .expect("object node at recorded offset")
    }

    /// Objects are direct children of the file's root, so look there rather than
    /// walking every node of the file.
    fn object_node(&self, o: &ObjectSym) -> Option<ast::Object> {
        let root = al_syntax::SyntaxNode::new_root(self.green(o.file));
        root.children().filter_map(ast::Object::cast).nth(o.ordinal)
    }

    fn resolve_references(&mut self) {
        let mut diags = Vec::new();
        for o in &self.objects {
            let file = &self.files[o.file];
            // A dependency app compiled; its references are resolved when its code is used.
            if file.lazy {
                continue;
            }
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

/// What loading needs from one file; plain data, so it can be produced
/// on a worker thread (syntax nodes cannot cross threads, green trees can).
struct ParsedFile {
    /// The tree; none for a file that was only scanned.
    green: Option<rowan::GreenNode>,
    errors: Vec<al_syntax::ParseError>,
    namespace: Option<String>,
    usings: Vec<String>,
    /// The objects of the file, in source order.
    objects: Vec<FileObject>,
}

struct FileObject {
    kind: ObjKind,
    id: Option<u32>,
    name: String,
    offset: usize,
    extends: Option<Vec<String>>,
    implements: Vec<Vec<String>>,
    subscribers: Vec<al_syntax::scan::ScannedSubscriber>,
}

fn parse_file(src: &str, symbols: &BTreeSet<String>, cache: &mut al_syntax::TreeCache) -> ParsedFile {
    let parse = al_syntax::parse_with_cache(src, symbols, cache);
    let root = ast::SourceFile::cast(parse.syntax()).expect("root is a SourceFile");
    ParsedFile {
        namespace: root.namespace(),
        usings: root.usings(),
        objects: root
            .objects()
            .map(|o| FileObject {
                kind: ObjKind::from_keyword(&o.kind()),
                id: o.id(),
                name: o.name(),
                offset: o.syntax().text_range().start().into(),
                extends: o.extends_parts(),
                implements: o.implements_parts(),
                subscribers: Vec::new(),
            })
            .collect(),
        green: Some(parse.green),
        errors: parse.errors,
    }
}

/// The index entry of a file that is not parsed at load.
fn scanned_file(src: &str, symbols: &BTreeSet<String>) -> ParsedFile {
    let scan = al_syntax::scan::scan(src, symbols);
    ParsedFile {
        green: None,
        errors: Vec::new(),
        namespace: scan.namespace,
        usings: scan.usings,
        objects: scan
            .objects
            .into_iter()
            .map(|o| FileObject {
                kind: ObjKind::from_keyword(&o.kind),
                id: o.id,
                name: o.name,
                offset: o.offset,
                extends: o.extends,
                implements: o.implements,
                subscribers: o.subscribers,
            })
            .collect(),
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
        Workspace::load(apps, &["BC27PLUS".to_string()].into())
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

    fn lazy_ws() -> Workspace {
        let dep = app(
            "dep",
            &[],
            &[
                (
                    "Dep/cust.al",
                    "namespace Dep.Sales;\ntable 18 Customer { fields { field(1; \"No.\"; Code[20]) { } } }",
                ),
                ("Dep/item.al", "namespace Dep.Inv;\ntable 27 Item { }"),
                (
                    "Dep/ext.al",
                    "namespace Dep.Sales;\ntableextension 50 \"Cust Ext\" extends Customer { }",
                ),
                (
                    "Dep/sub.al",
                    "codeunit 60 Subs { [EventSubscriber(ObjectType::Table, Database::Customer, 'OnAfterInsertEvent', '', false, false)] local procedure A() begin end; }",
                ),
                ("Dep/impl.al", "codeunit 61 Impl implements \"Dep I\" { }"),
                (
                    "Dep/broken.al",
                    "table 70 Broken { fields { field(1; F; Integer) { } } procedure P( begin end; }",
                ),
            ],
        );
        let main = app(
            "main",
            &["dep"],
            &[("m.al", "codeunit 1 M { var C: Record Customer; }")],
        );
        Workspace::load_apps(
            vec![main, dep],
            vec![AppKind::Workspace, AppKind::Dependency],
            &["BC27PLUS".to_string()].into(),
        )
    }

    #[test]
    fn dependency_files_are_scanned_at_load_and_parsed_on_first_use() {
        let ws = lazy_ws();
        let file = |p: &str| ws.files.iter().position(|f| f.path == p).unwrap();
        assert!(
            ws.files[file("m.al")].green.get().is_some(),
            "workspace files parse at load"
        );
        // The index knows every object, with its namespace.
        let customer = ws.objects.iter().position(|o| o.name == "Customer").unwrap();
        assert_eq!(ws.objects[customer].namespace.as_deref(), Some("Dep.Sales"));
        assert_eq!(ws.objects[customer].id, Some(18));
        // The workspace's own reference to Customer resolved without a diagnostic. That parsed
        // the file of Customer and no other.
        assert_eq!(ws.diagnostics, vec![]);
        assert!(ws.files[file("Dep/cust.al")].green.get().is_some());
        for p in [
            "Dep/item.al",
            "Dep/ext.al",
            "Dep/sub.al",
            "Dep/impl.al",
            "Dep/broken.al",
        ] {
            assert!(ws.files[file(p)].green.get().is_none(), "{p} is only scanned");
        }
    }

    #[test]
    fn extensions_and_subscribers_are_found_from_the_index() {
        let ws = lazy_ws();
        let ix = |n: &str| ws.objects.iter().position(|o| o.name == n).unwrap();
        assert_eq!(ws.extension_target(ix("Cust Ext")), Some(ix("Customer")));
        assert_eq!(ws.extensions_of(ix("Customer")), [ix("Cust Ext")]);
        assert_eq!(ws.extension_target(ix("Customer")), None);
        let subs = ws.subscribers_of(ix("Customer"));
        assert_eq!(subs.len(), 1);
        assert_eq!(subs[0].0, ix("Subs"));
        assert_eq!(ws.objects[subs[0].0].subscribers[subs[0].1].event, "OnAfterInsertEvent");
        assert_eq!(ws.implementers_of("Dep I"), [ix("Impl")]);
        assert!(ws.subscribers_of(ix("Item")).is_empty());
    }

    #[test]
    fn a_dependency_object_whose_file_does_not_parse_is_unusable_and_says_why() {
        let ws = lazy_ws();
        let broken = ws.objects.iter().position(|o| o.name == "Broken").unwrap();
        assert!(!ws.usable(broken));
        let why = ws.unusable_reason(broken).unwrap();
        assert!(why.contains("Dep/broken.al") && why.contains("does not parse"), "{why}");
        // A reference to it resolves to nothing rather than to half a table.
        let fi = ws.files.iter().position(|f| f.path == "m.al").unwrap();
        assert_eq!(
            ws.resolve_ref(fi, ObjKind::Table, &["Broken".to_string()]),
            Resolution::Missing
        );
        assert!(ws.lazy_parse_problems().iter().any(|(p, _)| p == "Dep/broken.al"));
        // The healthy objects are unaffected.
        let customer = ws.objects.iter().position(|o| o.name == "Customer").unwrap();
        assert!(ws.usable(customer));
        assert_eq!(ws.unusable_reason(customer), None);
    }

    #[test]
    fn clashes_between_dependency_objects_are_not_reported() {
        let ws = Workspace::load_apps(
            vec![app(
                "d",
                &[],
                &[("a.al", "codeunit 1 A { }\ncodeunit 1 B { }\ncodeunit 2 a { }")],
            )],
            vec![AppKind::Dependency],
            &Default::default(),
        );
        assert_eq!(ws.diagnostics, vec![]);
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
