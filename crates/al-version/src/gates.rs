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
    /// The platform table "Feature Key" has another list of rows from BC 28: the corpus asserts the 27.x list (16 rows)
    /// only below BC 28 (`#if not BC28PLUS`, record/TestFieldLenAndFeatureKeyContracts.al); above, only that the table
    /// has rows. Not consumed yet (see `feature_key_rows_known`).
    FeatureKeyListChangedFrom28,
    /// Column 15 of the Permission table ("AL Object Name") is Text[100] from BC 29, Text[30] before
    /// (permissions/W6PermissionColumns.al, corpus bump 9). Not consumed yet.
    PermissionObjectNameLongerFrom29,
    /// A query aggregate (Sum, Average, Min, Max) over a BigInteger column is accepted from BC 29
    /// (query/W7QueryAggregates.al, corpus bump 9). Not consumed yet.
    QueryAggregateOverBigInteger,
    /// `Base64Convert.FromBase64` of text that is not Base64 raises from BC 28 (types/PVBase64.al, corpus bump 9).
    /// Not consumed yet.
    FromBase64InvalidTextRaises,
    /// Exporting a Media from BC 29 writes the imported bytes back (types/W7Media.al, corpus bump 9). Not consumed yet.
    MediaExportKeepsTheImportedBytes,
    /// The platform metadata tables (All Obj, All Obj with Caption, Field, Key, Table and Codeunit Metadata) list other
    /// columns and rows from BC 29 (metadata/W6*.al, corpus bump 9). Not consumed yet.
    PlatformMetadataColumnsFrom29,
    /// `SetView` and `SetFilter` of text that is no table filter raise the Evaluate error `The value "..." can't be
    /// evaluated into type TableFilter.` from BC 28 (records/W4PlatformAndMeta.al, corpus bump 9). Not consumed yet.
    TableFilterEvaluateError,
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
        Gate::PermissionObjectNameLongerFrom29,
        Gate::QueryAggregateOverBigInteger,
        Gate::FromBase64InvalidTextRaises,
        Gate::MediaExportKeepsTheImportedBytes,
        Gate::PlatformMetadataColumnsFrom29,
        Gate::TableFilterEvaluateError,
    ];

    /// The first BC major with the new behaviour.
    pub fn since_bc(self) -> u16 {
        match self {
            Gate::FlowFieldSumIntegerIntoBigInteger
            | Gate::FlowFieldMinMaxOverRowVersionRefused
            | Gate::SuperPermissionSetIgnoresAppId
            | Gate::NotSaaSInfrastructure
            | Gate::ReportSaveAsXmlHeaderHasCompanyBlock
            | Gate::PermissionObjectNameLongerFrom29
            | Gate::QueryAggregateOverBigInteger
            | Gate::MediaExportKeepsTheImportedBytes
            | Gate::PlatformMetadataColumnsFrom29 => 29,
            Gate::ReportSaveAsDataItemIdentity
            | Gate::FeatureKeyListChangedFrom28
            | Gate::FromBase64InvalidTextRaises
            | Gate::TableFilterEvaluateError => 28,
        }
    }

    /// The corpus guards that pin both sides.
    pub fn evidence(self) -> &'static [Evidence] {
        match self {
            Gate::FlowFieldSumIntegerIntoBigInteger => &[
                Evidence {
                    file: "record/TestCalcFormulaValidationTests.Codeunit.al",
                    lines: "205,227",
                },
                Evidence {
                    file: "record/TestProbeCalcFieldsContracts.al",
                    lines: "120,125",
                },
                Evidence {
                    file: "calcfields/W4CalcFields.al",
                    lines: "31",
                },
            ],
            Gate::FlowFieldMinMaxOverRowVersionRefused => &[
                Evidence {
                    file: "record/TestCalcFormulaSystemFieldsTests.Codeunit.al",
                    lines: "298,306,353",
                },
                Evidence {
                    file: "calcfields/W4CalcFields.al",
                    lines: "105",
                },
            ],
            Gate::SuperPermissionSetIgnoresAppId => &[Evidence {
                file: "permissions/TestMetadataPermissionSet.al",
                lines: "98,106",
            }],
            Gate::NotSaaSInfrastructure => &[Evidence {
                file: "session/TestEnvironmentUserTenantContracts.al",
                lines: "169",
            }],
            Gate::ReportSaveAsXmlHeaderHasCompanyBlock => &[
                Evidence {
                    file: "handlers/TestReportSaveAsDatasetContracts.al",
                    lines: "243",
                },
                Evidence {
                    file: "report/W3ReportDataset.al",
                    lines: "164,209",
                },
            ],
            Gate::PermissionObjectNameLongerFrom29 => &[Evidence {
                file: "permissions/W6PermissionColumns.al",
                lines: "122",
            }],
            Gate::QueryAggregateOverBigInteger => &[Evidence {
                file: "query/W7QueryAggregates.al",
                lines: "148",
            }],
            Gate::FromBase64InvalidTextRaises => &[Evidence {
                file: "types/PVBase64.al",
                lines: "145",
            }],
            Gate::PlatformMetadataColumnsFrom29 => &[
                Evidence {
                    file: "metadata/W6AllObj.al",
                    lines: "207,212",
                },
                Evidence {
                    file: "metadata/W6FieldKeyRows.al",
                    lines: "167,262",
                },
                Evidence {
                    file: "metadata/W6TableCodeunitMetadata.al",
                    lines: "258",
                },
            ],
            Gate::TableFilterEvaluateError => &[Evidence {
                file: "records/W4PlatformAndMeta.al",
                lines: "256",
            }],
            Gate::MediaExportKeepsTheImportedBytes => &[Evidence {
                file: "types/W7Media.al",
                lines: "134,141,146",
            }],
            Gate::FeatureKeyListChangedFrom28 => &[Evidence {
                file: "record/TestFieldLenAndFeatureKeyContracts.al",
                lines: "97",
            }],
            Gate::ReportSaveAsDataItemIdentity => &[
                Evidence {
                    file: "handlers/TestReportSaveAsDatasetContracts.al",
                    lines: "299",
                },
                Evidence {
                    file: "report/W3ReportDataset.al",
                    lines: "294,314",
                },
            ],
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

/// Which list of Feature Key rows a target has (record/PFFeatureKeyColumns.al keys them on the version of the application):
/// the rows differ per minor, and were measured on the newest minor of each major. The 27.0 and 28.0 rows differ from
/// those and are not served.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeatureKeyRows {
    Bc27,
    Bc28,
    Bc29,
}

pub fn feature_key_rows(target: Option<&Target>) -> Option<FeatureKeyRows> {
    match target.map(|t| (t.bc.major, t.bc.minor))? {
        (27, _) => Some(FeatureKeyRows::Bc27),
        (28, 5) => Some(FeatureKeyRows::Bc28),
        (29, 0) => Some(FeatureKeyRows::Bc29),
        _ => None,
    }
}

/// Whether the platform tables of System.app that are `Scope = OnPrem`, internal or removed are known for
/// this BC major: corpus must-not-compile 0020 to 0024 hold the lists for the symbols of BC 27 only, so
/// the AL0296 / AL0161 / AL0433 verdicts for them are not given for another major.
pub fn platform_table_scopes_known(bc_major: Option<u32>) -> bool {
    bc_major == Some(27)
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
