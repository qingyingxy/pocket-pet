[CmdletBinding()]
param([switch]$Offline, [switch]$SkipBuild, [switch]$NativeHover)
$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path -Parent $PSScriptRoot
$cargo = Get-Command cargo -ErrorAction SilentlyContinue
if (!$cargo) {
    $cargoBin = Join-Path $env:USERPROFILE '.cargo\bin'
    if (!(Test-Path -LiteralPath (Join-Path $cargoBin 'cargo.exe'))) { throw 'Rust cargo is not installed.' }
    $env:PATH = $cargoBin + ';' + $env:PATH
}
$manifest = Join-Path $repoRoot 'slint-preview\Cargo.toml'
$exe = Join-Path $repoRoot 'slint-preview\target\release\pocket-pet-slint.exe'
if (@(Get-Process pocket-pet-slint -ErrorAction SilentlyContinue).Count) {
    throw 'Exit Pocket Pet before verifying: the hotkey integration test needs Ctrl+Alt+V.'
}
function Invoke-Cargo([string[]]$Arguments) {
    & cargo @Arguments
    if ($LASTEXITCODE -ne 0) { throw "cargo failed: $($Arguments -join ' ')" }
}
Invoke-Cargo @('fmt', '--manifest-path', $manifest, '--', '--check')
$common = @('--locked', '--manifest-path', $manifest)
if ($Offline) { $common += '--offline' }
Invoke-Cargo (@('test') + $common)
if (!$SkipBuild) { Invoke-Cargo (@('build', '--release') + $common) }
if (!(Test-Path -LiteralPath $exe)) { throw 'Release binary missing. Run this script without -SkipBuild.' }
$runRoot = Join-Path $repoRoot ('slint-preview\preview-output\validation\' + (Get-Date -Format 'yyyyMMdd-HHmmss') + '-' + [guid]::NewGuid().ToString('N').Substring(0, 8))
[void][IO.Directory]::CreateDirectory($runRoot)
$results = @()
function Invoke-Preview([string]$Name, [string[]]$Flags, [string[]]$Reports) {
    $out = Join-Path $runRoot $Name
    $arguments = @('--snapshot', '--snapshot-output', ('"' + $out + '"')) + $Flags
    $checkProcess = Start-Process -FilePath $exe -ArgumentList $arguments -WindowStyle Hidden -PassThru
    try {
        if (!$checkProcess.WaitForExit(60000)) { throw "Preview timed out: $Name" }
        if ($checkProcess.ExitCode -ne 0) { throw "Preview failed ($($checkProcess.ExitCode)): $Name" }
        $errorReport = Join-Path $out 'snapshot-error.txt'
        if (Test-Path -LiteralPath $errorReport) { throw (Get-Content -LiteralPath $errorReport -Raw) }
        foreach ($report in $Reports) {
            $file = Join-Path $out $report
            if (!(Test-Path -LiteralPath $file)) { throw "Missing report: $file" }
            $content = Get-Content -LiteralPath $file -Raw
            if ($content -notmatch '^PASS\b' -or $content -match '\bFAIL\b|=false\b') { throw "Failed report: $file`n$content" }
            Write-Host "PASS $Name/$report"
        }
        [PSCustomObject]@{ suite = $Name; status = 'PASS'; reports = $Reports; output = $out }
    } finally {
        # Only the isolated preview started here may be stopped on timeout.
        if (!$checkProcess.HasExited) { Stop-Process -Id $checkProcess.Id -Force }
        $checkProcess.Dispose()
    }
}
$flowReports = @('input-check.txt', 'image-preview-check.txt', 'completion-check.txt', 'reminder-check.txt', 'side-rail-check.txt', 'hover-preview-check.txt', 'hover-cycles-check.txt', 'hover-quick-check.txt', 'drop-registration.txt')
$flags = @()
if ($NativeHover) { $flags += '--native-hover' }
$results += Invoke-Preview 'flow' $flags $flowReports
$results += Invoke-Preview 'deletion' @('--delete-check') @('deletion-check.txt', 'drop-registration.txt')
$results += Invoke-Preview 'tray' @('--tray-check') @('tray-check.txt', 'drop-registration.txt')
$results += Invoke-Preview 'pet' @('--pet-check') @('pet-boundary-check.txt', 'drop-registration.txt')
$results | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath (Join-Path $runRoot 'validation.json') -Encoding UTF8
Write-Output "Validation passed. Reports: $runRoot"
