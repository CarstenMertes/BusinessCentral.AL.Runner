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
        // SecurityFilter ordinals are measured on BC 27.5: Validated 0, Filtered 1, Ignored 2,
        // Disallowed 3, read with Format(x, 0, 2) and Evaluate from a number
        // (recordref/TestPlatformOptionsLockingContracts.al:36-43).
        "securityfilter" => Some(&["Validated", "Filtered", "Ignored", "Disallowed"]),
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
        // Only passed to Session.LogSecurityAudit, which changes nothing AL can see. The members are
        // the ones Microsoft's source uses; their ordinals are not pinned and nothing may print them.
        "securityoperationresult" => Some(&["Success", "Failure"]),
        "auditcategory" => Some(&[
            "ApplicationManagement",
            "CustomerFacing",
            "EntitlementManagement",
            "KeyManagement",
            "Other",
            "PolicyManagement",
            "RoleManagement",
            "UserManagement",
        ]),
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
        // The report layout kinds, in the documentation's order. A documentation-only assumption: no corpus
        // test names the members or pins an ordinal, so nothing may print the number (docs/probes-needed.md,
        // "ReportLayoutType"). Base Application compares it with `ReportLayoutType::Word` and `::RDLC`.
        "reportlayouttype" => Some(&["RDLC", "Word", "Excel", "Custom"]),
        "reportformat" => Some(&["Pdf", "Word", "Excel", "Html", "Xml"]),
        "errortype" => Some(&["Client", "Internal"]),
        // The members in the documentation's order (HttpRequestType option type). A handler formats the value
        // (`Get`, `Head`: session/PZHttpHandler.al on the staging branch); the ordinals are not pinned and nothing may
        // print them.
        "httprequesttype" => Some(&[
            "Unknown", "Get", "Delete", "Post", "Put", "Patch", "Options", "Head", "Connect", "Trace",
        ]),
        // The platform's ordinals, probed on BC 27.5 and 29.0 (types/PWMisc.al:ClientType_OrdinalsAndNames, evaluated from
        // 0 to 30; nothing above 15): Current 0, Default 1, Windows 2, Web 3, SOAP 4, OData 5, NAS 6, Background 7,
        // Management 8, Tablet 9, Phone 10, Desktop 11, Teams 12, ODataV4 13, Api 14, ChildSession 15.
        "clienttype" => Some(&[
            "Current",
            "Default",
            "Windows",
            "Web",
            "SOAP",
            "OData",
            "NAS",
            "Background",
            "Management",
            "Tablet",
            "Phone",
            "Desktop",
            "Teams",
            "ODataV4",
            "Api",
            "ChildSession",
        ]),
        "datascope" => Some(&["Module", "Company", "User", "CompanyAndUser"]),
        // Normal and Install are pinned (install/TestInstallEnvVisible_Tests.al:90-127).
        "executioncontext" => Some(&["Normal", "Install", "Upgrade"]),
        // Standard and Debug only (corpus must-not-compile 0010 to 0013 and 0019: Install, Uninstall, Upgrade,
        // Report and Profile do not exist). A test run formats as `Standard`
        // (session/TestEnvironmentUserTenantContracts.al:247); the ordinals are not pinned.
        "executionmode" => Some(&["Standard", "Debug"]),
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

/// Whether `members` is the `ClientType` option: an ordinal or a member name evaluates into it, any other text does not.
pub fn is_client_type(members: &[String]) -> bool {
    let known = members_static("clienttype");
    members.len() == known.len() && members.iter().zip(known).all(|(a, b)| a == b)
}

/// Whether `members` are those of the Field table's `Type` option, whose ordinals are not its positions.
pub fn is_field_table_type(members: &[String]) -> bool {
    const MEMBERS: [&str; 21] = [
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
        "BLOB",
        "Boolean",
        "Integer",
        "OemCode",
        "Option",
        "BigInteger",
        "Duration",
        "GUID",
        "DateTime",
    ];
    members.len() == MEMBERS.len() && members.iter().zip(MEMBERS).all(|(a, b)| a == b)
}

/// Whether `members` is the `FieldType` option. AL cannot read its ordinals, so `Format(x, 0, 2)`
/// prints the member name like any other format (recordref/TestPlatformOptionsLockingContracts.al:
/// 76, "format 2 prints the name too, not an ordinal").
pub fn ordinals_hidden(members: &[String]) -> bool {
    let known = members_static("fieldtype");
    members.len() == known.len() && members.iter().zip(known).all(|(a, b)| a == b)
}

/// Whether `members` is the `SecurityFilter` option, the one platform option an ordinal can be
/// evaluated into (recordref/TestPlatformOptionsLockingContracts.al:42).
pub fn is_security_filter(members: &[String]) -> bool {
    let known = members_static("securityfilter");
    members.len() == known.len() && members.iter().zip(known).all(|(a, b)| a == b)
}

fn members_static(name: &str) -> &'static [&'static str] {
    members(name).expect("known")
}

/// The member names as an owned option type.
pub fn option_members(name: &str) -> Option<Vec<String>> {
    members(name).map(|m| m.iter().map(|s| s.to_string()).collect())
}
