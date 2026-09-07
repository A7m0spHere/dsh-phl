param(
    [Parameter(Mandatory = $true)]
    [string]$CurrentBinary,
    [string]$BaselineBinary = "",
    [int]$LargeFileMiB = 64,
    [int]$SmallFileCount = 1000,
    [int]$SmallFileKiB = 4,
    [int]$Repetitions = 3,
    [string]$OutputJson = ""
)

$ErrorActionPreference = "Stop"

function Resolve-Binary([string]$Path, [string]$Label) {
    if ([string]::IsNullOrWhiteSpace($Path)) { return $null }
    $resolved = (Resolve-Path -LiteralPath $Path).Path
    if (-not (Test-Path -LiteralPath $resolved -PathType Leaf)) {
        throw "$Label binary not found: $Path"
    }
    return $resolved
}

function Invoke-Measured([string]$Binary, [string[]]$Arguments) {
    $psi = [System.Diagnostics.ProcessStartInfo]::new()
    $psi.FileName = $Binary
    $psi.UseShellExecute = $false
    $psi.CreateNoWindow = $true
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    foreach ($arg in $Arguments) { [void]$psi.ArgumentList.Add($arg) }

    $process = [System.Diagnostics.Process]::new()
    $process.StartInfo = $psi
    $watch = [System.Diagnostics.Stopwatch]::StartNew()
    if (-not $process.Start()) { throw "Failed to start $Binary" }
    $peak = 0L
    while (-not $process.HasExited) {
        try {
            $process.Refresh()
            $peak = [Math]::Max($peak, $process.WorkingSet64)
        } catch {
            # The process may exit between HasExited and Refresh.
        }
        Start-Sleep -Milliseconds 10
    }
    $watch.Stop()
    $stdout = $process.StandardOutput.ReadToEnd()
    $stderr = $process.StandardError.ReadToEnd()
    if ($process.ExitCode -ne 0) {
        throw "Command failed ($($process.ExitCode)): $Binary $($Arguments -join ' ')`n$stdout`n$stderr"
    }
    [PSCustomObject]@{
        elapsedMs = [Math]::Round($watch.Elapsed.TotalMilliseconds, 2)
        peakWorkingSetMiB = [Math]::Round($peak / 1MB, 2)
    }
}

function Get-Median([double[]]$Values) {
    $sorted = @($Values | Sort-Object)
    if ($sorted.Count % 2 -eq 1) { return $sorted[[int]($sorted.Count / 2)] }
    return ($sorted[$sorted.Count / 2 - 1] + $sorted[$sorted.Count / 2]) / 2
}

function Measure-Binary([string]$Label, [string]$Binary, [string]$Layout, [string]$Root) {
    $runs = @()
    for ($i = 1; $i -le $Repetitions; $i++) {
        $pack = Join-Path $Root "$Label-$i.phlpack"
        $unpack = Join-Path $Root "$Label-unpack-$i"
        New-Item -ItemType Directory -Path $unpack | Out-Null
        $build = Invoke-Measured $Binary @("build", $Layout, $pack)
        $validate = Invoke-Measured $Binary @("validate", $pack)
        $extract = Invoke-Measured $Binary @("unpack", $pack, $unpack)
        $runs += [PSCustomObject]@{
            iteration = $i
            packBytes = (Get-Item -LiteralPath $pack).Length
            build = $build
            validate = $validate
            unpack = $extract
        }
    }
    [PSCustomObject]@{
        label = $Label
        binary = $Binary
        repetitions = $Repetitions
        median = [PSCustomObject]@{
            buildMs = [Math]::Round((Get-Median @($runs.build.elapsedMs)), 2)
            buildPeakMiB = [Math]::Round((Get-Median @($runs.build.peakWorkingSetMiB)), 2)
            validateMs = [Math]::Round((Get-Median @($runs.validate.elapsedMs)), 2)
            validatePeakMiB = [Math]::Round((Get-Median @($runs.validate.peakWorkingSetMiB)), 2)
            unpackMs = [Math]::Round((Get-Median @($runs.unpack.elapsedMs)), 2)
            unpackPeakMiB = [Math]::Round((Get-Median @($runs.unpack.peakWorkingSetMiB)), 2)
        }
        runs = $runs
    }
}

$current = Resolve-Binary $CurrentBinary "current"
$baseline = Resolve-Binary $BaselineBinary "baseline"
$tempBase = [System.IO.Path]::GetFullPath([System.IO.Path]::GetTempPath())
$tempRoot = [System.IO.Path]::GetFullPath((Join-Path $tempBase ("phl-pack-bench-" + [guid]::NewGuid().ToString("N"))))
if (-not $tempRoot.StartsWith($tempBase, [System.StringComparison]::OrdinalIgnoreCase)) {
    throw "Refusing benchmark temp path outside system temp: $tempRoot"
}

try {
    $layout = Join-Path $tempRoot "layout"
    $plugin = Join-Path $layout "embedded/plugins/benchmark"
    $smallDir = Join-Path $plugin "small"
    New-Item -ItemType Directory -Path $smallDir -Force | Out-Null

    $manifest = @{
        formatVersion = 1
        pack = @{ id = "benchmark"; name = "Benchmark"; version = "1.0.0" }
        dsh = @{ version = "benchmark" }
        runtime = @{ kind = "node"; nodeVersion = "22" }
        plugins = @(@{
            id = "benchmark"
            version = "1.0.0"
            source = @{ type = "embedded"; path = "embedded/plugins/benchmark" }
        })
        content = @{ sessionsIncluded = $false; secretsExcluded = $true }
    } | ConvertTo-Json -Depth 8
    Set-Content -LiteralPath (Join-Path $layout "phlpack.json") -Value $manifest -Encoding utf8NoBOM
    Set-Content -LiteralPath (Join-Path $plugin "package.json") -Value '{"name":"benchmark","version":"1.0.0"}' -Encoding utf8NoBOM

    $random = [System.Random]::new(20260907)
    $largePath = Join-Path $plugin "large.bin"
    $stream = [System.IO.File]::Create($largePath)
    try {
        $buffer = [byte[]]::new(1MB)
        for ($i = 0; $i -lt $LargeFileMiB; $i++) {
            $random.NextBytes($buffer)
            $stream.Write($buffer, 0, $buffer.Length)
        }
    } finally {
        $stream.Dispose()
    }
    $smallBuffer = [byte[]]::new($SmallFileKiB * 1KB)
    for ($i = 0; $i -lt $SmallFileCount; $i++) {
        $random.NextBytes($smallBuffer)
        [System.IO.File]::WriteAllBytes((Join-Path $smallDir ("entry-{0:D5}.bin" -f $i)), $smallBuffer)
    }

    $results = @()
    if ($baseline) { $results += Measure-Binary "baseline" $baseline $layout $tempRoot }
    $results += Measure-Binary "current" $current $layout $tempRoot
    $report = [PSCustomObject]@{
        generatedAt = [DateTimeOffset]::Now.ToString("o")
        dataset = [PSCustomObject]@{
            largeFileMiB = $LargeFileMiB
            smallFileCount = $SmallFileCount
            smallFileKiB = $SmallFileKiB
            totalPayloadBytes = (Get-ChildItem -LiteralPath $plugin -File -Recurse | Measure-Object Length -Sum).Sum
        }
        results = $results
    }
    $json = $report | ConvertTo-Json -Depth 8
    if ($OutputJson) {
        $outputPath = [System.IO.Path]::GetFullPath($OutputJson)
        Set-Content -LiteralPath $outputPath -Value $json -Encoding utf8NoBOM
    }
    $report.results | Select-Object label, @{n='build ms';e={$_.median.buildMs}}, @{n='build MiB';e={$_.median.buildPeakMiB}}, @{n='validate ms';e={$_.median.validateMs}}, @{n='validate MiB';e={$_.median.validatePeakMiB}}, @{n='unpack ms';e={$_.median.unpackMs}}, @{n='unpack MiB';e={$_.median.unpackPeakMiB}} | Format-Table -AutoSize
    $json
} finally {
    if (Test-Path -LiteralPath $tempRoot) {
        $resolved = [System.IO.Path]::GetFullPath($tempRoot)
        if ($resolved.StartsWith($tempBase, [System.StringComparison]::OrdinalIgnoreCase)) {
            Remove-Item -LiteralPath $resolved -Recurse -Force
        }
    }
}
