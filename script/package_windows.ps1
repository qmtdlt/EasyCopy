param([string]$ExpectedVersion = '')

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
if (!$IsWindows -or ![Environment]::Is64BitOperatingSystem) {
    throw 'Run with PowerShell 7 on Windows x64 and Rust MSVC.'
}
$root = Split-Path -Parent $PSScriptRoot
$desktop = Join-Path $root 'apps/desktop'
$config = Get-Content (Join-Path $desktop 'src-tauri/tauri.conf.json') -Raw | ConvertFrom-Json
$version = $config.version
if ($version -notmatch '^\d+\.\d+\.\d+$') { throw 'Windows packaging requires a stable x.y.z source version.' }
if ($ExpectedVersion -and $ExpectedVersion -ne $version) { throw "Expected $ExpectedVersion, source is $version." }
if ($config.identifier -ne 'com.easycopy.desktop' -or $config.productName -ne 'EasyCopy') {
    throw 'Application identity changed; review data and upgrade migration first.'
}
$output = Join-Path $root '.local/packages/windows-x64'
New-Item -ItemType Directory -Force $output | Out-Null
# Only remove this script's previous installer outputs, never other platform artifacts.
Get-ChildItem $output -File | Where-Object Name -Match '^EasyCopy_.*_windows_x64_setup\.exe(\.sha256)?$' | Remove-Item
Push-Location $desktop
try {
    & npm.cmd ci
    if ($LASTEXITCODE -ne 0) { throw 'npm ci failed.' }
    & npm.cmd test
    if ($LASTEXITCODE -ne 0) { throw 'Frontend tests failed.' }
    # Tauri automatically merges tauri.windows.conf.json on Windows.
    & npm.cmd run tauri -- build --ci --no-sign --target x86_64-pc-windows-msvc --bundles nsis -- --locked
    if ($LASTEXITCODE -ne 0) { throw 'Tauri NSIS build failed.' }
    $metadata = & cargo.exe metadata --no-deps --format-version 1 --locked | ConvertFrom-Json
    if ($LASTEXITCODE -ne 0) { throw 'Cargo metadata failed.' }
    $bundle = Join-Path $metadata.target_directory 'x86_64-pc-windows-msvc/release/bundle/nsis'
    $installers = @(Get-ChildItem $bundle -Filter "EasyCopy_${version}_x64-setup.exe" -File)
    if ($installers.Count -ne 1) { throw "Expected exactly one NSIS x64 installer in $bundle." }
    $destination = Join-Path $output "EasyCopy_${version}_windows_x64_setup.exe"
    Copy-Item $installers[0].FullName $destination -Force
    if ((Get-AuthenticodeSignature $destination).Status -ne 'NotSigned') { throw 'Expected an unsigned installer.' }
    $hash = (Get-FileHash $destination -Algorithm SHA256).Hash.ToLowerInvariant()
    "$hash  $([IO.Path]::GetFileName($destination))" | Set-Content "$destination.sha256" -Encoding ascii
    Write-Host "Built $destination"
} finally {
    Pop-Location
}
