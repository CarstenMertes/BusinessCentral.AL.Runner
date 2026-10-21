alrunner (alpha)
================

alrunner runs the tests of Business Central AL apps. It is an alpha:
it does not match AL Runner v2 yet, some tests are refused ("not run")
with a reason, and results may change between builds.

Quick start:

    alrunner --version
    alrunner test ./MyApp ./MyApp.Test

Pass both the app and its test app. Microsoft's apps are fetched from
Microsoft's artifact feed on the first run and cached. No Microsoft code is
in this archive.

Optional: tests that use the DotNet type need the .NET 8 SDK. alrunner builds
a small helper from sources inside the binary on first use.

The binary is not signed.
  macOS:   xattr -d com.apple.quarantine ./alrunner
  Windows: SmartScreen: "More info", then "Run anyway"
           or: Unblock-File .\alrunner.exe

Report problems, especially wrong results:
https://github.com/aldevtools/BusinessCentral.AL.Runner/issues/new?template=alpha-feedback.md

License: MIT (see LICENSE).
