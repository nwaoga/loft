<#
  Makes a Windows release of Loft: sets the version number, checks the code,
  builds the app and its installer, and puts the installer in
  releases\windows\<version>\ with its checksum and a short description.

  Run it through release.bat, which first selects the MSVC build tools:

    scripts\windows\release.bat 0.2.0        release version 0.2.0
    scripts\windows\release.bat patch        0.1.0 -> 0.1.1 (also: minor, major)
    scripts\windows\release.bat              the current version again

  Options:
    -Msi        also make an .msi (for deployment tools); the .exe installer is always made
    -SkipTests  skip the type check and the unit tests
    -Force      replace a release of the same version that was made before

  The version number lives in package.json (see scripts\set-version.mjs); the
  app, Settings and the installer all take it from there.
#>
param(
  [Parameter(Position = 0)][string]$Version = "",
  [switch]$Msi,
  [switch]$SkipTests,
  [switch]$Force
)

$ErrorActionPreference = "Stop"
$root = (Resolve-Path (Join-Path $PSScriptRoot "..\..")).Path
Set-Location $root

function Step($text) { Write-Host ""; Write-Host "== $text" -ForegroundColor Cyan }
function Fail($text) { Write-Host ""; Write-Host "release: $text" -ForegroundColor Red; exit 1 }
function Run($what, [scriptblock]$command) {
  & $command
  if ($LASTEXITCODE -ne 0) { Fail "$what failed (exit code $LASTEXITCODE)" }
}

# --- What is needed --------------------------------------------------------
Step "Checking the tools"
foreach ($tool in "node", "npm", "cargo") {
  if (-not (Get-Command $tool -ErrorAction SilentlyContinue)) { Fail "$tool is not installed (see Prerequisites in README.md)" }
}
if (-not (Get-Command link.exe -ErrorAction SilentlyContinue)) {
  Fail "the MSVC build tools are not selected: run this through scripts\windows\release.bat"
}
if (-not (Test-Path "node_modules")) {
  Step "Installing the JavaScript packages (npm ci)"
  Run "npm ci" { npm ci }
}
# The llama.cpp builds go into the installer: without them Loft cannot run a model.
$engines = @("win-x64-cpu", "win-x64-vulkan") | Where-Object { Test-Path "src-tauri\binaries\$_\llama-server.exe" }
if ($engines.Count -eq 0) {
  Fail "no llama.cpp builds in src-tauri\binaries: run 'npm run fetch-llama' first (it downloads them)"
}
foreach ($icon in "icon.ico", "32x32.png", "128x128.png") {
  if (-not (Test-Path "src-tauri\icons\$icon")) { Fail "src-tauri\icons\$icon is missing: run 'npm run icons'" }
}

# --- The version -----------------------------------------------------------
Step "Version"
$before = (node scripts/set-version.mjs).Trim()
if ($Version) {
  $release = (node scripts/set-version.mjs $Version)
  if ($LASTEXITCODE -ne 0) { Fail "the version was not changed" }
  $release = $release.Trim()
} else {
  $release = $before
}
Write-Host "Loft $release$(if ($release -ne $before) { " (was $before)" })"

$out = Join-Path $root "releases\windows\$release"
if ((Test-Path $out) -and -not $Force) {
  Fail "releases\windows\$release already exists. Give a new version number, or add -Force to make it again."
}

# --- Checks ----------------------------------------------------------------
if (-not $SkipTests) {
  Step "Type check (svelte-check)"
  Run "the type check" { npm run check -- --threshold error }
  Step "Unit tests (cargo test)"
  Push-Location src-tauri
  try { Run "the unit tests" { cargo test --lib --quiet } } finally { Pop-Location }
}

# --- Build -----------------------------------------------------------------
$bundles = if ($Msi) { "nsis,msi" } else { "nsis" }
Step "Building Loft $release and its installer ($bundles)"
Run "the build" { npx tauri build --bundles $bundles }

# --- Collect ---------------------------------------------------------------
Step "Collecting the release"
$bundleDir = Join-Path $root "src-tauri\target\release\bundle"
$made = @(Get-Item (Join-Path $bundleDir "nsis\Loft_${release}_x64-setup.exe") -ErrorAction SilentlyContinue)
if ($Msi) { $made += @(Get-Item (Join-Path $bundleDir "msi\Loft_${release}_x64_en-US.msi") -ErrorAction SilentlyContinue) }
if ($made.Count -eq 0 -or -not $made[0]) { Fail "the build finished but no installer for $release was found in $bundleDir" }

if (Test-Path $out) { Remove-Item $out -Recurse -Force }
New-Item -ItemType Directory -Path $out | Out-Null
$sums = @()
foreach ($file in $made) {
  Copy-Item $file.FullName $out
  $hash = (Get-FileHash $file.FullName -Algorithm SHA256).Hash.ToLower()
  $sums += "$hash  $($file.Name)"
}
Set-Content -Path (Join-Path $out "SHA256SUMS.txt") -Value $sums -Encoding ascii

$llama = (Get-Content "src-tauri\engines.json" -Raw | ConvertFrom-Json).tag
$lines = @(
  "Loft $release for Windows (x64)",
  "Built: $(Get-Date -Format 'yyyy-MM-dd HH:mm') on $env:COMPUTERNAME",
  "llama.cpp: $llama",
  "Engines in the installer: $($engines -join ', ') (CUDA and OpenVINO are added from Settings)",
  "",
  "Install: run $($made[0].Name). It installs for the current user, with no administrator rights,",
  "and adds Loft to the Start menu. Installing a newer version replaces the older one; settings,",
  "models and indexes are kept (they are in %APPDATA%\com.loft.desktop).",
  "Unsigned: Windows SmartScreen may warn. Choose More info, then Run anyway.",
  "",
  "Files:"
) + ($made | ForEach-Object { "  $($_.Name)  ($([math]::Round($_.Length / 1MB, 1)) MB)" })
Set-Content -Path (Join-Path $out "RELEASE.txt") -Value $lines -Encoding utf8

Step "Done"
Write-Host "Loft $release is in releases\windows\$release\" -ForegroundColor Green
Get-ChildItem $out | ForEach-Object { Write-Host ("  {0,-34} {1,8:N1} MB" -f $_.Name, ($_.Length / 1MB)) }
