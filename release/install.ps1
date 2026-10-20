# Installs alrunner from GitHub releases (Windows x64).
#   install.ps1 [-Version <edge | latest-alpha | v3.0.0-alpha.2>]
# Default is latest-alpha. Environment: ALRUNNER_INSTALL_DIR (default %LOCALAPPDATA%\alrunner\bin),
# ALRUNNER_REPO (default StefanMaron/al-runtime-rs).
param([string]$Version = "latest-alpha")
$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"

$repo = if ($env:ALRUNNER_REPO) { $env:ALRUNNER_REPO } else { "StefanMaron/al-runtime-rs" }
$dir = if ($env:ALRUNNER_INSTALL_DIR) { $env:ALRUNNER_INSTALL_DIR } else { Join-Path $env:LOCALAPPDATA "alrunner\bin" }
$target = "x86_64-pc-windows-msvc"

switch -Regex ($Version) {
  '^edge$' { $tag = "edge"; $label = "edge" }
  '^latest-alpha$' {
    $releases = Invoke-RestMethod "https://api.github.com/repos/$repo/releases?per_page=30"
    $first = $releases | Where-Object { $_.tag_name -ne "edge" -and -not $_.draft } | Select-Object -First 1
    if (-not $first) { throw "No numbered alpha release found in $repo" }
    $tag = $first.tag_name; $label = $tag.TrimStart("v")
  }
  '^v' { $tag = $Version; $label = $Version.TrimStart("v") }
  default { $tag = "v$Version"; $label = $Version }
}

$archive = "alrunner-$label-$target.zip"
$base = "https://github.com/$repo/releases/download/$tag"
$tmp = Join-Path ([IO.Path]::GetTempPath()) ("alrunner-" + [Guid]::NewGuid())
New-Item -ItemType Directory -Path $tmp | Out-Null
try {
  Write-Host "Installing alrunner $tag for $target"
  Invoke-WebRequest "$base/$archive" -OutFile (Join-Path $tmp $archive)
  Invoke-WebRequest "$base/SHA256SUMS" -OutFile (Join-Path $tmp "SHA256SUMS")
  $line = Get-Content (Join-Path $tmp "SHA256SUMS") | Where-Object { $_ -match [regex]::Escape($archive) + '$' } | Select-Object -First 1
  if (-not $line) { throw "$archive is not in SHA256SUMS" }
  $expected = ($line -split '\s+')[0].ToLower()
  $actual = (Get-FileHash (Join-Path $tmp $archive) -Algorithm SHA256).Hash.ToLower()
  if ($expected -ne $actual) { throw "Checksum mismatch for $archive" }

  Expand-Archive (Join-Path $tmp $archive) -DestinationPath $tmp -Force
  New-Item -ItemType Directory -Path $dir -Force | Out-Null
  Copy-Item (Join-Path $tmp "alrunner.exe") (Join-Path $dir "alrunner.exe") -Force
  # The binary is unsigned. Remove the download mark so SmartScreen does not block it.
  Unblock-File (Join-Path $dir "alrunner.exe")
  & (Join-Path $dir "alrunner.exe") --version
  if (($env:PATH -split ';') -notcontains $dir) { Write-Host "Add $dir to your PATH." }
} finally {
  Remove-Item $tmp -Recurse -Force -ErrorAction SilentlyContinue
}
