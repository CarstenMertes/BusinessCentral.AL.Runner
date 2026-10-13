# Probes needed

Behaviour implemented from documentation only, because no corpus test pins
it yet. Each entry needs a probe test on a branch of the corpus repo (hard
rule 3, ADR-007); once its CI result is in, the entry moves into the code
comment as a corpus citation and is deleted here.

| Area | What is assumed | Source today | Code |
|---|---|---|---|
| Format | Decimal format 0/1 uses `,` thousands and `.` decimals (en-US), decimals trimmed of trailing zeros | doc table "Standard decimal formats" (US) | `al-builtins/src/format.rs` `format_decimal` |
| Format | Date format 0/1 is `MM/DD/YY`, format 2 `MMDDYY[C]D` (en-US) | doc table "Standard date formats" (US) | `format_date` |
| Format | Decimal format 2 trims trailing zeros (`1.50` → `1.5`) | doc `<Decimals>` field, no precision | `format_decimal` |
| Format | Boolean format 9 is `true`/`false` | doc table "Standard boolean formats" | `format` |
| Error | `Error(Text, ...)` sets `GetLastErrorCode()` to `Dialog` | Microsoft test libraries; corpus Assert `DialogTxt` label | `al-builtins/src/native.rs` `error_text` |
| Operators | `and` / `or` evaluate both operands (no short circuit) | C/AL tradition, not documented for AL | `al-vm` `eval` `Binary` |
| Operators | Text `=`/`<` compare ordinally and case-sensitively | none | `al-builtins/src/ops.rs` `compare` |
| asserterror | the message when the block raises nothing is `An error was expected inside an ASSERTERROR statement.` | BC message as commonly reported | `al-vm` `exec` `AssertError` |
| Code | assigning to Code upper-cases and trims leading/trailing spaces | doc "Code data type" | `ops::convert` |
| Record.Find | how `Find('=')` treats the record's active filters (two corpus tests pin opposite-looking cases) | corpus `OwnFilter_*` tests | `al-vm/src/records.rs`, Unsupported meanwhile |
| Record errors | `The record in table %1 already exists. Identification fields and values: %2` and `The %1 does not exist. Identification fields and values: %2` (corpus pins only the fragments) | BC messages as commonly seen | `al-vm/src/records.rs` `fail` |
| Temporary records | not affected by `asserterror` rollback | none | `al-vm` `AssertError` |
| Install | OnInstallAppPerDatabase runs before OnInstallAppPerCompany | doc "Writing extension install code" | `al-test-cli/src/runtime.rs` |
| SetFilter | text filter values on a Code field upper-case before matching (`'ap*'` = `'AP*'`); Boolean filter text accepts Yes/No/true/false | Code conversion rule; corpus uses `Format(false)` as a filter value | `al-builtins/src/filter.rs` |
| JSON | `WriteTo` writes compact JSON (no whitespace, keys in insertion order); the corpus only pins `{}`, `[]` and `"key"` appearing | json/TestJsonXmlDeepContracts.al, TestJsonObject.al | `al-builtins/src/json.rs` `write` |
| JSON | `ReadFrom` of invalid JSON (or JSON of the wrong top-level kind) returns false when the result is used | doc: "returns false if the operation was not successful"; no corpus test feeds invalid JSON | `json.rs` `read_from` |
| JSON | Adding a token that already has a parent copies it, an unattached one is shared (Newtonsoft `JContainer.Add`); a later change to the added object is therefore visible through the parent only in the shared case | none (the corpus never mutates after `Add`) | `json.rs` `attachable` |
| JSON | `Path` is Newtonsoft's (`a.b[0]`, root is empty) | corpus only asserts it contains the key (json/TestJsonTypedGetters.al `JsonToken_Path_ReturnsPath`) | `json.rs` `path_of` |
| JSON | ISO text read as Date/DateTime/Time; a DateTime without an offset is UTC; numbers read as Duration are milliseconds | corpus only asserts the results are non-zero (json/TestJsonComplete.al) | `json.rs`, `iso.rs` |
| JSON | the error texts of a missing key, an out-of-range index, `AsValue` on an object, `Add` of an existing key, `GetX(Key, true)` | corpus only asserts a non-empty `GetLastErrorText` (json/TestJsonTypedGetters.al); the runtime refuses instead of inventing a text | `json.rs` |
| JSON | `ReadFromYaml` / `WriteToYaml` throw on Cloud | session/TestFinalCoverage.al:258 pins only "an error"; not implemented, text unknown | not registered |
| Evaluate | a two-digit year in `MM/DD/YY` is read as 1930-2029 (the .NET default pivot) | none; the corpus only round-trips `Format(Date)` for 2024 and today | `al-builtins/src/evaluate.rs` `date` |
| Evaluate | Boolean accepts exactly `true`, `false`, `Yes`, `No`; other texts are refused | corpus pins `true`/`false` (text/TestEvaluate.al) and `Format(true)` = `Yes` (types/TestFormatBooleanContracts.al:112) | `evaluate.rs` `boolean` |
| Evaluate | a failed `Evaluate` leaves the variable unchanged | none | `evaluate.rs` |
| XML | `WriteTo` writes compact markup: no indentation, empty elements as `<a />`, text escapes `& < >`, attributes escape `& < "` and control characters; the corpus only checks `<test`, `&lt;`, `&amp;` and round trips | xml/TestXmlDocument.al, json/TestJsonXmlDeepContracts.al | `al-builtins/src/xml.rs` `write_node` |
| XML | writing a document that has an XML declaration is refused (whether BC emits `<?xml ...?>` and with which encoding is not pinned) | none | `xml.rs` `serialize` |
| XML | white-space-only text inside an element makes `ReadFrom` refuse (whether BC keeps it by default is not pinned); white space outside the root is dropped | none | `xml.rs` `Parser::element` |
| XML | adding a node that already has a parent copies it (LINQ-to-XML `Add`), a free node is shared; adding a text string, or a second root element, is refused | none (the corpus only adds fresh nodes) | `xml.rs` `content_nodes`, `check_fits` |
| XML | `IsXmlText` on a CDATA section, `GetPrefixOfNamespace` for a default namespace, `GetNamespaceOfPrefix('')`, `GetParent` of a root element, `GetDocument` of a detached element (false when the result is used) | none | `xml.rs` |
| XML | XPath: only simple location paths (names, `*`, `.`, `..`, `@a`, `//`, `[n]`, `[@a='v']`); unprefixed names match no-namespace elements (XPath 1.0); prefixed names and `XmlNamespaceManager` overloads are not implemented | xml/TestXmlDocument.al `//child`, TestXmlExtended.al | `xml.rs` `parse_xpath` |
| XML | `ReadFrom` of invalid XML returns false when the result is used | xml/TestXmlDocument.al `XmlDocument_ReadFrom_InvalidXml_ReturnsFalse` (pinned); the statement form's error text is not | `xml.rs` `read_from_text` |
| XML | `XmlDocument.GetDocumentType` throws without a DOCTYPE | xml/TestXmlComplete.al:194 pins "throws", not the text; refused | `xml.rs` `get_document_type` |
