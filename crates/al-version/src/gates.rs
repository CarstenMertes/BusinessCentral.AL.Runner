//! Behaviour that differs between BC versions, as named facts in one table (ADR-015 3.4).
//!
//! A gate is added only when the per-version oracles pin both sides (the corpus has a
//! `#if BCnnPLUS` guard for it). Code asks [`Target::has`]; it never compares a version.
//! A target newer than every gate gets every behaviour: a new major inherits the newest
//! known behaviour until an oracle case says otherwise.

use crate::Target;

/// A corpus test file and the lines of its `#if BCnnPLUS` guards, relative to `corpus/tests/al-language/`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Evidence {
    pub file: &'static str,
    pub lines: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Gate {
    /// A FlowField Sum or Average over an `Integer` source into a `BigInteger` FlowField is
    /// accepted (and the reverse). Before, it is refused with "The following fields must have
    /// the same type".
    FlowFieldSumIntegerIntoBigInteger,
    /// Min and max FlowFields over `SystemRowVersion` are refused when calculated. Before, they
    /// calculate.
    FlowFieldMinMaxOverRowVersionRefused,
    /// Looking up the `SUPER` or `SECURITY` Metadata Permission Set under any App ID finds the
    /// null-App-ID row. Before, the lookup keys on the App ID. Not consumed yet: the
    /// permission provider is not implemented.
    SuperPermissionSetIgnoresAppId,
    /// `Environment Information.IsSaaSInfrastructure` is false. Before, it is true (the oracle
    /// runs of 27.5 and 28.5 answer true, the 29.0 run false, on the same container setup). It is
    /// `NavUserAccountHelper.IsAzure()` in the server-class registry.
    NotSaaSInfrastructure,
    /// The header of the dataset XML that `Report.SaveAs(..., ReportFormat::Xml, ...)` writes has a
    /// `ReportCaption` element after `ReportName` and, after `Minute`, the company information
    /// block. Before, it has neither.
    ReportSaveAsXmlHeaderHasCompanyBlock,
    /// Every `DataItem` element of the dataset XML that `Report.SaveAs` writes carries
    /// ` tableId="<table id>" systemId="<guid>"` after its name. Before BC 28 it carries neither
    /// (27.5 absent, 28.5 and 29.0 present; BC 28.0 to 28.4 were not measured).
    ReportSaveAsDataItemIdentity,
    /// The platform table "Feature Key" has another list of columns and rows from BC 28: the corpus asserts
    /// the 27.x list (16 rows, the columns up to "Description In English") only below BC 28
    /// (`#if not BC28PLUS`); above, only that the table has rows. Not consumed yet: the runtime has no
    /// provider for the table's other columns.
    FeatureKeyListChangedFrom28,
}

impl Gate {
    pub const ALL: &'static [Gate] = &[
        Gate::FlowFieldSumIntegerIntoBigInteger,
        Gate::FlowFieldMinMaxOverRowVersionRefused,
        Gate::SuperPermissionSetIgnoresAppId,
        Gate::NotSaaSInfrastructure,
        Gate::ReportSaveAsXmlHeaderHasCompanyBlock,
        Gate::ReportSaveAsDataItemIdentity,
        Gate::FeatureKeyListChangedFrom28,
    ];

    /// The first BC major with the new behaviour.
    pub fn since_bc(self) -> u16 {
        match self {
            Gate::FlowFieldSumIntegerIntoBigInteger
            | Gate::FlowFieldMinMaxOverRowVersionRefused
            | Gate::SuperPermissionSetIgnoresAppId
            | Gate::NotSaaSInfrastructure
            | Gate::ReportSaveAsXmlHeaderHasCompanyBlock => 29,
            Gate::ReportSaveAsDataItemIdentity | Gate::FeatureKeyListChangedFrom28 => 28,
        }
    }

    /// The corpus guards that pin both sides.
    pub fn evidence(self) -> &'static [Evidence] {
        match self {
            Gate::FlowFieldSumIntegerIntoBigInteger => &[Evidence {
                file: "record/TestCalcFormulaValidationTests.Codeunit.al",
                lines: "205,227",
            }],
            Gate::FlowFieldMinMaxOverRowVersionRefused => &[Evidence {
                file: "record/TestCalcFormulaSystemFieldsTests.Codeunit.al",
                lines: "298,306,353",
            }],
            Gate::SuperPermissionSetIgnoresAppId => &[Evidence {
                file: "permissions/TestMetadataPermissionSet.al",
                lines: "98,106",
            }],
            Gate::NotSaaSInfrastructure => &[Evidence {
                file: "session/TestEnvironmentUserTenantContracts.al",
                lines: "169",
            }],
            Gate::ReportSaveAsXmlHeaderHasCompanyBlock => &[Evidence {
                file: "handlers/TestReportSaveAsDatasetContracts.al",
                lines: "243",
            }],
            Gate::FeatureKeyListChangedFrom28 => &[
                Evidence {
                    file: "record/TestFeatureKeyEnabledContracts.al",
                    lines: "33",
                },
                Evidence {
                    file: "record/TestFieldLenAndFeatureKeyContracts.al",
                    lines: "97",
                },
            ],
            Gate::ReportSaveAsDataItemIdentity => &[Evidence {
                file: "handlers/TestReportSaveAsDatasetContracts.al",
                lines: "299",
            }],
        }
    }
}

impl Target {
    /// Whether the target has the newer behaviour of `gate`.
    pub fn has(&self, gate: Gate) -> bool {
        self.bc.major >= gate.since_bc()
    }
}

/// [`Target::has`] for a run that may have no target: none means the newest behaviour.
pub fn has(target: Option<&Target>, gate: Gate) -> bool {
    target.is_none_or(|t| t.has(gate))
}

/// Whether the rows of the Feature Key table are known for this target: they were measured on BC 27.5
/// (record/TestFieldLenAndFeatureKeyContracts.al, `#if not BC28PLUS`) and not on any other major. It is
/// not a gate: no oracle pins the newer side, so there is no behaviour to switch to.
pub fn feature_key_rows_known(target: Option<&Target>) -> bool {
    target.is_some_and(|t| t.bc.major == 27)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::BcVersion;

    fn target(bc: &str) -> Target {
        Target::for_bc(BcVersion::parse(bc).unwrap())
    }

    #[test]
    fn a_gate_opens_at_its_major() {
        for g in Gate::ALL {
            let since = g.since_bc();
            assert!(!target("27.5").has(*g), "{g:?}");
            assert_eq!(target("28.5").has(*g), since <= 28, "{g:?}");
            assert!(target("29").has(*g), "{g:?}");
        }
    }

    #[test]
    fn a_newer_major_than_every_gate_has_every_behaviour() {
        for g in Gate::ALL {
            assert!(target("30").has(*g));
            assert!(has(None, *g), "no target means the newest");
        }
        assert!(!has(Some(&target("27")), Gate::FlowFieldSumIntegerIntoBigInteger));
    }

    #[test]
    fn every_gate_is_listed_once_and_has_evidence() {
        let mut seen = std::collections::BTreeSet::new();
        for g in Gate::ALL {
            assert!(seen.insert(*g), "{g:?} listed twice");
            assert!(!g.evidence().is_empty(), "{g:?} has no corpus evidence");
        }
    }
}
