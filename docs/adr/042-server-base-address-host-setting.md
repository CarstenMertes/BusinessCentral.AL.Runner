# ADR-042: The server base address behind `GetUrl` is a host setting

- Status: Proposed
- Date: 2026-10-17

## Context

`GetUrl` is refused today ("the server address is not a host value"). Microsoft's code needs it in places no test
chooses. With a company built from the RapidStart package (`--company-data package`), Company-Initialize calls
`Doc. Exch. Service Setup.SetURLsToDefault`, then `OAuth2Impl.GetDefaultRedirectUrl`, then `GetUrl(ClientType::Web)`
and stores the result in `Redirect URL`. The install baseline stops there, `Application Area Setup` and `Workflow`
stay unseeded and about 14000 Microsoft tests are refused (full BC 27.5 run, 2026-10-17). The backup company hides
this because it already holds the row.

The address of a server is environmental, like the user id or `--environment` (ADR-006, ADR-014 amendment
2026-10-15). It is not a BC behaviour a test can pin. The shape of the string around it is.
Owner decision 2026-10-17: "host setting + probe".

## Decision

1. **The host owns the base address.** A new defaulted method on the `al-host` identity capability, `server_base_url()`,
   returns the base address of the web client (scheme, host, port, instance path, no query). Core crates build
   nothing from the OS (ADR-006); they ask the host.
2. **Setting.** CLI: `--server-url URL` on `alrunner test`, `al-test-cli` and `alrunner serve`. Serve and embedding:
   the same value as the `serverUrl` field of the run request (additive field, protocol v2 unchanged otherwise).
   Environment variable `ALRUNNER_SERVER_URL` is the fallback. The value must be an absolute `http` or `https` URL;
   anything else is a start-up error (exit 2).
3. **Default.** `http://localhost:8080/BC/` when nothing is set. It is a stated host value, not a claim about BC: the
   run summary prints `server address: <value> (default)` or `(--server-url)`. A test whose result depends on the
   host value is, like any other, free to differ from real BC only in that value. (Owner: confirm the default, or
   choose "no default: `GetUrl` stays refused until set".)
4. **What `GetUrl` builds from it.** Only the part that is pinned. Until the probe below lands, `GetUrl(ClientType::Web)`
   without company and without object returns the base address; every form with a company, an object, a record or
   another client type stays refused with the reason "not pinned (docs/probes-needed.md)". The base address is
   returned verbatim, so code that cuts the path (`OAuth2Impl.GetDefaultRedirectUrl`) works on a value the user chose.
5. **Marked as an assumption.** The path through `GetUrl` registers an assumption id (hard rule 3 exception does not
   apply: no Microsoft test pins the string), so a test that passes through it is a flagged pass.

## Pinned and to be probed

| Fact | State |
|---|---|
| `GetUrl` returns a non-empty text starting with `http` for `ClientType::Web` | observed only (corpus lt-system-database-members/GetUrl*.al, `Observer`, no assertion) |
| Query parts (`company=`, `page=`, `report=`, `codeunit=`, `table=`) per call form | to be probed |
| Result without company, with an empty company, with a name needing escaping | to be probed |
| Other client types (Phone, Tablet, Desktop, Default, Background) | to be probed; stay refused |
| `GetUrl` inside a per-database install trigger (no company) | to be probed; the baseline needs it |
| Trailing `/` of the base address and `GetBaseUrl` on SaaS | to be probed; the runtime does not normalise the setting |

## Consequences

- The package company can pass Company-Initialize's address step. Other later stops are separate work.
- The default is visible in the summary, so a parity report names the address it ran with.
- The compile cache and the install-baseline cache key must include the address (it changes `Redirect URL`).
- Embedding hosts (ALchemist, LethAL) can pass their real service address.

## Conformance

- Unit test: `--server-url` parsing (valid, relative, wrong scheme, repeated).
- Test: `GetUrl(ClientType::Web)` returns exactly the configured address; a form with a company is refused.
- Test: the baseline-cache key differs for two addresses.
- Probe topic in `docs/probes-needed.md` (row `GetUrl(ClientType::Web)`).
