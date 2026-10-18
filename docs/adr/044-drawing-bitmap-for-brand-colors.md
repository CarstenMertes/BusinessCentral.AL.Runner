# ADR-044: `System.Drawing.Bitmap` for Microsoft's brand colors

- Status: Proposed
- Date: 2026-10-18
- Related: ADR-027 (scope, row 14), ADR-014 (policy, section 8), ADR-024, ADR-039, ADR-040 (same pattern), ADR-009

## Context

Both Contoso demo-data lanes of Microsoft's test recipe stop in their first step. `Foundation Module`.CreateSetupData
calls `Create Company Information`.CreateSetupCompany (CreateCompanyInformation.Codeunit.al:76), which calls Base App
`O365 Brand Color`.CreateDefaultBrandColors. For each of 12 colors that calls `MakePicture` (O365BrandColor.Table.al:113-137).
The refusal is `DotNet new System.Drawing.Bitmap` (policy.txt:61, ADR-027 row 14: "v3 will not substitute an imaging
library, bytes would differ from GDI+").

### What `MakePicture` uses

`new Bitmap(100, 100)`; `Graphics.FromImage`; `ColorTranslator.FromHtml("#RRGGBB")`; `new SolidBrush(Color)`;
`Graphics.FillEllipse(brush, 0, 0, 100, 100)`; `Graphics.Dispose`; `Bitmap.Save(InStream, ImageFormat.Png)`;
`Bitmap.Dispose`. The result goes to `"Sample Picture"` (field 15, type Media) through `ImportStream(InStr, '')`.
The brand colors page shows it as a brick. Nothing else in Microsoft's source reads it.

### What Microsoft's tests observe

Only `HasValue`: O365BrandColorTests.Codeunit.al:74 and :158 (`Assert.IsTrue(O365BrandColor."Sample Picture".HasValue, ...)`).
No test reads the bytes, the size or the media id. The Contoso data tool only needs `MakePicture` not to fail.

### Can the .NET helper (sidecar) run it on Linux?

No. Measured on this machine (.NET 8, `System.Drawing.Common` 8.0.0, MIT): `new Bitmap(100, 100)` throws
`TypeInitializationException: The type initializer for 'Gdip' threw an exception`. Since .NET 7 the package supports
Windows only; the Linux switch of .NET 6 is gone. `ColorTranslator.FromHtml` does work (managed code). The only way back
is libgdiplus (Mono's native GDI+ clone) installed on the host, plus a runtime that still honors it. That is a system
package outside our control (not in the Arch repositories checked here), and I could not verify its license offline;
it must be checked against the allow-list before anyone relies on it. It also brings back an install the project is
removing (ADR-039). The corpus CI runs BC in a Linux container, so a probe of Bitmap there would hit the same error:
a probe needs the Windows nightly (Linux suspect rule).

Permissive imaging libraries exist (SkiaSharp, MIT, native part BSD-3; Rust `png` and `tiny-skia`, MIT/Apache and BSD-3).
SixLabors ImageSharp is not on the allow-list (split license). None of them draws the same bytes as GDI+.

## Options

1. **Native stand-in limited to the eight members above.** Draw the ellipse, encode a PNG. About 300 lines in Rust, the
   members registered with pins. Bytes differ from GDI+, so anything that exports the picture and compares bytes
   would be wrong. Needs a probe of what AL can see (below) before it is pinned. Reverses the ADR-027 row 14 wording.
2. **Sidecar loading.** Not possible on Linux (see above). With a shim type backed by SkiaSharp it is option 1 with a
   .NET dependency and a second PNG encoder. Not recommended.
3. **Run `MakePicture`, refuse only what is observable (ADR-040 pattern).** The Bitmap members above are accepted and draw
   nothing. `Save` marks the stream as a "picture placeholder"; `ImportStream` gives the media a value so `HasValue`
   is true. Any later read of the picture content (`ExportStream`, `Length`, `CopyStream`, a download) raises
   `Signal::Unsupported`. Everything outside the eight members stays refused. Smallest change and it cannot show a
   wrong byte. It needs a placeholder flag on a media blob (an additive field, but it touches the Media storage
   path) and the same probe.
4. **Do nothing.** Both lanes stay refused with this reason, and everything behind it stays unreached.

## Recommendation

Option 3. It unblocks the lanes, keeps hard rule 4 (nothing observable is guessed) and follows ADR-040. Option 1 only
if the owner wants pictures that can be exported.

## Probes wanted (corpus, Windows nightly; the Linux container cannot run Bitmap)

1. After `Bitmap(100, 100)`, `FillEllipse`, `Save(InStream of a Temp Blob, ImageFormat.Png)` and `Media.ImportStream`:
   is `HasValue` true, and what are `ExportStream` length and its first 8 bytes (PNG signature `89 50 4E 47 0D 0A 1A 0A`)?
   (Decides whether `Save` into an InStream writes into the blob at all; the Microsoft tests say it does.)
2. The width and height in the PNG header (expect 100 by 100), and the pixel at the center versus a corner.
3. `Save` with another `ImageFormat` (Jpeg, Bmp): refused in any option until probed.

## Decision

Pending. Not decided by the implementation agent: it changes the DotNet policy (policy.txt:61) and ADR-027 row 14.

## Consequences

Option 3: 12 more Contoso steps run; the Foundation data exists for the 13 tests that depend on it. Any test that reads the
picture bytes is refused with a reason naming the media field.

## Conformance

None yet. Option 3 adds a unit test per accepted member and a test that an export of the placeholder is refused.
