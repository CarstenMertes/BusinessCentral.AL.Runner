//! The kind of an AL object (ADR-032 step 1). One enum for the compiler and the runtime.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
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
    #[default]
    #[serde(rename = "object")]
    Other,
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

impl std::fmt::Display for ObjKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

/// The text is not the label of any object kind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownKind(pub String);

impl std::fmt::Display for UnknownKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "`{}` is not an object kind", self.0)
    }
}

impl std::error::Error for UnknownKind {}

impl std::str::FromStr for ObjKind {
    type Err = UnknownKind;

    /// The exact inverse of [`ObjKind::label`] (case-sensitive, `object` is `Other`).
    fn from_str(s: &str) -> Result<Self, UnknownKind> {
        Ok(match s {
            "object" => ObjKind::Other,
            _ => match ObjKind::ALL.into_iter().find(|k| k.label() == s) {
                Some(k) => k,
                None => return Err(UnknownKind(s.to_string())),
            },
        })
    }
}

impl ObjKind {
    /// Every kind, in declaration order.
    pub const ALL: [ObjKind; 16] = [
        ObjKind::Table,
        ObjKind::TableExtension,
        ObjKind::Page,
        ObjKind::PageExtension,
        ObjKind::Codeunit,
        ObjKind::Report,
        ObjKind::ReportExtension,
        ObjKind::Query,
        ObjKind::XmlPort,
        ObjKind::Enum,
        ObjKind::EnumExtension,
        ObjKind::Interface,
        ObjKind::PermissionSet,
        ObjKind::PermissionSetExtension,
        ObjKind::Profile,
        ObjKind::Other,
    ];
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn label_and_from_str_are_inverse() {
        for k in ObjKind::ALL {
            assert_eq!(k.label().parse::<ObjKind>(), Ok(k));
            assert_eq!(k.to_string(), k.label());
        }
        assert!("Table".parse::<ObjKind>().is_err());
        assert!("uncompiled table".parse::<ObjKind>().is_err());
    }

    #[test]
    fn serialized_form_is_the_label() {
        for k in ObjKind::ALL {
            assert_eq!(serde_json::to_string(&k).unwrap(), format!("\"{}\"", k.label()));
        }
    }

    #[test]
    fn labels_are_todays_strings() {
        let got: Vec<&str> = ObjKind::ALL.iter().map(|k| k.label()).collect();
        assert_eq!(
            got,
            [
                "table",
                "tableextension",
                "page",
                "pageextension",
                "codeunit",
                "report",
                "reportextension",
                "query",
                "xmlport",
                "enum",
                "enumextension",
                "interface",
                "permissionset",
                "permissionsetextension",
                "profile",
                "object"
            ]
        );
    }
}
