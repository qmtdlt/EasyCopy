# Destructive installation smoke test for a fresh disposable GitHub runner only.
# Never run this against a developer's installed app or real profile.
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
if (!$IsWindows -or $env:GITHUB_ACTIONS -ne 'true' -or $env:RUNNER_ENVIRONMENT -ne 'github-hosted') {
    throw 'This smoke test requires a disposable GitHub-hosted Windows runner.'
}
$root = Split-Path -Parent $PSScriptRoot
$config = Get-Content (Join-Path $root 'apps/desktop/src-tauri/tauri.conf.json') -Raw | ConvertFrom-Json
$version = $config.version
$installer = Join-Path $root ".local/packages/windows-x64/EasyCopy_${version}_windows_x64_setup.exe"
$installDir = Join-Path $env:LOCALAPPDATA 'EasyCopy'
$appExe = Join-Path $installDir 'EasyCopy.exe'
$profileDir = Join-Path $env:APPDATA $config.identifier
$localProfileDir = Join-Path $env:LOCALAPPDATA $config.identifier
$registryPath = 'Software\Microsoft\Windows\CurrentVersion\Uninstall\EasyCopy'
$registry = [Microsoft.Win32.RegistryKey]::OpenBaseKey('CurrentUser', 'Registry64')
$reportPath = Join-Path $root '.local/verification/windows-package.json'
$report = [ordered]@{ version = $version; commit = $env:GITHUB_SHA; checks = @(); result = 'failed'; interactiveDesktop = 'not tested'; realClipboardSync = 'not tested'; loginAutostart = 'not tested'; differentVersionUpgrade = 'not tested' }

function Check([bool]$Condition, [string]$Description) {
    if (!$Condition) { throw $Description }
    $script:report.checks += $Description
}
function Run-Installer([string]$Path, [string]$Arguments) {
    $process = Start-Process -FilePath $Path -ArgumentList $Arguments -PassThru
    if (!$process.WaitForExit(180000)) { $process.Kill(); throw "Installer timed out: $Path" }
    if ($process.ExitCode -ne 0) { throw "Installer exit code $($process.ExitCode): $Path" }
}
function Assert-Installed {
    Check (Test-Path $appExe) 'Installed executable exists at the default per-user path'
    $key = $registry.OpenSubKey($registryPath)
    try {
        Check ($null -ne $key) 'Per-user uninstall registry entry exists'
        Check ($key.GetValue('DisplayVersion') -eq $version) 'Installed registry version matches source'
    } finally { if ($key) { $key.Dispose() } }
    $fileVersion = (Get-Item $appExe).VersionInfo.ProductVersion
    Check ($fileVersion -eq $version -or $fileVersion -eq "$version.0") 'Installed executable version matches source'
}

try {
    $existingKey = $registry.OpenSubKey($registryPath)
    if ($existingKey) { $existingKey.Dispose(); throw 'An existing EasyCopy installation was found.' }
    foreach ($path in @($installDir, $profileDir, $localProfileDir)) {
        if (Test-Path $path) { throw "Refusing to touch existing data: $path" }
    }
    Check ((Get-AuthenticodeSignature $installer).Status -eq 'NotSigned') 'Installer is unsigned as requested'
    Run-Installer $installer '/S'
    Assert-Installed
    Check ((Get-AuthenticodeSignature $appExe).Status -eq 'NotSigned') 'Installed application is unsigned'
    $bytes = [IO.File]::ReadAllBytes($appExe)
    $peOffset = [BitConverter]::ToInt32($bytes, 0x3c)
    Check ([BitConverter]::ToUInt16($bytes, $peOffset + 4) -eq 0x8664) 'Application PE architecture is AMD64'
    $shell = New-Object -ComObject WScript.Shell
    $shortcuts = @(
        (Join-Path ([Environment]::GetFolderPath('Programs')) 'EasyCopy.lnk'),
        (Join-Path ([Environment]::GetFolderPath('DesktopDirectory')) 'EasyCopy.lnk')
    )
    foreach ($shortcut in $shortcuts) {
        Check (Test-Path $shortcut) "Shortcut exists: $([IO.Path]::GetFileName($shortcut))"
        Check ($shell.CreateShortcut($shortcut).TargetPath -eq $appExe) 'Shortcut targets the installed application'
    }
    # A sentinel checks preservation without introducing pairing credentials or real test data.
    New-Item -ItemType Directory -Force $profileDir | Out-Null
    $sentinel = Join-Path $profileDir 'installer-preservation-check.txt'
    'disposable-ci-sentinel' | Set-Content $sentinel
    $sentinelHash = (Get-FileHash $sentinel).Hash
    Run-Installer $installer '/S'
    Assert-Installed
    Check ((Get-FileHash $sentinel).Hash -eq $sentinelHash) 'Same-version reinstall preserves the existing profile sentinel'
    Run-Installer (Join-Path $installDir 'uninstall.exe') "/S _?=$installDir"
    Check (!(Test-Path $appExe)) 'Uninstall removes the application executable'
    foreach ($shortcut in $shortcuts) {
        Check (!(Test-Path $shortcut)) 'Uninstall removes the application shortcut'
    }
    $key = $registry.OpenSubKey($registryPath)
    try { Check ($null -eq $key) 'Uninstall removes its registry entry' }
    finally { if ($key) { $key.Dispose() } }
    Check ((Get-FileHash $sentinel).Hash -eq $sentinelHash) 'Silent uninstall preserves user profile data by default'
    $report.result = 'passed'
} catch {
    $report.error = $_.Exception.Message
    throw
} finally {
    $registry.Dispose()
    New-Item -ItemType Directory -Force (Split-Path $reportPath) | Out-Null
    $report | ConvertTo-Json -Depth 5 | Set-Content $reportPath -Encoding utf8
    if ($env:GITHUB_STEP_SUMMARY) {
        "Windows package smoke test: **$($report.result)**. Checks: $($report.checks.Count). No real clipboard, login-autostart or different-version upgrade acceptance." | Add-Content $env:GITHUB_STEP_SUMMARY
    }
}
