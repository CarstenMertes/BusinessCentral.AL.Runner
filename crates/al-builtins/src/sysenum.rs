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
        // Declaration order of the platform's Verbosity: Critical first, so Error is not the zero
        // value (errorinfo/TestErrorInfoType.al:30-34).
        "verbosity" => Some(&["Critical", "Error", "Warning", "Normal", "Verbose"]),
        // Microsoft docs, TelemetryScope: ExtensionPublisher, then All. Only passed to
        // Session.LogMessage, which changes nothing AL can see, so the ordinals are not pinned.
        "telemetryscope" => Some(&["ExtensionPublisher", "All"]),
        "dataclassification" => Some(&[
            "CustomerContent",
            "ToBeClassified",
            "EndUserIdentifiableInformation",
            "AccountData",
            "EndUserPseudonymousIdentifiers",
            "OrganizationIdentifiableInformation",
            "SystemMetadata",
            "AnonymousData",
        ]),
        // Only CRM and ExternalSQL are named by the corpus (session/TestDatabaseTableConnectionCrm.al,
        // session/TestMiscComplete.al:192); the ordinals are not pinned.
        "tableconnectiontype" => Some(&["CRM", "ExternalSQL"]),
        // The members in the order the documentation lists them (TransactionType option type); the
        // ordinals are not pinned and nothing may print them as numbers.
        "transactiontype" => Some(&["UpdateNoLocks", "Update", "Snapshot", "Browse", "Report"]),
        // The members in the order the documentation lists them. The corpus names Xml, Pdf and Html
        // (handlers/TestReportSaveAsDatasetContracts.al) and never prints or compares the number,
        // so the ordinals are not pinned and nothing may print them.
        "reportformat" => Some(&["Pdf", "Word", "Excel", "Html", "Xml"]),
        "errortype" => Some(&["Client", "Internal"]),
        // The ordinals are not pinned: the corpus only calls `Session.CurrentClientType()` and
        // `DefaultClientType()` (session/TestSessionExtended.al:21,31) and names Default and Web
        // (session/TestFinalCoverage.al:110,120). Nothing may print them as numbers.
        "clienttype" => Some(&[
            "Default",
            "Windows",
            "SOAP",
            "OData",
            "Api",
            "NAS",
            "Background",
            "Management",
            "Tablet",
            "Phone",
            "Web",
            "Desktop",
            "ChildSession",
        ]),
        "datascope" => Some(&["Module", "Company", "User", "CompanyAndUser"]),
        // Normal and Install are pinned (install/TestInstallEnvVisible_Tests.al:90-127).
        "executioncontext" => Some(&["Normal", "Install", "Upgrade"]),
        // Only Standard is pinned (session/TestEnvironmentUserTenantContracts.al:247: a test run
        // formats as `Standard`). The other members and all ordinals are not pinned.
        "executionmode" => Some(&["Standard", "Debug", "Profile"]),
        // Only `LocalScope` is named by the corpus; the ordinals are not pinned.
        "notificationscope" => Some(&["LocalScope", "GlobalScope"]),
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
