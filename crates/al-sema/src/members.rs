//! Members of objects: table fields (with extensions merged), enum values
//! (with extensions merged), and procedure signatures.

use crate::types::Ty;
use crate::workspace::{ObjKind, Resolution, Workspace};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FieldClass {
    Normal,
    FlowField,
    FlowFilter,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldInfo {
    pub id: u32,
    pub name: String,
    pub ty: Ty,
    pub class: FieldClass,
    /// Object index of the table or tableextension that declares it (None for system fields).
    pub declared_in: Option<usize>,
    /// The `InitValue` property as written.
    pub init_value: Option<String>,
    pub auto_increment: bool,
    pub table_relation: bool,
    /// Lower-case name of the table the `TableRelation` points to.
    pub relation_table: Option<String>,
    /// Every table the relation can point to (conditional relations name several).
    pub relation_tables: Vec<String>,
    /// Offset of the field's OnValidate trigger node (in `declared_in`'s file).
    pub on_validate: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcParam {
    pub name: String,
    pub ty: Ty,
    pub by_var: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcSig {
    pub name: String,
    pub params: Vec<ProcParam>,
    /// None when the procedure returns nothing.
    pub ret: Option<Ty>,
    pub local: bool,
    /// Object index of the declaring object.
    pub declared_in: usize,
    /// Offset of the routine node in its file.
    pub offset: usize,
}

#[derive(Debug, Default, Clone)]
pub struct Members {
    pub fields: Vec<FieldInfo>,
    pub enum_values: Vec<(u32, String)>,
    /// `Caption` of enum values that declare one, by ordinal.
    pub enum_captions: Vec<(u32, String)>,
    pub procedures: Vec<ProcSig>,
}

impl Members {
    pub fn field(&self, name: &str) -> Option<&FieldInfo> {
        self.fields.iter().find(|f| f.name.eq_ignore_ascii_case(name))
    }
    pub fn procedures_named(&self, name: &str) -> Vec<&ProcSig> {
        self.procedures
            .iter()
            .filter(|p| p.name.eq_ignore_ascii_case(name))
            .collect()
    }
}

/// Builds members for every object; an extended object's members include
/// those of its extensions.
pub fn build(ws: &Workspace) -> Vec<Members> {
    let mut out: Vec<Members> = (0..ws.objects.len()).map(|i| own_members(ws, i)).collect();
    for (i, o) in ws.objects.iter().enumerate() {
        let Some(target_kind) = o.kind.extended() else { continue };
        let Some(base) = ws.object_ast(i).extends_parts() else {
            continue;
        };
        if let Resolution::Found(t) = ws.resolve_ref(o.file, target_kind, &base) {
            let ext = out[i].clone();
            let target = &mut out[t];
            target.fields.extend(ext.fields);
            target.enum_values.extend(ext.enum_values);
            target.enum_captions.extend(ext.enum_captions);
            target.procedures.extend(ext.procedures);
        }
    }
    out
}

/// Platform fields every table has (learn.microsoft.com, "System fields").
const SYSTEM_FIELDS: &[(u32, &str, Ty)] = &[
    (2000000000, "SystemId", Ty::Guid),
    (2000000001, "SystemCreatedAt", Ty::DateTime),
    (2000000002, "SystemCreatedBy", Ty::Guid),
    (2000000003, "SystemModifiedAt", Ty::DateTime),
    (2000000004, "SystemModifiedBy", Ty::Guid),
    (2000000005, "SystemRowVersion", Ty::BigInteger),
];

/// The text of the first `'…'` literal in a property value (`'It''s', Locked = true` → `It's`).
fn first_string_literal(value: &str) -> Option<String> {
    let rest = value.trim().strip_prefix('\'')?;
    let mut out = String::new();
    let mut chars = rest.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\'' {
            if chars.peek() == Some(&'\'') {
                chars.next();
                out.push('\'');
            } else {
                return Some(out);
            }
        } else {
            out.push(c);
        }
    }
    None
}

/// `OptionMembers = " ",Draft,"In Work"` → `[" ", "Draft", "In Work"]`; an empty
/// entry (`,Alpha`) is the blank member.
fn option_members_property(value: &str) -> Vec<String> {
    // (text, whether any part was quoted): a quoted member keeps its spaces.
    let mut members: Vec<(String, bool)> = vec![(String::new(), false)];
    let mut quoted = false;
    for c in value.chars() {
        let cur = members.last_mut().expect("non-empty");
        match c {
            '"' => {
                quoted = !quoted;
                cur.1 = true;
            }
            ',' if !quoted => members.push((String::new(), false)),
            c => cur.0.push(c),
        }
    }
    members
        .into_iter()
        .map(|(m, was_quoted)| if was_quoted { m } else { m.trim().to_string() })
        .collect()
}

fn own_members(ws: &Workspace, i: usize) -> Members {
    let obj = ws.object_ast(i);
    let mut m = Members::default();
    for sec in obj.sections() {
        let name = sec.name().to_ascii_lowercase();
        if name == "fields" {
            for f in sec.sections().filter(|f| f.name().eq_ignore_ascii_case("field")) {
                let args = f.args();
                let (Some(id), Some(fname)) = (args.first().and_then(|a| a.parse().ok()), args.get(1)) else {
                    continue;
                };
                let mut ty = args
                    .get(2)
                    .and_then(|t| al_syntax::parse_type(t))
                    .map(|t| Ty::from_type_ref(&t))
                    .unwrap_or(Ty::Unknown);
                let class = match f
                    .properties()
                    .find(|p| p.name().eq_ignore_ascii_case("FieldClass"))
                    .map(|p| p.value_text().to_ascii_lowercase())
                {
                    Some(c) if c == "flowfield" => FieldClass::FlowField,
                    Some(c) if c == "flowfilter" => FieldClass::FlowFilter,
                    _ => FieldClass::Normal,
                };
                let prop = |n: &str| {
                    f.properties()
                        .find(|p| p.name().eq_ignore_ascii_case(n))
                        .map(|p| p.value_text())
                };
                // An Option field names its members in the OptionMembers property.
                if let (Ty::Option(_), Some(members)) = (&ty, prop("OptionMembers")) {
                    ty = Ty::Option(option_members_property(&members));
                }
                let init_value = prop("InitValue");
                let auto_increment = prop("AutoIncrement").is_some_and(|v| v.trim().eq_ignore_ascii_case("true"));
                let on_validate = f
                    .triggers()
                    .find(|t| t.name().eq_ignore_ascii_case("OnValidate"))
                    .map(|t| t.syntax().text_range().start().into());
                m.fields.push(FieldInfo {
                    id,
                    name: fname.clone(),
                    ty,
                    class,
                    declared_in: Some(i),
                    init_value,
                    auto_increment,
                    table_relation: prop("TableRelation").is_some(),
                    relation_table: prop("TableRelation").map(|v| relation_target(&v)),
                    relation_tables: prop("TableRelation").map(|v| relation_targets(&v)).unwrap_or_default(),
                    on_validate,
                });
            }
        } else if name == "value" {
            let args = sec.args();
            if let (Some(id), Some(v)) = (args.first().and_then(|a| a.parse().ok()), args.get(1)) {
                m.enum_values.push((id, v.clone()));
                let caption = sec
                    .properties()
                    .find(|p| p.name().eq_ignore_ascii_case("Caption"))
                    .and_then(|p| first_string_literal(&p.value_text()));
                if let Some(c) = caption {
                    m.enum_captions.push((id, c));
                }
            }
        }
    }
    if ws.objects[i].kind == ObjKind::Table {
        for (id, name, ty) in SYSTEM_FIELDS {
            m.fields.push(FieldInfo {
                id: *id,
                name: name.to_string(),
                ty: ty.clone(),
                class: FieldClass::Normal,
                declared_in: None,
                init_value: None,
                auto_increment: false,
                table_relation: false,
                relation_table: None,
                relation_tables: Vec::new(),
                on_validate: None,
            });
        }
    }
    for r in obj.routines() {
        m.procedures.push(ProcSig {
            name: r.name(),
            params: r
                .params()
                .map(|p| ProcParam {
                    name: p.name(),
                    ty: p.type_ref().map(|t| Ty::from_type_ref(&t)).unwrap_or(Ty::Unknown),
                    by_var: p.is_var(),
                })
                .collect(),
            ret: r.return_type().map(|t| Ty::from_type_ref(&t)),
            local: r.is_local(),
            declared_in: i,
            offset: r.syntax().text_range().start().into(),
        });
    }
    m
}

/// Every table a `TableRelation` value names: each arm of `if (…) T1.F else if (…) T2.F else T3.F`.
fn relation_targets(v: &str) -> Vec<String> {
    // Split at top-level `else` keywords, outside quotes and parentheses.
    let chars: Vec<char> = v.chars().collect();
    let (mut segments, mut cur) = (Vec::new(), String::new());
    let (mut depth, mut quote) = (0usize, None::<char>);
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match quote {
            Some(q) => {
                cur.push(c);
                if c == q {
                    quote = None;
                }
            }
            None => match c {
                '"' | '\'' => {
                    quote = Some(c);
                    cur.push(c);
                }
                '(' => {
                    depth += 1;
                    cur.push(c);
                }
                ')' => {
                    depth = depth.saturating_sub(1);
                    cur.push(c);
                }
                _ if depth == 0
                    && chars[i..]
                        .iter()
                        .take(4)
                        .collect::<String>()
                        .eq_ignore_ascii_case("else")
                    && (i == 0 || !chars[i - 1].is_alphanumeric())
                    && chars.get(i + 4).is_none_or(|n| !n.is_alphanumeric()) =>
                {
                    segments.push(std::mem::take(&mut cur));
                    i += 3;
                }
                _ => cur.push(c),
            },
        }
        i += 1;
    }
    segments.push(cur);
    segments
        .iter()
        .map(|seg| {
            // Drop a leading `if (…)` condition.
            let seg = seg.trim();
            let seg = match seg.get(..2).filter(|p| p.eq_ignore_ascii_case("if")) {
                Some(_) => {
                    let mut depth = 0usize;
                    let mut end = None;
                    for (n, c) in seg.char_indices() {
                        match c {
                            '(' => depth += 1,
                            ')' => {
                                depth = depth.saturating_sub(1);
                                if depth == 0 {
                                    end = Some(n + 1);
                                    break;
                                }
                            }
                            _ => {}
                        }
                    }
                    seg[end.unwrap_or(seg.len())..].trim()
                }
                None => seg,
            };
            relation_target(seg)
        })
        .filter(|t| !t.is_empty())
        .collect()
}

/// The table a `TableRelation` value names first: `"Cust".No.` → `cust`.
fn relation_target(v: &str) -> String {
    let v = v.trim();
    let name = match v.strip_prefix('"') {
        Some(rest) => rest.split('"').next().unwrap_or(""),
        None => v.split(['.', ' ', '(']).next().unwrap_or(""),
    };
    name.to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::AppSource;

    #[test]
    fn relation_targets_cover_every_arm_of_a_conditional_relation() {
        // record/TestRenamePropagationConditional.al: the else table's rename also reaches the row.
        assert_eq!(relation_targets("\"ALT Parent\".Code"), vec!["alt parent"]);
        assert_eq!(relation_targets("Customer"), vec!["customer"]);
        assert_eq!(
            relation_targets(
                "if (Kind = const(A)) \"ALT Parent\".Code else if (Kind = const(B)) \"Parent B\".Code where (Active = const(true)) else Third"
            ),
            vec!["alt parent", "parent b", "third"]
        );
        assert_eq!(
            relation_targets("if (Kind = const(\" else \")) Only.Code"),
            vec!["only"]
        );
    }

    fn ws(files: &[(&str, &str)]) -> Workspace {
        Workspace::load(
            vec![AppSource {
                id: "a".into(),
                name: "a".into(),
                dependencies: vec![],
                files: files.iter().map(|(p, s)| (p.to_string(), s.to_string())).collect(),
            }],
            &Default::default(),
        )
    }

    fn members_of(ws: &Workspace, kind: ObjKind, name: &str) -> Members {
        let m = build(ws);
        let Resolution::Found(i) = ws.resolve_ref(0, kind, &[name.to_string()]) else {
            panic!("{name} not found")
        };
        m[i].clone()
    }

    const TABLES: &str = r#"
table 50900 "ALT Universal"
{
    fields
    {
        field(1; "No."; Code[20]) { }
        field(2; Amount; Decimal) { }
        field(3; "Total"; Decimal) { FieldClass = FlowField; CalcFormula = sum("ALT Universal".Amount); }
        field(4; "Filter"; Date) { FieldClass = FlowFilter; }
    }
    procedure Double(X: Integer): Integer
    begin
        exit(X * 2);
    end;
    local procedure Hidden(var R: Record "ALT Universal")
    begin
    end;
}
tableextension 50901 "ALT Ext" extends "ALT Universal"
{
    fields { field(50100; "Ext Field"; Text[50]) { } }
    procedure FromExt(): Boolean
    begin
    end;
}
"#;

    #[test]
    fn table_fields_with_types_and_classes() {
        let w = ws(&[("t.al", TABLES)]);
        let m = members_of(&w, ObjKind::Table, "ALT Universal");
        let f = m.field("no.").unwrap();
        assert_eq!(
            (f.id, f.ty.clone(), f.class.clone()),
            (1, Ty::Code(Some(20)), FieldClass::Normal)
        );
        assert_eq!(m.field("Total").unwrap().class, FieldClass::FlowField);
        assert_eq!(m.field("Filter").unwrap().class, FieldClass::FlowFilter);
    }

    #[test]
    fn table_extension_fields_and_procedures_are_merged_into_the_table() {
        let w = ws(&[("t.al", TABLES)]);
        let m = members_of(&w, ObjKind::Table, "ALT Universal");
        assert_eq!(m.field("Ext Field").unwrap().ty, Ty::Text(Some(50)));
        assert_eq!(m.procedures_named("FromExt").len(), 1);
    }

    #[test]
    fn system_fields_are_present() {
        let w = ws(&[("t.al", TABLES)]);
        let m = members_of(&w, ObjKind::Table, "ALT Universal");
        assert_eq!(m.field("SystemId").unwrap().ty, Ty::Guid);
        assert_eq!(m.field("SystemCreatedAt").unwrap().ty, Ty::DateTime);
        assert_eq!(m.field("SystemModifiedBy").unwrap().ty, Ty::Guid);
        assert_eq!(m.field("SystemRowVersion").unwrap().ty, Ty::BigInteger);
    }

    #[test]
    fn procedure_signatures() {
        let w = ws(&[("t.al", TABLES)]);
        let m = members_of(&w, ObjKind::Table, "ALT Universal");
        let d = m.procedures_named("double")[0];
        assert_eq!(
            d.params,
            vec![ProcParam {
                name: "X".into(),
                ty: Ty::Integer,
                by_var: false
            }]
        );
        assert_eq!(d.ret, Some(Ty::Integer));
        assert!(!d.local);
        let h = m.procedures_named("Hidden")[0];
        assert!(h.local && h.params[0].by_var && h.ret.is_none());
    }

    #[test]
    fn enum_values_with_extension_values() {
        let w = ws(&[(
            "e.al",
            "enum 50900 \"ALT Status\" { Extensible = true; value(0; Open) { } value(1; \"In Progress\") { } }\nenumextension 50901 X extends \"ALT Status\" { value(50000; Extra) { } }",
        )]);
        let m = members_of(&w, ObjKind::Enum, "ALT Status");
        assert_eq!(
            m.enum_values,
            vec![(0, "Open".into()), (1, "In Progress".into()), (50000, "Extra".into())]
        );
    }

    #[test]
    fn codeunit_procedures() {
        let w = ws(&[(
            "c.al",
            "codeunit 1 C { procedure A() begin end; procedure A(X: Text) begin end; internal procedure B(): Text begin end; }",
        )]);
        let m = members_of(&w, ObjKind::Codeunit, "C");
        assert_eq!(m.procedures_named("A").len(), 2, "overloads");
        assert_eq!(m.procedures_named("B")[0].ret, Some(Ty::Text(None)));
    }
}
