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
    /// The `CalcFormula` property as written.
    pub calc_formula: Option<String>,
    pub table_relation: bool,
    /// Lower-case name of the table the `TableRelation` points to.
    pub relation_table: Option<String>,
    /// Lower-case names of every table of the relation (all arms of a conditional one).
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
                // A table field of type Option lists its members in a property.
                if let (Ty::Option(_), Some(members)) = (&ty, prop("OptionMembers")) {
                    ty = Ty::Option(split_option_members(&members));
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
                    calc_formula: prop("CalcFormula"),
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
                calc_formula: None,
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

/// `Open,"In Progress",,Closed` as member names: quotes removed and kept spaces inside them,
/// blank members kept, unquoted names trimmed.
fn split_option_members(text: &str) -> Vec<String> {
    let mut out: Vec<(String, bool)> = vec![(String::new(), false)];
    let mut in_quote = false;
    for c in text.chars() {
        match c {
            '"' => {
                in_quote = !in_quote;
                out.last_mut().expect("non-empty").1 = true;
            }
            ',' if !in_quote => out.push((String::new(), false)),
            c => out.last_mut().expect("non-empty").0.push(c),
        }
    }
    out.into_iter()
        .map(|(name, quoted)| if quoted { name } else { name.trim().to_string() })
        .collect()
}

/// Every table a `TableRelation` names: `if (..) A.F else if (..) B.F else C.F` gives A, B, C.
/// The `where(..)` filter of an arm names no table.
fn relation_targets(v: &str) -> Vec<String> {
    let c: Vec<char> = v.chars().collect();
    let mut out = Vec::new();
    let (mut i, mut expect_table) = (0, true);
    let skip_parens = |i: &mut usize| {
        // At a '(': to just past its matching ')', ignoring quoted text.
        let mut depth = 0;
        while *i < c.len() {
            match c[*i] {
                '"' | '\'' => {
                    let q = c[*i];
                    *i += 1;
                    while *i < c.len() && c[*i] != q {
                        *i += 1;
                    }
                }
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        *i += 1;
                        return;
                    }
                }
                _ => {}
            }
            *i += 1;
        }
    };
    while i < c.len() {
        if c[i].is_whitespace() || c[i] == '.' {
            i += 1;
            continue;
        }
        if c[i] == '(' {
            skip_parens(&mut i);
            continue;
        }
        let word = if c[i] == '"' {
            i += 1;
            let start = i;
            while i < c.len() && c[i] != '"' {
                i += 1;
            }
            let w: String = c[start..i].iter().collect();
            i += 1;
            (w, true)
        } else {
            let start = i;
            while i < c.len() && (c[i].is_alphanumeric() || c[i] == '_') {
                i += 1;
            }
            if start == i {
                i += 1;
                continue;
            }
            (c[start..i].iter().collect::<String>(), false)
        };
        match (word.1, word.0.to_ascii_lowercase().as_str()) {
            (false, "if" | "else" | "where") => expect_table = !word.0.eq_ignore_ascii_case("where"),
            _ if expect_table => {
                out.push(word.0.to_lowercase());
                expect_table = false;
            }
            _ => {}
        }
    }
    out
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
    fn option_fields_take_their_members_from_the_option_members_property() {
        let w = ws(&[(
            "t.al",
            "table 50902 T { fields { field(1; K; Code[10]) { } field(2; St; Option) { OptionMembers = Open,\"In Progress\",Closed; } field(3; Bl; Option) { OptionMembers = ,A,B; } field(4; Sp; Option) { OptionMembers = \" \",Draft; } } }",
        )]);
        let m = members_of(&w, ObjKind::Table, "T");
        assert_eq!(
            m.field("St").unwrap().ty,
            Ty::Option(vec!["Open".into(), "In Progress".into(), "Closed".into()])
        );
        assert_eq!(
            m.field("Bl").unwrap().ty,
            Ty::Option(vec!["".into(), "A".into(), "B".into()])
        );
        // A quoted name keeps its spaces: a member named with one space is not a blank one.
        assert_eq!(m.field("Sp").unwrap().ty, Ty::Option(vec![" ".into(), "Draft".into()]));
    }

    // corpus _fixtures/tables/ALTRelationParentChild.al:49-96: every arm of a conditional
    // relation names a table whose rename the relating field follows.
    #[test]
    fn relation_tables_lists_every_table_of_a_conditional_relation() {
        let w = ws(&[(
            "t.al",
            "table 50903 T { fields { field(1; K; Code[10]) { }
                field(2; A; Code[10]) { TableRelation = \"Parent A\".\"Code\"; }
                field(3; B; Code[10]) { TableRelation = if (K = const(A)) \"Parent A\".\"Code\" else \"Parent B\".\"Code\"; }
                field(4; C; Code[10]) { TableRelation = if (K = const(A)) \"Parent A\".\"Code\" else if (K = const(B)) \"Parent B\".Code else \"Parent C\".\"Code\"; }
                field(5; D; Code[10]) { TableRelation = \"Parent A\".\"Code\" where(Blocked = const(false)); }
                field(6; E; Code[10]) { TableRelation = if (K = const(A)) Other.Code; }
                field(7; F; Code[10]) { } } }",
        )]);
        let m = members_of(&w, ObjKind::Table, "T");
        let rel = |n: &str| m.field(n).unwrap().relation_tables.clone();
        assert_eq!(rel("A"), ["parent a"]);
        assert_eq!(rel("B"), ["parent a", "parent b"]);
        assert_eq!(rel("C"), ["parent a", "parent b", "parent c"]);
        assert_eq!(rel("D"), ["parent a"]);
        assert_eq!(rel("E"), ["other"]);
        assert!(rel("F").is_empty());
    }

    #[test]
    fn calc_formula_text_is_kept_for_flow_fields() {
        let w = ws(&[("t.al", TABLES)]);
        let m = members_of(&w, ObjKind::Table, "ALT Universal");
        assert_eq!(
            m.field("Total").unwrap().calc_formula.as_deref(),
            Some("sum(\"ALT Universal\".Amount)")
        );
        assert_eq!(m.field("Amount").unwrap().calc_formula, None);
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
