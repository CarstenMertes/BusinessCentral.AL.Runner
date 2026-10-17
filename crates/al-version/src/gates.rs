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
    /// A BigInteger can be assigned to an Enum variable from BC 29 (an Option variable accepts it in every version).
    /// Before, it is refused with AL0122 (types/lt-bigint-enum-option/BigIntToEnumOption.al, corpus bump 20; must-not-compile
    /// case 0451). Not consumed yet.
    BigIntegerToEnumVariableFrom29,
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
    /// The conversion and argument error texts of BC 27.5 and later (`The value "x" can't be evaluated into type Integer.`)
    /// differ from those of BC 27.0 and 27.3 (`The type NavInteger is unknown.`, `Invalid Conversion. Source: NavChar, ...`).
    /// The corpus compiles those tests from the BC 28 symbols on (`#if BC28PLUS`), so BC 27.5 is not covered either, and only the
    /// BC 28 and 29 values are pinned. HttpHeaders.al also pins the code of a blank header name per major (27 Format,
    /// 28 Argument, 29 Format). Not consumed yet (corpus bump 10).
    ConversionErrorTextsFrom28,
    /// A test codeunit that runs a `List of [Interface ...]` is compiled only for BC 29: on BC 27 the server's generated C# does not
    /// compile and on BC 28 no result comes back (codeunit/lt-enums-interfaces/OjT71052.al, corpus bump 10). Not consumed yet.
    InterfaceListCodeunitFrom29,
    /// Probed values that are the same on BC 28 and 29 and differ on BC 27 (the laptop writer's probes-needed files, corpus
    /// bump 11): the date control shorthands, metadata table rows and keys, report dataset rows and names, the SystemRowVersion
    /// aggregates, Media import with three arguments. Each guarded test names its fact; split into one gate per fact when the
    /// runtime implements it. Not consumed yet.
    ProbedValuesDifferFrom28,
    /// Probed values that are the same on BC 29 and differ on BC 27 and 28 (corpus bump 11): BLOB length after a fresh Get, the
    /// BigInteger/Integer FlowField type mismatch, the report dataset header bytes and lines and a second SaveAs on one variable,
    /// Report/Field metadata rows, Media import, BigInteger into an Enum field. Not consumed yet.
    ProbedValuesDifferFrom29,
    /// `Base64Convert.ToBase64` of a Text with `TextEncoding::Windows` or `MSDos` and no code page gives the code page 1252
    /// bytes from BC 28 (`äöü` is `5Pb8`); before, the UTF-8 bytes (`w6TDtsO8`) (types/PVBase64.al, corpus bump 13).
    Base64TextIsWindows1252From28,
    /// The `TestField` error text for a Guid value has braces around the expected value from BC 28 (`must be equal to
    /// '{1111...}'`); BC 27 writes it without braces. A Byte value is written as a number on BC 27. The corpus guards the
    /// typed-value tests with `#if BC28PLUS` (record/lt-testfield-typed-values/, corpus bump 17). Not consumed yet.
    TestFieldTypedValueTexts,
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
        Gate::BigIntegerToEnumVariableFrom29,
        Gate::FromBase64InvalidTextRaises,
        Gate::MediaExportKeepsTheImportedBytes,
        Gate::PlatformMetadataColumnsFrom29,
        Gate::TableFilterEvaluateError,
        Gate::ConversionErrorTextsFrom28,
        Gate::InterfaceListCodeunitFrom29,
        Gate::ProbedValuesDifferFrom28,
        Gate::ProbedValuesDifferFrom29,
        Gate::Base64TextIsWindows1252From28,
        Gate::TestFieldTypedValueTexts,
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
            | Gate::BigIntegerToEnumVariableFrom29
            | Gate::MediaExportKeepsTheImportedBytes
            | Gate::PlatformMetadataColumnsFrom29
            | Gate::InterfaceListCodeunitFrom29
            | Gate::ProbedValuesDifferFrom29 => 29,
            Gate::ReportSaveAsDataItemIdentity
            | Gate::FeatureKeyListChangedFrom28
            | Gate::FromBase64InvalidTextRaises
            | Gate::TableFilterEvaluateError
            | Gate::ConversionErrorTextsFrom28
            | Gate::ProbedValuesDifferFrom28
            | Gate::Base64TextIsWindows1252From28
            | Gate::TestFieldTypedValueTexts => 28,
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
            Gate::BigIntegerToEnumVariableFrom29 => &[Evidence {
                file: "types/lt-bigint-enum-option/BigIntToEnumOption.al",
                lines: "8,28,39,145",
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
            Gate::ConversionErrorTextsFrom28 => &[
                Evidence {
                    file: "errors/lt-error-handling/ErTRuntimeErrors.al",
                    lines: "13",
                },
                Evidence {
                    file: "network/lt-http/HttpHeaders.al",
                    lines: "282,304",
                },
                Evidence {
                    file: "record/lt-record-read/RcAGetFind.al",
                    lines: "200",
                },
                Evidence {
                    file: "recordref/lt-refs/RfTFieldFilters.al",
                    lines: "118",
                },
                Evidence {
                    file: "recordref/lt-refs/RfTValues.al",
                    lines: "178",
                },
            ],
            Gate::InterfaceListCodeunitFrom29 => &[Evidence {
                file: "codeunit/lt-enums-interfaces/OjT71052.al",
                lines: "43",
            }],
            Gate::ProbedValuesDifferFrom28 => &[
                Evidence {
                    file: "misc/lt-probes-needed-1/PnACalcFieldsBC29.al",
                    lines: "210",
                },
                Evidence {
                    file: "misc/lt-probes-needed-2/PnBPages.al",
                    lines: "297",
                },
                Evidence {
                    file: "misc/lt-probes-needed-2/PnBReportSaveAs.al",
                    lines: "185,291",
                },
                Evidence {
                    file: "misc/lt-probes-needed-3/PnC21Metadata.al",
                    lines: "297,385,468",
                },
                Evidence {
                    file: "misc/lt-probes-needed-5/PnE06Media.al",
                    lines: "65",
                },
                Evidence {
                    file: "pages/lt-page-runtime/P82TestField.al",
                    lines: "77,208",
                },
                Evidence {
                    file: "recordref/lt-recordref-fieldref-members/P92DirtyRef.al",
                    lines: "8",
                },
                Evidence {
                    file: "misc/lt-open-overloads-collections-isolated/P107RecRefNames.al",
                    lines: "99",
                },
                Evidence {
                    file: "record/lt-testfield-fielderror-typed/P96ErrorInfo.al",
                    lines: "142",
                },
                Evidence {
                    file: "record/lt-testfield-fielderror-typed/P96Temporal.al",
                    lines: "171",
                },
                Evidence {
                    file: "streams/StTCopyBlob.al",
                    lines: "451,456,462,484",
                },
                Evidence {
                    file: "error-handling/lt-last-error-text-forms/FindEmptyTableExcludeFlag.al",
                    lines: "76",
                },
                Evidence {
                    file: "metadata/lt-platform-metadata-columns/PlatformMetadataColumnValues.al",
                    lines: "220",
                },
            ],
            Gate::ProbedValuesDifferFrom29 => &[
                Evidence {
                    file: "misc/lt-probes-needed-1/PnABlobLength.al",
                    lines: "34",
                },
                Evidence {
                    file: "misc/lt-probes-needed-1/PnACalcFieldsBC29.al",
                    lines: "87,253",
                },
                Evidence {
                    file: "misc/lt-probes-needed-2/PnBRecordMisc.al",
                    lines: "627",
                },
                Evidence {
                    file: "misc/lt-probes-needed-2/PnBReportSaveAs.al",
                    lines: "103,143,417",
                },
                Evidence {
                    file: "misc/lt-probes-needed-3/PnC21Metadata.al",
                    lines: "255,427",
                },
                Evidence {
                    file: "misc/lt-probes-needed-5/PnE06Media.al",
                    lines: "23",
                },
                Evidence {
                    file: "misc/lt-probes-needed-5/PnE13EnumConv.al",
                    lines: "159",
                },
                Evidence {
                    file: "recordref/lt-recordref-fieldref-members/P92Fields.al",
                    lines: "31",
                },
                Evidence {
                    file: "report/lt-report-dataset-header/ReportDatasetHeaderRaw.al",
                    lines: "192,197,202,207,212,217,222,227,232,237,242",
                },
            ],
            Gate::Base64TextIsWindows1252From28 => &[Evidence {
                file: "types/PVBase64.al",
                lines: "83,88,231",
            }],
            Gate::TestFieldTypedValueTexts => &[
                Evidence {
                    file: "record/lt-testfield-typed-values/FieldRefTestFieldTypedNumeric.al",
                    lines: "143,165,175,207",
                },
                Evidence {
                    file: "record/lt-testfield-typed-values/FieldRefTestFieldTypedOther.al",
                    lines: "137,159,169,201",
                },
                Evidence {
                    file: "record/lt-testfield-typed-values/RecordTestFieldTypedOther.al",
                    lines: "121,139,149,177",
                },
                Evidence {
                    file: "record/lt-testfield-typed-values/RecordTestFieldWrongTypeGuidOnText.al",
                    lines: "8,53,60,67,75",
                },
            ],
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
