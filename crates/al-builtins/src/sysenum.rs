//! Platform option types whose values AL writes `FieldClass::Normal`: their members, in
//! ordinal order. Values are options at run time (`Value::Option`), so `Format` prints the
//! member name and `=` compares ordinals.
//!
//! `FieldClass` is the C/AL order. The `FieldType` member names are the platform's; their
//! ordinals are not pinned by the corpus (docs/probes-needed.md), so nothing here may be
//! relied on to print as a number.

pub fn members(name: &str) -> Option<&'static [&'static str]> {
    match name.to_ascii_lowercase().as_str() {
        "fieldclass" => Some(&["Normal", "FlowField", "FlowFilter"]),
        // Ordinals of these two are not pinned; the corpus only sets a member and reads it back
        // (record/TestSecurityFilteringContracts.al, TestRecordLock.al).
        "securityfilter" => Some(&["Filtered", "Ignored", "Validated", "Disallowed"]),
        "isolationlevel" => Some(&[
            "Default",
            "ReadUncommitted",
            "ReadCommitted",
            "RepeatableRead",
            "UpdLock",
        ]),
        // The platform's object type option: AL names seven of these members (corpus
        // session/TestWebServiceActionContext.al:176-190 lists exactly Table, Page, Report,
        // Codeunit, XmlPort, Query and MenuSuite), the zero value `TableData` is not one of them
        // (the fresh value of `WebServiceActionContext.GetObjectType`). The ordinals are the C/AL
        // object type option's (docs/probes-needed.md); blank names are ordinals AL cannot name.
        "objecttype" => Some(&[
            "TableData",
            "Table",
            "",
            "Report",
            "",
            "Codeunit",
            "XmlPort",
            "MenuSuite",
            "Page",
            "Query",
        ]),
        // Five members over four values: Get and Updated are both 200
        // (session/TestWebServiceActionContext.al:100-110).
        "webserviceactionresultcode" => Some(&["None", "Get", "Created", "Updated", "Deleted"]),
        "fieldtype" => Some(&[
            "TableFilter",
            "RecordID",
            "OemText",
            "Date",
            "Time",
            "DateFormula",
            "Decimal",
            "Media",
            "MediaSet",
            "Text",
            "Code",
            "Binary",
            "Boolean",
            "Integer",
            "OemCode",
            "Option",
            "BigInteger",
            "BLOB",
            "GUID",
            "DateTime",
            "Duration",
        ]),
        _ => None,
    }
}

/// The member names as an owned option type.
pub fn option_members(name: &str) -> Option<Vec<String>> {
    members(name).map(|m| m.iter().map(|s| s.to_string()).collect())
}
