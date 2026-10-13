//! Semantic types.

use crate::workspace::ObjKind;
use al_syntax::ast::TypeRef;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Ty {
    Integer,
    BigInteger,
    Decimal,
    Boolean,
    Char,
    Byte,
    Text(Option<u32>),
    Code(Option<u32>),
    Date,
    Time,
    DateTime,
    Duration,
    DateFormula,
    Guid,
    Option(Vec<String>),
    Enum(String),
    Record {
        table: String,
        temporary: bool,
    },
    /// Codeunit, Page, Report, Query, XmlPort, Interface variables.
    Object(ObjKind, String),
    TestPage(String),
    TestRequestPage(String),
    /// A part on a test page, carrying the page it hosts.
    TestPart(String),
    /// `TestPage.Filter` or a request page data item: a filter on this table.
    TestFilter(String),
    List(Box<Ty>),
    Dictionary(Box<Ty>, Box<Ty>),
    Array(Vec<u32>, Box<Ty>),
    Label,
    Variant,
    /// A library type from the builtins catalog: `RecordRef`, `JsonObject`, `InStream`, ...
    Builtin(String),
    /// The type could not be determined; errors on it are not reported again.
    Unknown,
}

/// The members of the platform's `TextEncoding` option, in ordinal order (docs: MSDos, UTF8, UTF16,
/// Windows); the corpus uses `TextEncoding::Windows` and `::UTF8` (text/TestTextEncodingWindows.al).
pub fn text_encoding_members() -> Vec<String> {
    ["MSDos", "UTF8", "UTF16", "Windows"].map(String::from).to_vec()
}

impl Ty {
    pub fn from_type_ref(t: &TypeRef) -> Ty {
        let name = t.type_name();
        let lower = name.to_ascii_lowercase();
        let obj = || t.object_name_parts().join(".");
        let size = || t.size_args().first().copied();
        let args = t.type_args();
        let arg = |i: usize| Box::new(args.get(i).map(Ty::from_type_ref).unwrap_or(Ty::Unknown));
        match lower.as_str() {
            "integer" => Ty::Integer,
            "biginteger" => Ty::BigInteger,
            "decimal" => Ty::Decimal,
            "boolean" => Ty::Boolean,
            "char" => Ty::Char,
            "byte" => Ty::Byte,
            "text" => Ty::Text(size()),
            "code" => Ty::Code(size()),
            "date" => Ty::Date,
            "time" => Ty::Time,
            "datetime" => Ty::DateTime,
            "duration" => Ty::Duration,
            "dateformula" => Ty::DateFormula,
            "guid" => Ty::Guid,
            "label" | "textconst" => Ty::Label,
            "variant" => Ty::Variant,
            "option" => Ty::Option(t.option_members()),
            "action" => Ty::Option(
                al_builtins::value::ACTION_MEMBERS
                    .iter()
                    .map(|m| m.to_string())
                    .collect(),
            ),
            "textencoding" => Ty::Option(text_encoding_members()),
            "enum" => Ty::Enum(obj()),
            "record" => Ty::Record {
                table: obj(),
                temporary: t.is_temporary(),
            },
            "testpage" => Ty::TestPage(obj()),
            "testrequestpage" => Ty::TestRequestPage(obj()),
            "list" => Ty::List(arg(0)),
            "dictionary" => Ty::Dictionary(arg(0), arg(1)),
            "array" => Ty::Array(t.size_args(), arg(0)),
            _ => match ObjKind::from_type_name(&name) {
                Some(kind) => Ty::Object(kind, obj()),
                None => Ty::Builtin(canonical_builtin(&name)),
            },
        }
    }
}

impl std::fmt::Display for Ty {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Ty::Text(Some(n)) => write!(f, "Text[{n}]"),
            Ty::Code(Some(n)) => write!(f, "Code[{n}]"),
            Ty::Code(None) => write!(f, "Code"),
            Ty::Text(None) => write!(f, "Text"),
            Ty::Enum(n) => write!(f, "Enum \"{n}\""),
            Ty::Record { table, .. } => write!(f, "Record \"{table}\""),
            Ty::Object(k, n) => write!(f, "{} \"{n}\"", k.label()),
            Ty::TestPage(n) => write!(f, "TestPage \"{n}\""),
            Ty::TestRequestPage(n) => write!(f, "TestRequestPage \"{n}\""),
            Ty::TestPart(n) => write!(f, "TestPart \"{n}\""),
            Ty::TestFilter(n) => write!(f, "TestFilter \"{n}\""),
            Ty::List(t) => write!(f, "List of [{t}]"),
            Ty::Dictionary(k, v) => write!(f, "Dictionary of [{k}, {v}]"),
            Ty::Array(d, t) => write!(f, "array{d:?} of {t}"),
            Ty::Builtin(n) => write!(f, "{n}"),
            other => write!(f, "{other:?}"),
        }
    }
}

/// The catalog's spelling of a library type name (`recordref` → `RecordRef`).
fn canonical_builtin(name: &str) -> String {
    static RECEIVERS: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
    let receivers = RECEIVERS.get_or_init(|| {
        let mut r: Vec<String> = al_builtins::catalog::Catalog::load()
            .signatures
            .iter()
            .map(|s| s.receiver.clone())
            .collect();
        r.dedup();
        r
    });
    receivers
        .iter()
        .find(|r| r.eq_ignore_ascii_case(name))
        .cloned()
        .unwrap_or_else(|| name.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ty(src: &str) -> Ty {
        Ty::from_type_ref(&al_syntax::parse_type(src).unwrap_or_else(|| panic!("`{src}` does not parse")))
    }

    #[test]
    fn primitives_case_insensitive() {
        assert_eq!(ty("Integer"), Ty::Integer);
        assert_eq!(ty("boolean"), Ty::Boolean);
        assert_eq!(ty("BigInteger"), Ty::BigInteger);
        assert_eq!(ty("DateTime"), Ty::DateTime);
        assert_eq!(ty("Guid"), Ty::Guid);
    }

    #[test]
    fn sized_text_and_code() {
        assert_eq!(ty("Text"), Ty::Text(None));
        assert_eq!(ty("Text[30]"), Ty::Text(Some(30)));
        assert_eq!(ty("Code[20]"), Ty::Code(Some(20)));
    }

    #[test]
    fn object_types() {
        assert_eq!(
            ty("Record \"ALT Universal\" temporary"),
            Ty::Record {
                table: "ALT Universal".into(),
                temporary: true
            }
        );
        assert_eq!(ty("Codeunit Assert"), Ty::Object(ObjKind::Codeunit, "Assert".into()));
        assert_eq!(
            ty("Interface IALTCompute"),
            Ty::Object(ObjKind::Interface, "IALTCompute".into())
        );
        assert_eq!(ty("Enum \"ALT Status\""), Ty::Enum("ALT Status".into()));
        assert_eq!(ty("TestPage \"ALT Card\""), Ty::TestPage("ALT Card".into()));
    }

    #[test]
    fn collections_and_arrays() {
        assert_eq!(ty("List of [Integer]"), Ty::List(Box::new(Ty::Integer)));
        assert_eq!(
            ty("Dictionary of [Text, List of [Code[20]]]"),
            Ty::Dictionary(
                Box::new(Ty::Text(None)),
                Box::new(Ty::List(Box::new(Ty::Code(Some(20)))))
            )
        );
        assert_eq!(
            ty("array[2, 3] of Decimal"),
            Ty::Array(vec![2, 3], Box::new(Ty::Decimal))
        );
    }

    #[test]
    fn option_label_variant_and_library_types() {
        assert_eq!(ty("Option A,\"B C\""), Ty::Option(vec!["A".into(), "B C".into()]));
        assert_eq!(ty("Option ,A"), Ty::Option(vec!["".into(), "A".into()]));
        assert_eq!(ty("Label 'x', Locked = true"), Ty::Label);
        assert_eq!(ty("Variant"), Ty::Variant);
        assert_eq!(ty("JsonObject"), Ty::Builtin("JsonObject".into()));
        assert_eq!(ty("recordref"), Ty::Builtin("RecordRef".into()));
    }
}
