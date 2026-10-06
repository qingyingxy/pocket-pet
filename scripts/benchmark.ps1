[CmdletBinding()]
param(
    [ValidateRange(2, 60)][int]$Seconds = 8,
    [ValidateRange(0, 10000)][int[]]$Records = @(0, 100, 1000),
    [switch]$Images,
    [switch]$SkipBuild,
    [switch]$Offline
)
$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path -Parent $PSScriptRoot
$exe = Join-Path $repoRoot 'slint-preview\target\release\pocket-pet-slint.exe'
if (!$SkipBuild) {
    if (!(Get-Command cargo -ErrorAction SilentlyContinue)) {
        $env:PATH = (Join-Path $env:USERPROFILE '.cargo\bin') + ';' + $env:PATH
    }
    $buildArgs = @('build', '--release', '--locked', '--manifest-path', (Join-Path $repoRoot 'slint-preview\Cargo.toml'))
    if ($Offline) { $buildArgs += '--offline' }
    & cargo @buildArgs
    if ($LASTEXITCODE -ne 0) { throw 'Release build failed.' }
}
if (!(Test-Path -LiteralPath $exe)) { throw 'Release binary missing.' }
$runRoot = Join-Path $repoRoot ('slint-preview\preview-output\benchmarks\' + (Get-Date -Format 'yyyyMMdd-HHmmss') + '-' + [guid]::NewGuid().ToString('N').Substring(0, 8))
[void][IO.Directory]::CreateDirectory($runRoot)
$results = @()
foreach ($count in $Records) {
    $scenario = Join-Path $runRoot ($count.ToString() + '-' + [guid]::NewGuid().ToString('N').Substring(0, 8))
    $imageCount = 0
    if ($Images) { $imageCount = [Math]::Min($count, 64) }
    $arguments = @('--benchmark', '--benchmark-records', $count, '--benchmark-images', $imageCount, '--benchmark-seconds', $Seconds, '--benchmark-output', ('"' + $scenario + '"'))
    $sampleProcess = Start-Process -FilePath $exe -ArgumentList $arguments -WindowStyle Hidden -PassThru
    $samples = @()
    $watch = [Diagnostics.Stopwatch]::StartNew()
    $lastPhase = ''; $lastTime = 0.0; $lastCpu = 0.0
    try {
        while (!$sampleProcess.HasExited) {
            if ($watch.Elapsed.TotalSeconds -gt (90 + $Seconds * 2)) { throw "Benchmark timed out: $count records" }
            $sampleProcess.Refresh()
            if ($sampleProcess.HasExited) { break }
            $phaseFile = Join-Path $scenario 'phase.txt'
            $phase = 'starting'
            if (Test-Path -LiteralPath $phaseFile) {
                # A short Windows replacement/share conflict is not a new phase.
                try { $phase = [IO.File]::ReadAllText($phaseFile) } catch { $phase = $lastPhase }
            }
            $time = $watch.Elapsed.TotalSeconds
            $cpu = $sampleProcess.TotalProcessorTime.TotalSeconds
            if ($phase -eq $lastPhase -and @('idle', 'panel') -contains $phase -and $time -gt $lastTime) {
                $samples += [PSCustomObject]@{
                    phase = $phase; elapsed_seconds = $time; delta_seconds = $time - $lastTime; cpu_seconds = $cpu - $lastCpu
                    working_set_mib = $sampleProcess.WorkingSet64 / 1MB; private_mib = $sampleProcess.PrivateMemorySize64 / 1MB
                    handles = $sampleProcess.HandleCount
                }
            }
            $lastPhase = $phase; $lastTime = $time; $lastCpu = $cpu
            Start-Sleep -Milliseconds 250
        }
        $sampleProcess.WaitForExit()
        if ($sampleProcess.ExitCode -ne 0) { throw "Benchmark failed ($($sampleProcess.ExitCode)): $scenario" }
        $errorFile = Join-Path $scenario 'benchmark-error.txt'
        if (Test-Path -LiteralPath $errorFile) { throw (Get-Content -LiteralPath $errorFile -Raw) }
        $timingsFile = Join-Path $scenario 'timings.json'
        if (!(Test-Path -LiteralPath $timingsFile)) { throw "Missing timings: $scenario" }
        $timings = Get-Content -LiteralPath $timingsFile -Raw | ConvertFrom-Json
        $phases = @()
        foreach ($phaseName in @('idle', 'panel')) {
            $phaseSamples = @($samples | Where-Object { $_.phase -eq $phaseName })
            if ($phaseSamples.Count -lt 3) { throw "Insufficient samples for $phaseName in $scenario" }
            $wall = ($phaseSamples | Measure-Object delta_seconds -Sum).Sum
            $cpu = ($phaseSamples | Measure-Object cpu_seconds -Sum).Sum
            $memory = $phaseSamples | Measure-Object working_set_mib -Average -Maximum
            $private = $phaseSamples | Measure-Object private_mib -Average -Maximum
            $phases += [PSCustomObject]@{
                phase = $phaseName; samples = $phaseSamples.Count; sampled_seconds = [Math]::Round($wall, 3)
                cpu_percent_one_core = [Math]::Round(100 * $cpu / $wall, 3)
                working_set_mib_mean = [Math]::Round($memory.Average, 2); working_set_mib_peak = [Math]::Round($memory.Maximum, 2)
                private_mib_mean = [Math]::Round($private.Average, 2); private_mib_peak = [Math]::Round($private.Maximum, 2)
            }
        }
        $samples | Export-Csv -LiteralPath (Join-Path $scenario 'samples.csv') -NoTypeInformation -Encoding UTF8
        $results += [PSCustomObject]@{ records = $count; images = $imageCount; timings = $timings; phases = $phases; output = $scenario }
        Write-Output "Measured $count records / $imageCount images"
    } finally {
        if (!$sampleProcess.HasExited) { Stop-Process -Id $sampleProcess.Id -Force }
        $sampleProcess.Dispose()
    }
}
$report = [PSCustomObject]@{
    recorded_at = (Get-Date).ToString('o'); os = [Environment]::OSVersion.VersionString
    logical_processors = [Environment]::ProcessorCount; executable_bytes = (Get-Item -LiteralPath $exe).Length
    scenarios = $results; limitations = @('Synthetic data; no global hotkey, OLE drop target or tray.', 'One run per scenario; CPU percent uses one logical core as 100%.', 'Open callback and offscreen snapshot timings are not end-to-end input latency.')
}
$report | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath (Join-Path $runRoot 'benchmark.json') -Encoding UTF8
Write-Output "Benchmark saved: $runRoot"
$results | ForEach-Object { foreach ($phase in $_.phases) { [PSCustomObject]@{ Records = $_.records; Images = $_.images; Phase = $phase.phase; CPU = $phase.cpu_percent_one_core; WorkingSetMiB = $phase.working_set_mib_mean; OpenMs = [Math]::Round($_.timings.open_callback_ms, 2) } } } | Format-Table -AutoSize
