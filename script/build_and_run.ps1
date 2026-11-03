param(
    [switch]$Build,
    [switch]$Dev,
    [switch]$Acceptance
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$desktop = Join-Path $root 'apps\desktop'
$output = Join-Path $root '.local\windows-client'
$executable = Join-Path $output 'EasyCopy.exe'
$previousProfile = $env:EASYCOPY_PROFILE_DIR

try {
    # Do not overwrite the portable executable while it is running.
    $projectExecutables = @(
        $executable,
        (Join-Path $root 'target\debug\EasyCopy.exe'),
        (Join-Path $root 'target\release\EasyCopy.exe')
    )
    $running = Get-Process -Name EasyCopy -ErrorAction SilentlyContinue |
        Where-Object { $_.Path -in $projectExecutables }
    if ($running) {
        if ($Build -or $Dev -or $Acceptance) {
            throw 'Exit EasyCopy from its system tray before rebuilding or changing profiles.'
        }
        Write-Host 'EasyCopy is already running. Open it from the system tray.'
        return
    }
    if ($Acceptance) {
        $env:EASYCOPY_PROFILE_DIR = Join-Path $root '.local\windows-acceptance'
    }
    if ($Dev -or $Build -or !(Test-Path -LiteralPath $executable)) {
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
            & npm.cmd run tauri -- build --debug --no-bundle -- --locked
            if ($LASTEXITCODE -ne 0) { throw 'Tauri build failed.' }
            $metadata = (& cargo.exe metadata --no-deps --format-version 1 --locked | ConvertFrom-Json)
            if ($LASTEXITCODE -ne 0) { throw 'Unable to locate the Cargo output directory.' }
            New-Item -ItemType Directory -Path $output -Force | Out-Null
            Copy-Item -LiteralPath (Join-Path $metadata.target_directory 'debug\EasyCopy.exe') -Destination $executable -Force
        } finally {
            Pop-Location
        }
    }
    Start-Process -FilePath $executable -WorkingDirectory $root -WindowStyle Hidden
    Write-Host "Started EasyCopy: $executable"
} finally {
    $env:EASYCOPY_PROFILE_DIR = $previousProfile
}
