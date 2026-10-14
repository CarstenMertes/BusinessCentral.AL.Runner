//! The BC server classes Microsoft's AL reaches through `DotNet` (ADR-014 decision 2, owner
//! decision 2026-10-14). They cannot work outside a service tier: they read the session, the
//! tenant or the service configuration. The runtime provides them itself, so they are never
//! forwarded to the sidecar.
//!
//! A value measured on real BC 27.5 is pinned by a probe (`Pin::Probe`): the probe files are in
//! the corpus-probes branch `al-runtime-rs` (head c542144) and reach the corpus submodule with
//! its next bump. The rest mirrors AL Runner v2 until a probe covers it (`Pin::AlRunner`, with the
//! AL Runner file and line). AL Runner runs BC's real classes on a faked session, so a value is
//! what the real class answers on that session. Where the probe and AL Runner differ, the probe
//! wins: the oracle tenant is a sandbox on Azure-like topology, AL Runner's is Production on
//! premises.
//!
//! The result type of each member is declared in `al_builtins::dotnet_sigs`; a test in
//! `tests/dotnet.rs` checks that both lists name the same members.

use super::registry::{MemberFn, MemberKind, Pin, RegistryMember, RegistryType};
use crate::Session;
use al_builtins::dotnet_sigs::{AL_CONFIG_SETTINGS, TENANT_SETTINGS_HELPER, TEST_EXECUTION, USER_ACCOUNT_HELPER};
use al_builtins::native::Signal;
use al_builtins::value::{DotNetKind, DotNetObject, Value};
use std::cell::RefCell;
use std::rc::Rc;

type Args<'a> = &'a [Value];

fn no_args(args: Args, what: &str) -> Result<(), Signal> {
    if args.is_empty() {
        Ok(())
    } else {
        Err(Signal::Unsupported(format!(
            "DotNet {what} takes no arguments here; the call has {}",
            args.len()
        )))
    }
}

/// The `ALConfigSettings` flags are measured on real BC 27.5. They equal the `ConfigSetting`
/// attribute defaults of `ServerUserSettings` (Nav.Types 27.5), which AL Runner v2 answers.
const CFG_PIN: &str = "probe tests/al-language-onprem/session/TestServerSettingContracts.al:31-48 \
    (corpus-probes c542144); AL Runner v2 AlRunner/BcRuntime.cs:2167 agrees (no config file in headless \
    mode: the real ServerUserSettings answers its ConfigSetting attribute default)";

macro_rules! config_flag {
    ($name:ident, $what:literal, $value:expr) => {
        fn $name(_: &mut Session, _: Option<&Rc<DotNetObject>>, args: Args) -> Result<Value, Signal> {
            no_args(args, $what)?;
            Ok($value)
        }
    };
}

fn al_config_instance(_: &mut Session, _: Option<&Rc<DotNetObject>>, args: Args) -> Result<Value, Signal> {
    no_args(args, "ALConfigSettings.Instance")?;
    Ok(Value::DotNet(Rc::new(DotNetObject::new(DotNetKind::Internal {
        type_name: AL_CONFIG_SETTINGS.into(),
        state: RefCell::new(Vec::new()),
    }))))
}

// ServerUserSettings attribute defaults (Nav.Types 27.5): EnableSaasExtensionInstall false,
// ExcelAddInAzureActiveDirectoryClientId "" (no Guid, so false), EnableEntraGroupsOnPrem false,
// ApiServicesEnabled true, ApiSubscriptionsEnabled true, ApiSubscriptionSendingNotificationTimeout
// 30000, ApiSubscriptionMaxNumberOfNotifications 1000, ApiSubscriptionDelayTime 30000,
// TestAutomationEnabled true, UsePermissionSetsFromExtensions true (also stated in AL Runner's
// RecordPatches.AggregatePermissionSetVirtualTable.cs:9), EnableMembershipEntitlement false.
config_flag!(
    save_ext_install,
    "ALConfigSettings.EnableSaasExtensionInstallConfigSetting",
    Value::Boolean(false)
);
config_flag!(
    excel_addin,
    "ALConfigSettings.IsSaasExcelAddinEnabled",
    Value::Boolean(false)
);
config_flag!(
    entra_groups,
    "ALConfigSettings.EnableEntraGroupsOnPrem",
    Value::Boolean(false)
);
config_flag!(
    api_services,
    "ALConfigSettings.ApiServicesEnabled",
    Value::Boolean(true)
);
config_flag!(
    api_subscriptions,
    "ALConfigSettings.ApiSubscriptionsEnabled",
    Value::Boolean(true)
);
config_flag!(
    api_sending_timeout,
    "ALConfigSettings.ApiSubscriptionSendingNotificationTimeout",
    Value::Integer(30000)
);
config_flag!(
    api_max_notifications,
    "ALConfigSettings.ApiSubscriptionMaxNumberOfNotifications",
    Value::Integer(1000)
);
config_flag!(
    api_delay,
    "ALConfigSettings.ApiSubscriptionDelayTime",
    Value::Integer(30000)
);
config_flag!(
    test_automation,
    "ALConfigSettings.TestAutomationEnabled",
    Value::Boolean(true)
);
config_flag!(
    permission_sets_from_extensions,
    "ALConfigSettings.UsePermissionsFromExtensions",
    Value::Boolean(true)
);
config_flag!(membership_entitlement, "ALConfigSettings.IsSaaS", Value::Boolean(false));

/// Whether the tenant is a sandbox. The oracle tenant is one (probe: `IsSandbox` true,
/// `IsProduction` false, `IsSaaS` true, no testability flag set). AL Runner's skeleton tenant is
/// Production; the probe wins. Fixed here until the identity configuration carries the
/// environment type (ADR-013).
const TENANT_IS_SANDBOX: bool = true;

fn is_sandbox(_: &mut Session, _: Option<&Rc<DotNetObject>>, args: Args) -> Result<Value, Signal> {
    no_args(args, "NavTenantSettingsHelper.IsSandbox")?;
    Ok(Value::Boolean(TENANT_IS_SANDBOX))
}

/// BC's body is `!IsSandbox()`.
fn is_production(_: &mut Session, _: Option<&Rc<DotNetObject>>, args: Args) -> Result<Value, Signal> {
    no_args(args, "NavTenantSettingsHelper.IsProduction")?;
    Ok(Value::Boolean(!TENANT_IS_SANDBOX))
}

fn text(what: &str, args: Args, want: usize, v: &str) -> Result<Value, Signal> {
    if args.len() != want {
        return Err(Signal::Unsupported(format!(
            "DotNet {what} takes {want} argument(s) here; the call has {}",
            args.len()
        )));
    }
    Ok(Value::Text(v.into()))
}

/// The wrapper answers 'Sandbox' (probe). Whether the helper returns that or an empty name that
/// the wrapper turns into 'Sandbox' is not observable through the wrapper; the empty name is the
/// default of an unset `NavTenantSettings.EnvironmentName`.
fn environment_name(_: &mut Session, _: Option<&Rc<DotNetObject>>, args: Args) -> Result<Value, Signal> {
    text("NavTenantSettingsHelper.GetEnvironmentName", args, 0, "")
}

fn application_family(_: &mut Session, _: Option<&Rc<DotNetObject>>, args: Args) -> Result<Value, Signal> {
    text("NavTenantSettingsHelper.GetApplicationFamily", args, 0, "W1")
}

fn power_platform_environment_id(_: &mut Session, _: Option<&Rc<DotNetObject>>, args: Args) -> Result<Value, Signal> {
    text(
        "NavTenantSettingsHelper.GetLinkedPowerPlatformEnvironmentId",
        args,
        0,
        "",
    )
}

/// Every name maps to `GetEnvironmentSetting`, which reads the empty `EnvironmentSettings`.
fn environment_application_setting(_: &mut Session, _: Option<&Rc<DotNetObject>>, args: Args) -> Result<Value, Signal> {
    text("NavTenantSettingsHelper.GetEnvironmentApplicationSetting", args, 1, "")
}

/// `!NavEnvironment.Topology.IsServiceRunningInLocalEnvironment`. The oracle answers true
/// (probe: `IsSaaSInfrastructure` is true), where AL Runner's `StandardServiceTopology` answers
/// false; the probe wins.
fn is_azure(_: &mut Session, _: Option<&Rc<DotNetObject>>, args: Args) -> Result<Value, Signal> {
    no_args(args, "NavUserAccountHelper.IsAzure")?;
    Ok(Value::Boolean(true))
}

/// `NavUserAccountHelper.GetEffectivePermissionForObject(user, company, objectType, objectId)`:
/// five comma-separated Permission ordinals (Read, Insert, Modify, Delete, Execute) of the
/// session user. A test session is SUPER; on table data it holds the four data rights and no
/// Execute (corpus session/TestSessionCompanyInfoExperienceClose.al:62-90, measured on real BC).
/// Other object kinds and other users are not pinned and are refused.
fn effective_permission(s: &mut Session, _: Option<&Rc<DotNetObject>>, args: Args) -> Result<Value, Signal> {
    let [
        Value::Guid(user),
        Value::Text(_) | Value::Code(_),
        kind,
        Value::Integer(_),
    ] = args
    else {
        return Err(Signal::Unsupported(
            "DotNet NavUserAccountHelper.GetEffectivePermissionForObject takes (Guid, Text, Option, Integer)".into(),
        ));
    };
    if *user != s.host.identity.user_security_id() {
        return Err(Signal::Unsupported(
            "DotNet NavUserAccountHelper.GetEffectivePermissionForObject for another user: no per-user \
             permission state is held"
                .into(),
        ));
    }
    match kind {
        Value::Option(0, _) | Value::Integer(0) => Ok(Value::Text("1,1,1,1,0".into())),
        _ => Err(Signal::Unsupported(
            "DotNet NavUserAccountHelper.GetEffectivePermissionForObject on an object kind other than table \
             data: the rights of a SUPER session are not pinned (docs/probes-needed.md)"
                .into(),
        )),
    }
}

/// `NavTestExecution.IsInTestMode()`: true while a test codeunit runs. The probe's
/// `Environment Information.CanStartSession` is false, which is `TaskScheduler.CanCreateTask`
/// (false, also probed) behind this being true.
fn is_in_test_mode(s: &mut Session, _: Option<&Rc<DotNetObject>>, args: Args) -> Result<Value, Signal> {
    no_args(args, "NavTestExecution.IsInTestMode")?;
    Ok(Value::Boolean(s.in_test_codeunit))
}

const ENV_PROBE: &str = "probe tests/al-language/session/TestEnvironmentUserTenantContracts.al";

fn member(name: &str, pin: Pin, f: MemberFn) -> RegistryMember {
    RegistryMember {
        name: name.into(),
        kind: MemberKind::Call,
        pin,
        call: f,
    }
}

fn probe(what: &str) -> Pin {
    Pin::Probe(format!("{ENV_PROBE}:{what} (corpus-probes c542144)"))
}

fn al_runner(what: &str) -> Pin {
    Pin::AlRunner(format!("AL Runner v2 {what}"))
}

/// The shipped types.
pub fn types() -> Vec<RegistryType> {
    let cfg = |name: &str, f: MemberFn| member(name, Pin::Probe(CFG_PIN.into()), f);
    vec![
        RegistryType {
            clr_name: AL_CONFIG_SETTINGS.into(),
            members: vec![
                member(
                    "Instance",
                    al_runner(
                        "AlRunner/BcRuntime.cs:2167 (ALConfigSettings.Instance() is a plain constructor call in \
                         Nav.Types 27.5; the settings it reads are the members below)",
                    ),
                    al_config_instance,
                ),
                cfg("EnableSaasExtensionInstallConfigSetting", save_ext_install),
                cfg("IsSaasExcelAddinEnabled", excel_addin),
                cfg("EnableEntraGroupsOnPrem", entra_groups),
                cfg("ApiServicesEnabled", api_services),
                cfg("ApiSubscriptionsEnabled", api_subscriptions),
                cfg("ApiSubscriptionSendingNotificationTimeout", api_sending_timeout),
                cfg("ApiSubscriptionMaxNumberOfNotifications", api_max_notifications),
                cfg("ApiSubscriptionDelayTime", api_delay),
                cfg("TestAutomationEnabled", test_automation),
                cfg("UsePermissionsFromExtensions", permission_sets_from_extensions),
                cfg("IsSaaS", membership_entitlement),
            ],
        },
        RegistryType {
            clr_name: TENANT_SETTINGS_HELPER.into(),
            members: vec![
                member("IsSandbox", probe("119 (IsSandbox true, IsSaaS true)"), is_sandbox),
                member("IsProduction", probe("120 (IsProduction false)"), is_production),
                member(
                    "GetEnvironmentName",
                    probe("125 ('Sandbox' through the wrapper; the raw name is not observable)"),
                    environment_name,
                ),
                member("GetApplicationFamily", probe("126 ('W1')"), application_family),
                member(
                    "GetLinkedPowerPlatformEnvironmentId",
                    probe("128 ('')"),
                    power_platform_environment_id,
                ),
                member(
                    "GetEnvironmentApplicationSetting",
                    al_runner(
                        "AlRunner/Patches/MetadataPatches.cs:778-835 (empty EnvironmentSettings: every name gives '')",
                    ),
                    environment_application_setting,
                ),
            ],
        },
        RegistryType {
            clr_name: USER_ACCOUNT_HELPER.into(),
            members: vec![
                member("IsAzure", probe("124 (IsSaaSInfrastructure true)"), is_azure),
                member(
                    "GetEffectivePermissionForObject",
                    Pin::Corpus("session/TestSessionCompanyInfoExperienceClose.al:62-90".into()),
                    effective_permission,
                ),
            ],
        },
        RegistryType {
            clr_name: TEST_EXECUTION.into(),
            members: vec![member(
                "IsInTestMode",
                probe("127 (CanStartSession false behind IsInTestMode true)"),
                is_in_test_mode,
            )],
        },
    ]
}
