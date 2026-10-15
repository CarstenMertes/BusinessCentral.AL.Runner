//! The AL result types of the `DotNet` members the runtime provides itself (ADR-014 section 1,
//! the registry in `al-vm/src/dotnet/`).
//!
//! A member of a .NET type is normally untyped at compile time: BC types it from the assembly,
//! which lowering never opens. A registry member is different. The runtime implements it, so it
//! knows what the member returns, and a result of a known scalar type is an ordinary operand
//! (`if Helper.IsAzure() then`, `Settings.ApiSubscriptionDelayTime() > 0`).
//!
//! This table is read by `al-sema` (typing) and checked against the registry by a test in `al-vm`
//! (every registry member has a row here and every row has a member), so the two cannot drift.
//! Types and members compare ignoring ASCII case, as AL does.

/// What a registry member returns, as far as AL's type checker is concerned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ret {
    Boolean,
    Integer,
    Text,
    /// A `DotNet` object (for example `Instance()`); its type stays "known at run time".
    Object,
    /// No result (a setter): the call is a statement.
    Void,
}

/// `Microsoft.Dynamics.Nav.Types.ALConfigSettings`.
pub const AL_CONFIG_SETTINGS: &str = "Microsoft.Dynamics.Nav.Types.ALConfigSettings";
/// `Microsoft.Dynamics.Nav.NavUserAccount.NavTenantSettingsHelper`.
pub const TENANT_SETTINGS_HELPER: &str = "Microsoft.Dynamics.Nav.NavUserAccount.NavTenantSettingsHelper";
/// `Microsoft.Dynamics.Nav.Runtime.NavTestExecution`.
pub const TEST_EXECUTION: &str = "Microsoft.Dynamics.Nav.Runtime.NavTestExecution";
/// `Microsoft.Dynamics.Nav.NavUserAccount.NavUserAccountHelper`.
/// Azure AD Graph's query object (System Application "Azure AD Graph Impl.").
pub const GRAPH_QUERY: &str = "Microsoft.Dynamics.Nav.AzureADGraphClient.GraphQuery";
pub const USER_ACCOUNT_HELPER: &str = "Microsoft.Dynamics.Nav.NavUserAccount.NavUserAccountHelper";

/// `System.IO.Compression.GZipStream`, behind System Application's "Data Compression".
pub const GZIP_STREAM: &str = "System.IO.Compression.GZipStream";
/// `System.IO.Compression.CompressionMode`.
pub const COMPRESSION_MODE: &str = "System.IO.Compression.CompressionMode";

/// (CLR type, member, result). One row per registry member that returns a value.
pub const SIGS: &[(&str, &str, Ret)] = &[
    (GRAPH_QUERY, "GraphQuery", Ret::Object),
    (GRAPH_QUERY, "IsM365CollaborationEnabled", Ret::Boolean),
    (GRAPH_QUERY, "GetEnvironmentDirectoryGroup", Ret::Text),
    (GRAPH_QUERY, "GetGroupPage", Ret::Object),
    (GRAPH_QUERY, "GetGroupInfo", Ret::Object),
    (GRAPH_QUERY, "FindFirstGroupInfoForDisplayName", Ret::Object),
    (GRAPH_QUERY, "GetUser", Ret::Object),
    (AL_CONFIG_SETTINGS, "Instance", Ret::Object),
    (
        AL_CONFIG_SETTINGS,
        "EnableSaasExtensionInstallConfigSetting",
        Ret::Boolean,
    ),
    (AL_CONFIG_SETTINGS, "IsSaasExcelAddinEnabled", Ret::Boolean),
    (AL_CONFIG_SETTINGS, "EnableEntraGroupsOnPrem", Ret::Boolean),
    (AL_CONFIG_SETTINGS, "ApiServicesEnabled", Ret::Boolean),
    (AL_CONFIG_SETTINGS, "ApiSubscriptionsEnabled", Ret::Boolean),
    (
        AL_CONFIG_SETTINGS,
        "ApiSubscriptionSendingNotificationTimeout",
        Ret::Integer,
    ),
    (
        AL_CONFIG_SETTINGS,
        "ApiSubscriptionMaxNumberOfNotifications",
        Ret::Integer,
    ),
    (AL_CONFIG_SETTINGS, "ApiSubscriptionDelayTime", Ret::Integer),
    (AL_CONFIG_SETTINGS, "TestAutomationEnabled", Ret::Boolean),
    (AL_CONFIG_SETTINGS, "UsePermissionsFromExtensions", Ret::Boolean),
    (AL_CONFIG_SETTINGS, "IsSaaS", Ret::Boolean),
    (TENANT_SETTINGS_HELPER, "IsSandbox", Ret::Boolean),
    (TENANT_SETTINGS_HELPER, "IsProduction", Ret::Boolean),
    (TENANT_SETTINGS_HELPER, "SetTestTenantEnvironmentType", Ret::Void),
    (TENANT_SETTINGS_HELPER, "GetEnvironmentName", Ret::Text),
    (TENANT_SETTINGS_HELPER, "GetLinkedPowerPlatformEnvironmentId", Ret::Text),
    (TENANT_SETTINGS_HELPER, "GetEnvironmentApplicationSetting", Ret::Text),
    (TENANT_SETTINGS_HELPER, "GetApplicationFamily", Ret::Text),
    (TENANT_SETTINGS_HELPER, "TryGetStringTenantSetting", Ret::Boolean),
    (USER_ACCOUNT_HELPER, "IsAzure", Ret::Boolean),
    (USER_ACCOUNT_HELPER, "IsUserDelegatedAdmin", Ret::Boolean),
    (USER_ACCOUNT_HELPER, "GetEffectivePermissionForObject", Ret::Text),
    (TEST_EXECUTION, "IsInTestMode", Ret::Boolean),
    (GZIP_STREAM, "GZipStream", Ret::Object),
    (GZIP_STREAM, "CopyTo", Ret::Void),
    (GZIP_STREAM, "Dispose", Ret::Void),
    (COMPRESSION_MODE, "Compress", Ret::Object),
    (COMPRESSION_MODE, "Decompress", Ret::Object),
];

/// The declared result of `member` on `clr_type`, if the runtime provides that member.
pub fn lookup(clr_type: &str, member: &str) -> Option<Ret> {
    SIGS.iter()
        .find(|(t, m, _)| t.eq_ignore_ascii_case(clr_type) && m.eq_ignore_ascii_case(member))
        .map(|(_, _, r)| *r)
}
