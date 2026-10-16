# ADR-039: Microsoft's .NET set served natively; the sidecar optional

- Status: Proposed
- Date: 2026-10-16
- Related: ADR-014 (DotNet provider seam), ADR-038 (in-process stand-ins; this ADR extends it), ADR-009, ADR-006

## Context

A cloud extension cannot declare `DotNet`. All .NET a cloud app reaches comes from Microsoft's own apps. That set is
closed and measured. If the runtime serves it in Rust, a run needs no .NET install. Only OnPrem extensions with their
own `DotNet` still need the sidecar.

### Measured (inventory of 2026-10-16, `~/.cache/al-runtime/orchestration/dotnet-inventory.md`)

- Declared in Microsoft's apps: 594 types (BC 27.5), 600 (28.1), 602 (29.0); about 4,020 `DotNet` variables. 578 types
  are declared in System Application. About 115 are plain framework types; the rest are BC server classes (about 169),
  OpenXml wrappers (99), XML (62), crypto (52), network (50), imaging (38).
- Reached: six buckets (Bank, ERM, Misc, SCM, SCM-Service, Workflow), stand-ins off: 340,753 sidecar requests,
  241,697 member calls, 87 runtime types, 229 type.member pairs. This is a floor: 12,713 of 24,513 tests did not run.
- No request trace exists in the code. The inventory traced by wrapping `dotnet` (a script that tees the framed
  protocol). A built-in trace is a separate, small task (see Owner decisions).
- Full BC 27 run: 897 sidecar processes (102 s to start), 500,218 requests (69 s), summed over threads.

| Share of 241,697 member calls | Calls | Cumulative |
|---|---|---|
| `TimeZoneInfo` 163,502 + `DateTimeOffset` 47,249 | 210,751 (87.2%) | 87.2% |
| Cheap pure members (Math 8,283, Queue 11,415, ArrayList 1,817, Char, CultureInfo, Path, Uri, String, DateTime, TimeSpan, DataTable) | 22,935 (9.5%) | 96.7% |
| XML (11 types) | 5,856 (2.4%) | 99.1% |
| Rest (already in process by ADR-038, JSON, crypto, streams) | about 2,200 | 100% |

99.99% of 46,380 time zone lookups ask for `UTC`.

## Decision

Serve the closed set below in Rust, phase by phase, under the rules of ADR-038 (closed list in `inproc.txt`,
differential tests, sidecar as oracle). The sidecar stays for OnPrem extension DotNet and for the tail. Nothing in
ADR-014 changes.

### 1. Phases

| Phase | Scope | Measured share | Size |
|---|---|---|---|
| 1 | `TimeZoneInfo` (`FindSystemTimeZoneById`, `BaseUtcOffset`, `GetUtcOffset`, `ConvertTime`, DST members) and `DateTimeOffset` (constructors, `Offset`, `LocalDateTime`, `DateTime`, `ToLocalTime`, `Parse`). UTC first, then a Windows-name table over a zone database. | 87.2% of calls | small (UTC) to medium |
| 2 | Cheap pure members: `Math`, `Char`, `Queue`, `ArrayList`, `List`, `Dictionary`, `KeyValuePair`, `CultureInfo.InvariantCulture`, `Path`, `Uri`, `String`, `DateTime`, `TimeSpan` | +9.5% (96.7%) | small |
| 3 | An owned XML DOM with XPath 1.0 select: `XmlDocument`, `XmlNode(List)`, `XmlElement`, `XmlAttribute(Collection)`, `XmlNamespaceManager`, `XmlReaderSettings`, `XmlTextReader.Create`, `Load`, `LoadXml`, `Save`, `XmlNodeType` | +2.4% (99.1%); about 270 XML refusals; 1,030 declared variables | large |
| 4 | Newtonsoft `JObject`, `JArray`, `JToken`, `JProperty`, `JsonConvert`, `JsonTextReader/Writer` | 21 calls now; 210 `JObject` variables; 65 tests refused because the DLL does not load | medium to large |
| 5 | Crypto beyond hashes: `HMAC*`, `KeyedHashAlgorithm`, `Rfc2898DeriveBytes`, AES/Rijndael, then `RSACryptoServiceProvider` and `X509Certificate2` | 121 calls; 8 `HMACSHA1.Key` refusals | small to medium |
| 6 | `ZipArchive`, `ZipArchiveEntry`, `ZipFile` (gzip is already native), then `MemoryStream`, `StreamReader/Writer`, `BinaryReader/Writer` | 391 calls | small to medium |

Each phase is a separate change with its own differential suite. A phase ships when its suite and the bucket junit
comparison show 0 differences.

### 2. What stays in the sidecar

OnPrem extension `DotNet`; XSLT, XML schema validation, `SignedXml`, `EncryptedXml`; X509 chain handling; non-invariant
cultures; `Regex` unless phase 2 is extended by a later ADR; OpenXml SDK, Word merge, PDF; `System.Drawing`; anything
that opens a socket or touches the file system or Windows (denied by ADR-014 policy and still denied).

### 3. A run without .NET

When no .NET host is found, the runtime serves its native set. A call outside the set is refused, never guessed:
`Signal::Unsupported` (or a static refusal where it can be found at compile time) with a reason of the form
`System.Xml.Xsl.XslCompiledTransform.Transform: not served natively and no .NET host is installed`. The test is
reported as not run, as today for other refusals. With a host installed, behavior is unchanged: native first for the
listed members, sidecar for the rest. An object created natively and handed to a sidecar-only member migrates to the
sidecar (ADR-038 section 2); without a host that migration is the refusal above.

### 4. Equality with .NET

As in ADR-038: every served member runs twice in `crates/al-vm/tests/dotnet_diff.rs`, natively and forced through the
sidecar (`ALRUNNER_NO_INPROC=1`), with generated inputs, comparing the value or the exception type and message. The
list is a file (`inproc.txt`); a test fails when a member has no differential test or the router serves an unlisted
member. Phase-specific inputs: every Windows zone name BC accepts and dates around DST changes from 1900 to 2100
(phase 1); malformed XML, namespaces, `InnerText`/`InnerXml` round trips, `Load` error texts (phase 3); number typing
and formatting (phase 4). Where .NET and the corpus disagree about something AL can see, hard rule 3 applies: a probe
decides. The sidecar answer wins until then.

## Expected effect

- Phase 1 alone removes about 87% of member calls and, with their handle releases, about 87% of all requests (estimate).
  Most test codeunits then never start a sidecar process.
- After phase 3, 99.1% of the measured calls need no sidecar.
- Not an effect of this ADR: the 3,115 tests refused for `GetDotNetType` (1,614) and a DotNet value typed only at run
  time (1,501). They need a probe and a sema decision. They are tracked separately.

## Alternatives considered

1. **Keep the sidecar mandatory.** Simple; every user needs .NET 8 and pays 90 microseconds per call.
2. **Ship a .NET runtime with the tool.** Heavy, platform-specific.
3. **Serve everything Microsoft declares.** 594 types; about 400 are server classes, network, Windows or imaging and
   cannot work outside a service tier.
4. **Faster IPC / batching.** Rejected in ADR-038; the call count stays.

## Consequences

- A second implementation of each served member. The differential suite is the guard; the sidecar is the oracle.
- New dependencies (zone database, XML, JSON, crypto, zip). Owner approval per crate.
- Test results can change where the sidecar had a gap (for example `XmlElement.InnerText` not found, 45 tests; the DLL
  for Newtonsoft missing, 65 tests). Those changes are improvements and are reviewed in the junit comparison.

## Owner decisions needed

1. Phase order: as listed (by measured share), or XML before the cheap members.
2. Is "no .NET installed" a supported mode with a CLI flag (for example `--no-dotnet`, which also makes the refusal
   reason mention the flag), or only the automatic fallback when no host is found?
3. Crates: for time zones (`chrono-tz` or `jiff`); for XML (`roxmltree` for reading plus an owned mutable DOM, or
   `quick-xml`); for JSON (`serde_json` with order preservation, or a hand-written LINQ-to-JSON tree); for crypto
   (`hmac`, `pbkdf2`, `aes`, `cbc`, `rsa`); for zip (`zip`).
4. Add a built-in sidecar request trace (an env variable writing `Type.member` counts), so the inventory can be re-run
   without wrapping `dotnet`.

## Conformance

`dotnet_diff.rs` and the `inproc.txt` list test from ADR-038, extended per phase; the bucket junit comparison
(`tools/compare_junit.py`, 0 differences); a CI run on a host without .NET that must show only the listed refusals.
