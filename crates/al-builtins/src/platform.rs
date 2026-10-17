//! The registry of platform tables that have a native row provider (ADR-013 decision 7, ADR-019).
//!
//! A platform table is a table of the platform app `System` (the system tables, ids from
//! 2000000000). Its schema is the real declaration, taken as AL. Its rows are not AL: the platform
//! computes them, or stores them under native triggers. A platform table with no entry here is
//! refused wherever code reaches it, because a declared table with no behaviour would answer
//! empty, and silently wrong.
//!
//! This is the second id namespace of the builtin registry (ADR-008). The providers themselves
//! live in `al-vm` (`platform/`); this table is data so that `al-sema` can refuse at compile time.

/// How the rows of a platform table are provided.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    /// Rows computed on read; the table cannot be written.
    Virtual(Virtual),
    /// An ordinary stored table, with its real declaration (ADR-013 decision 7).
    Stored,
}

/// The computed tables. One variant per table the runtime can answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Virtual {
    Integer,
    Date,
    Session,
    ActiveSession,
    AllObj,
    AllObjWithCaption,
    CodeunitMetadata,
    TableMetadata,
    Field,
    Key,
    FeatureKey,
    PageMetadata,
    PageControlField,
    TimeZone,
    WindowsLanguage,
    NavAppInstalledApp,
    NavAppExtra,
    InstalledApplication,
    PublishedApplication,
    /// Known to exist, not pinned: lowers, and every read is refused.
    ApiWebhookEntity,
    /// Media live in the session's media store (ADR-017); the table is not a view of it yet.
    TenantMedia,
    MetadataPermissionSet,
    PermissionSet,
    /// The assignable rows of Permission Set (the table is walked through this one); not a table.
    PermissionSetAssignable,
    AggregatePermissionSet,
    MetadataPermission,
    ExpandedPermission,
    Permission,
    ReportLayoutList,
    ReportMetadata,
    ReportDataItems,
    XmlPortMetadata,
    QueryMetadata,
    EventSubscription,
    PageAction,
    TableRelations,
    /// A table of the platform app that is not in [`REGISTRY`]: it lowers, a temporary record of it is
    /// an ordinary temporary table, and every read or write of its persistent rows is refused.
    Unprovided,
}

/// (table id, table name, provider). Every entry is a table of the platform `System` app.
pub const REGISTRY: &[(u32, &str, Provider)] = &[
    (2_000_000_026, "Integer", Provider::Virtual(Virtual::Integer)),
    (2_000_000_007, "Date", Provider::Virtual(Virtual::Date)),
    (2_000_000_009, "Session", Provider::Virtual(Virtual::Session)),
    (
        2_000_000_110,
        "Active Session",
        Provider::Virtual(Virtual::ActiveSession),
    ),
    (2_000_000_038, "AllObj", Provider::Virtual(Virtual::AllObj)),
    (
        2_000_000_058,
        "AllObjWithCaption",
        Provider::Virtual(Virtual::AllObjWithCaption),
    ),
    (
        2_000_000_137,
        "CodeUnit Metadata",
        Provider::Virtual(Virtual::CodeunitMetadata),
    ),
    (
        2_000_000_136,
        "Table Metadata",
        Provider::Virtual(Virtual::TableMetadata),
    ),
    (2_000_000_041, "Field", Provider::Virtual(Virtual::Field)),
    // Pinned for BC 27 only (record/TestFieldLenAndFeatureKeyContracts.al, `#if not BC28PLUS`).
    (2_000_000_211, "Feature Key", Provider::Virtual(Virtual::FeatureKey)),
    // Ordinary stored tables that take Insert and Delete (record/TestPlatformTablesWritableContracts.al:
    // Tenant License State refuses Delete, in al-vm records.rs).
    (2_000_000_196, "Object Options", Provider::Stored),
    (2_000_000_237, "Privacy Notice", Provider::Stored),
    (2_000_000_078, "Chart", Provider::Stored),
    (2_000_000_189, "Tenant License State", Provider::Stored),
    (2_000_000_063, "Key", Provider::Virtual(Virtual::Key)),
    (2_000_000_138, "Page Metadata", Provider::Virtual(Virtual::PageMetadata)),
    (
        2_000_000_192,
        "Page Control Field",
        Provider::Virtual(Virtual::PageControlField),
    ),
    (2_000_000_164, "Time Zone", Provider::Virtual(Virtual::TimeZone)),
    (
        2_000_000_045,
        "Windows Language",
        Provider::Virtual(Virtual::WindowsLanguage),
    ),
    (
        2_000_000_153,
        "NAV App Installed App",
        Provider::Virtual(Virtual::NavAppInstalledApp),
    ),
    (2_000_000_157, "NAV App Extra", Provider::Virtual(Virtual::NavAppExtra)),
    (
        2_000_000_212,
        "Installed Application",
        Provider::Virtual(Virtual::InstalledApplication),
    ),
    (
        2_000_000_206,
        "Published Application",
        Provider::Virtual(Virtual::PublishedApplication),
    ),
    (
        2_000_000_097,
        "API Webhook Entity",
        Provider::Virtual(Virtual::ApiWebhookEntity),
    ),
    // Stored tables whose rows AL reads as facts about the session; the platform seeds them
    // (al-vm `platform/seed.rs`).
    (2_000_000_006, "Company", Provider::Stored),
    (2_000_000_120, "User", Provider::Stored),
    (2_000_000_053, "Access Control", Provider::Stored),
    // Written by Record.AddLink and friends (al-vm `platform/links.rs`).
    (2_000_000_068, "Record Link", Provider::Stored),
    // Read by Base Application's API Webhook Notification Mgt. when a database write reaches the Global
    // Triggers codeunit; a plain tenant table with no native behaviour.
    (2_000_000_095, "API Webhook Subscription", Provider::Stored),
    (2000000159, "Data Sensitivity", Provider::Stored),
    (2000000073, "User Personalization", Provider::Stored),
    (
        2_000_000_250,
        "Metadata Permission Set",
        Provider::Virtual(Virtual::MetadataPermissionSet),
    ),
    (
        2_000_000_004,
        "Permission Set",
        Provider::Virtual(Virtual::PermissionSet),
    ),
    (
        2_000_000_167,
        "Aggregate Permission Set",
        Provider::Virtual(Virtual::AggregatePermissionSet),
    ),
    (2_000_000_165, "Tenant Permission Set", Provider::Stored),
    (2_000_000_166, "Tenant Permission", Provider::Stored),
    (2_000_000_253, "Tenant Permission Set Rel.", Provider::Stored),
    (
        2_000_000_251,
        "Metadata Permission",
        Provider::Virtual(Virtual::MetadataPermission),
    ),
    (
        2_000_000_254,
        "Expanded Permission",
        Provider::Virtual(Virtual::ExpandedPermission),
    ),
    (2_000_000_005, "Permission", Provider::Virtual(Virtual::Permission)),
    (2_000_000_178, "All Profile", Provider::Stored),
    (
        2_000_000_234,
        "Report Layout List",
        Provider::Virtual(Virtual::ReportLayoutList),
    ),
    (
        2_000_000_139,
        "Report Metadata",
        Provider::Virtual(Virtual::ReportMetadata),
    ),
    (
        2_000_000_203,
        "Report Data Items",
        Provider::Virtual(Virtual::ReportDataItems),
    ),
    (
        2_000_000_280,
        "XmlPort Metadata",
        Provider::Virtual(Virtual::XmlPortMetadata),
    ),
    (
        2_000_000_142,
        "Query Metadata",
        Provider::Virtual(Virtual::QueryMetadata),
    ),
    (
        2_000_000_140,
        "Event Subscription",
        Provider::Virtual(Virtual::EventSubscription),
    ),
    (2_000_000_143, "Page Action", Provider::Virtual(Virtual::PageAction)),
    (
        2_000_000_141,
        "Table Relations Metadata",
        Provider::Virtual(Virtual::TableRelations),
    ),
    (2_000_000_049, "Code Coverage", Provider::Stored),
    (2_000_000_288, "Code Coverage Test Lookup", Provider::Stored),
    (2_000_000_289, "Code Coverage Tests Run", Provider::Stored),
    (2_000_000_184, "Tenant Media", Provider::Virtual(Virtual::TenantMedia)),
    (2_000_000_121, "User Property", Provider::Stored),
    (2_000_000_175, "Scheduled Task", Provider::Stored),
    (2_000_000_096, "API Webhook Notification", Provider::Stored),
    (2_000_000_098, "API Webhook Notification Aggr", Provider::Stored),
];

/// Platform tables that only the System app of a later major declares (the registry above is checked against the System app
/// of the first major the corpus pins, `tests/platform_tables.rs`). Ordinary stored tables here, written by install code:
/// "Agent Install" (System Application "Agent Upgrade") and "MCP Install" insert into them, and without a provider the install
/// baseline of those majors stopped at them and left tables unseeded. No corpus test touches them.
pub const LATER_MAJORS: &[(u32, &str, Provider)] = &[
    (2_000_000_297, "Agent Creation Control", Provider::Stored),
    (2_000_000_292, "MCP Configuration", Provider::Stored),
];

/// The provider of platform table `id`, if it has one.
pub fn provider(id: u32) -> Option<Provider> {
    REGISTRY
        .iter()
        .chain(LATER_MAJORS)
        .find(|(i, _, _)| *i == id)
        .map(|(_, _, p)| *p)
}

/// The computed-table provider of platform table `id`, if it is one.
pub fn virtual_table(id: u32) -> Option<Virtual> {
    match provider(id)? {
        Provider::Virtual(v) => Some(v),
        Provider::Stored => None,
    }
}

/// The columns of a registered table that a provider fills with a value something pins, by table
/// id. A table that is not listed has every column filled; for a table that is, a column outside
/// the list (and below the system fields) is not known, and code that uses it is refused where it
/// is lowered (`al-sema`), never given a default (ADR-013 decision 7; hard rule 3).
pub const PINNED_COLUMNS: &[(u32, &[u32])] = &[
    // AllObj: record/TestAllObjVirtualTable.al, TestAllObjOwningAppPerKind.al. "AL Namespace" (62) is not read.
    (2_000_000_038, &[1, 3, 4, 60, 61]),
    // AllObjWithCaption: same files. "AL Namespace" (63) is not read.
    (2_000_000_058, &[1, 3, 4, 20, 30, 60, 61, 62]),
    // CodeUnit Metadata: record/TestCodeunitMetadataVirtualTable.al, TestCodeunitInventoryOrder.al. "App ID" (6) is not read.
    (2_000_000_137, &[1, 2, 3, 4, 5, 7, 8, 9, 10, 11]),
    // Table Metadata: record/TestTableMetadataVirtualTable.al.
    (2_000_000_136, &[1, 2, 3, 4, 5, 6, 11, 12, 13, 15]),
    // Field: record/TestField*VirtualTable.al, TestFieldDataClassificationInheritance.al. TableName, Class, Enabled and "Field Caption"
    // are read from the declaration (probe batch 5); Len (6) from the type (record/TestFieldLenAndFeatureKeyContracts.al); Type Name (9), ExternalName (10) and the rest are not.
    (2_000_000_041, &[1, 2, 3, 4, 5, 6, 7, 8, 20, 21, 22, 24, 25, 26, 27]),
    // Feature Key: ID (1), Enabled (2), Mandatory By (5), Can Try (6), Is One Way (7), Data Update Required (8) and Mandatory By Version (9),
    // record/PFFeatureKeyColumns.al, TestFieldLenAndFeatureKeyContracts.al, TestFeatureKeyEnabledContracts.al.
    (2_000_000_211, &[1, 2, 5, 6, 7, 8, 9]),
    // Key: record/TestKeyVirtualTable.al.
    (2_000_000_063, &[1, 2, 4, 7, 9, 10, 11, 13]),
    // Page Metadata: record/TestPageMetadata*.al. Caption (3) and CardPageID (6) are read from the declaration (probe batch 5).
    (
        2_000_000_138,
        &[
            1, 2, 3, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 19, 20, 21, 22, 23, 24, 26, 27, 28, 30, 31, 32,
        ],
    ),
    // Page Control Field: record/TestPageControlField*.al. "ControlId" (2) is not read; Sequence (11): record/TestTableRelationsSequenceMetadataContracts.al.
    (2_000_000_192, &[1, 3, 4, 5, 6, 7, 8, 9, 10, 11]),
    // NAV App Installed App: record/TestNavAppInstalledAppSysTable.al. The compatibility, hash and extension columns are not read.
    (2_000_000_153, &[1, 2, 3, 4, 5, 6, 7, 8]),
    // Published Application: install/TestRetenPolAllowedTables_Tests.al reads the package ids through System Application code;
    // the identity and version columns mirror NAV App Installed App. "Tenant ID" (12) is read only through the filter
    // `'' | TenantId` that the same test passes through (see apps.rs). Installed (13), "Tenant Visible" (30) and
    // "PerTenant Or Installed" (31): record/TestPublishedApplicationSysTable.al:275-365 (true for every listed app).
    (2_000_000_206, &[1, 2, 3, 4, 5, 6, 7, 8, 9, 12, 13, 30, 31]),
    // NAV App Extra: "Runtime Package ID" (1), "Tenant Visible" (2), "PerTenant Or Installed" (3), the targets of the Lookup
    // FlowFields of Published Application (record/TestPublishedApplicationSysTable.al:294-365). "Package ID" (4) is not read.
    (2_000_000_157, &[1, 2, 3]),
    // Installed Application: "Runtime Package ID" (1), the match column of the Exist FlowField Published Application.Installed
    // (record/TestPublishedApplicationSysTable.al:275-290, :342-365). "Package ID" (2) and "Tenant ID" (3) are not read.
    (2_000_000_212, &[1]),
    // Metadata Permission Set: permissions/TestMetadataPermissionSet.al, TestPermissionSetUndeclaredAssignable.al.
    (2_000_000_250, &[1, 2, 3, 4]),
    // Permission Set: permissions/TestPermissionSetTable.al.
    (2_000_000_004, &[1, 2, 3]),
    // Aggregate Permission Set: access-control/TestAggregatePermissionSetTable.al, permissions/TestPermissionSetTable.al.
    (2_000_000_167, &[1, 2, 3, 4, 5]),
    // Metadata Permission, Expanded Permission: permissions/TestMetadataExpandedPermission.al. The role and object names and
    // the security filter are not read.
    (2_000_000_251, &[1, 2, 4, 5, 7, 8, 9, 10, 11, 12]),
    (2_000_000_254, &[1, 2, 4, 5, 7, 8, 9, 10, 11, 14]),
    // Permission: permissions/TestPermissionSetGrants.al.
    (2_000_000_005, &[1, 3, 4, 6, 7, 8, 9, 10]),
    // All Profile: profile/TestAllProfileTable.al, TestCopyProfile.al. The obsolete columns and "Disable Personalization" are not read.
    (2_000_000_178, &[1, 2, 3, 4, 5, 6, 14, 15, 16, 17]),
    // Report Layout List: handlers/TestSourceReportLayoutFlags.al, TestPrecompiledReportLayouts.al; Caption (3) and Description (10): record/TestEventSubscriptionColumnsReportLayoutListContracts.al.
    (2_000_000_234, &[1, 2, 3, 7, 10, 11, 16, 17]),
    // Report Metadata, Report Data Items: handlers/TestReportMetadataColumns.al.
    (2_000_000_139, &[1, 4, 9, 17, 18]),
    (2_000_000_203, &[1, 2, 3, 4, 5, 6, 7, 8]),
    // XmlPort Metadata: xmlport/TestXmlPortMetadataVirtualTable.al, and Encoding (7) from
    // xmlport/TestPlatformMetadataDefaultsContracts.al:93. The Inherent* columns are not read.
    (2_000_000_280, &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10]),
    // Query Metadata: query/TestQueryMetadataVirtualTable.al. Category (9) is not read.
    (2_000_000_142, &[1, 2, 3, 4, 5, 6, 7, 8, 10]),
    // Event Subscription: record/TestEventSubscriptionVirtualTable.al. "Number of Calls" (8), "Error Information" (9), "Subscriber Instance" (12) and "Active Manual Instances" (13): record/TestEventSubscriptionColumnsReportLayoutListContracts.al. The package and app name are not read.
    (2_000_000_140, &[1, 2, 3, 4, 5, 6, 7, 8, 9, 12, 13]),
    // Page Action: record/TestPageActionVirtualTable.al. Captions, tool tips, Promoted and the Run* columns but RunObjectID are not read.
    (2_000_000_143, &[1, 2, 3, 4, 5, 6, 8, 16]),
    // Table Relations Metadata: record/TestTableRelationsMetadataVirtualTable.al. The condition type, value and the two flags: record/TestTableRelationsSequenceMetadataContracts.al. The names are not read.
    (2_000_000_141, &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11]),
    // Code Coverage tables: codeunit/TestCodeCoverageTable.al, TestCodeCoveragePerTest.al, and "Line" (5) and
    // "Code Coverage Status" (7) from TestCodeCoverageLineTextContracts.al. The method id of a line is not read.
    (2_000_000_049, &[1, 2, 3, 4, 5, 6, 7]),
    (2_000_000_288, &[1, 2, 3, 4]),
    (2_000_000_289, &[1, 2, 3, 4, 5]),
    // Windows Language: record/TestWindowsLanguageTable.al.
    (2_000_000_045, &[1, 2, 10, 11, 30, 35]),
    // Session: session/TestSessionVirtualTable.al. "Login Type" (18) is not read.
    (2_000_000_009, &[1, 2, 3, 5, 6, 16, 17, 19]),
    // Active Session: session/TestActiveSessionTable.al, TestSessionVirtualTable.al. "Server Instance Name", "Server Computer Name" and "Client Computer Name": session/TestActiveSessionHostColumnsContracts.al (shape only).
    (2_000_000_110, &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11]),
];

/// Why column `field` of platform table `table` cannot be read, if it cannot.
pub fn unpinned_column(table: u32, field: u32) -> Option<String> {
    if field >= SYSTEM_OBJECT_BASE {
        return None;
    }
    let (_, pinned) = PINNED_COLUMNS.iter().find(|(t, _)| *t == table)?;
    if pinned.contains(&field) {
        return None;
    }
    let name = REGISTRY
        .iter()
        .find(|(i, _, _)| *i == table)
        .map_or("?", |(_, n, _)| *n);
    Some(format!(
        "column {field} of platform table '{name}' ({table}) is not pinned: no corpus test fixes its value (docs/probes-needed.md)"
    ))
}

/// The first id of the platform's system objects.
pub const SYSTEM_OBJECT_BASE: u32 = 2_000_000_000;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique_system_ids() {
        for (i, (id, name, _)) in REGISTRY.iter().enumerate() {
            assert!(*id >= SYSTEM_OBJECT_BASE, "{name} is not a system object");
            assert!(
                REGISTRY
                    .iter()
                    .skip(i + 1)
                    .all(|(j, n, _)| j != id && !n.eq_ignore_ascii_case(name)),
                "{name} is registered twice"
            );
        }
    }
}
