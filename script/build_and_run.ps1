param(
    [switch]$Build,
    [switch]$Dev,
    [switch]$Acceptance,
    [switch]$DebugBuild
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$desktop = Join-Path $root 'apps\desktop'
$output = Join-Path $root '.local\windows-client'
$buildProfile = 'release'
if ($DebugBuild) {
    $buildProfile = 'debug'
    $output = Join-Path $root '.local\windows-debug-client'
}
$executable = Join-Path $output 'EasyCopy.exe'
$buildProfileMarker = Join-Path $output 'build-profile.txt'
$needsBuild = !(Test-Path -LiteralPath $executable) -or
    !(Test-Path -LiteralPath $buildProfileMarker)
if (!$needsBuild) {
    $needsBuild = (Get-Content -LiteralPath $buildProfileMarker -Raw).Trim() -ne $buildProfile
}
$previousProfile = $env:EASYCOPY_PROFILE_DIR

try {
    # Do not overwrite the portable executable while it is running.
    $projectExecutables = @(
        $executable,
        (Join-Path $root '.local\windows-client\EasyCopy.exe'),
        (Join-Path $root '.local\windows-debug-client\EasyCopy.exe'),
        (Join-Path $root 'target\debug\EasyCopy.exe'),
        (Join-Path $root 'target\release\EasyCopy.exe')
    )
    $running = Get-Process -Name EasyCopy -ErrorAction SilentlyContinue |
        Where-Object { $_.Path -in $projectExecutables }
    if ($running) {
        if ($Build -or $Dev -or $Acceptance -or $DebugBuild -or $needsBuild) {
            throw 'Exit EasyCopy from its system tray before rebuilding or changing profiles.'
        }
        Write-Host 'EasyCopy is already running. Open it from the system tray.'
        return
    }
    if ($Acceptance) {
        $env:EASYCOPY_PROFILE_DIR = Join-Path $root '.local\windows-acceptance'
    }
    if ($Dev -or $Build -or $needsBuild) {
        foreach ($command in @('node.exe', 'npm.cmd', 'cargo.exe')) {
            if (!(Get-Command $command -ErrorAction SilentlyContinue)) {
                throw "Missing $command. Install Node.js 22+ and Rust MSVC; see README.md."
            }
        }
        $nodeMajor = [int]((& node.exe --version).TrimStart('v').Split('.')[0])
        if ($nodeMajor -lt 22) { throw 'Node.js 22 or newer is required.' }
        Push-Location $desktop
        try {
            & npm.cmd ci
            if ($LASTEXITCODE -ne 0) { throw 'npm ci failed.' }
            if ($Dev) {
                & npm.cmd run tauri -- dev -- --locked
                if ($LASTEXITCODE -ne 0) { throw 'Tauri development run failed.' }
                return
            }
            # Build with embedded frontend assets. A tauri-dev executable requires Vite.
            if ($DebugBuild) {
                & npm.cmd run tauri -- build --debug --no-bundle -- --locked
            } else {
                & npm.cmd run tauri -- build --no-bundle -- --locked
            }
            if ($LASTEXITCODE -ne 0) { throw 'Tauri build failed.' }
            $metadata = (& cargo.exe metadata --no-deps --format-version 1 --locked | ConvertFrom-Json)
            if ($LASTEXITCODE -ne 0) { throw 'Unable to locate the Cargo output directory.' }
            New-Item -ItemType Directory -Path $output -Force | Out-Null
            Copy-Item -LiteralPath (Join-Path $metadata.target_directory "$buildProfile\EasyCopy.exe") -Destination $executable -Force
            Set-Content -LiteralPath $buildProfileMarker -Value $buildProfile -Encoding Ascii
        } finally {
            Pop-Location
        }
    }
    Start-Process -FilePath $executable -WorkingDirectory $root -WindowStyle Hidden
    Write-Host "Started EasyCopy: $executable"
} finally {
    $env:EASYCOPY_PROFILE_DIR = $previousProfile
}
