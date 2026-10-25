param([switch]$Bench)

$ErrorActionPreference = 'Stop'
try { [Console]::OutputEncoding = [System.Text.Encoding]::UTF8 } catch {}

$root = Split-Path -Parent $PSScriptRoot
$runs = 3
$timeoutMs = 60000
$folders = @('flat_10k', 'flat_100k')
$keys = @('entries', 'start_to_first_frame_ms', 'first_rows_ms', 'full_listing_ms', 'scroll_frames', 'frame_period_ms', 'dropped_frames', 'frame_cpu_p50_ms', 'frame_cpu_p99_ms', 'input_to_frame_p50_ms', 'input_to_frame_max_ms', 'sort_ms')
$flit = Join-Path $root 'target\release\flit.exe'
$gen = Join-Path $root 'target\release\flit-gen.exe'
$testdata = Join-Path $root 'target\testdata'
$outDir = Join-Path $root 'target\metrics'
$report = Join-Path $root 'docs\metrics.md'

Push-Location $root
try {
    & cargo build --release -p flit -p flit-gen
    if ($LASTEXITCODE -ne 0) { throw "cargo build failed ($LASTEXITCODE)" }

    & $gen $testdata --all
    if ($LASTEXITCODE -ne 0) { throw "flit-gen failed ($LASTEXITCODE)" }

    $benchLines = @()
    if ($Bench) {
        & cargo bench -p flit-core --bench core -- --noplot | Tee-Object -Variable benchOut
        if ($LASTEXITCODE -ne 0) { throw "cargo bench failed ($LASTEXITCODE)" }
        $prev = ''
        foreach ($line in $benchOut) {
            if ($line -match '^(.*?)\s*time:\s*\[(.+)\]') {
                $name = $Matches[1].Trim()
                if (-not $name) { $name = $prev.Trim() }
                $benchLines += "| $name | $($Matches[2]) |"
            }
            if ($line.Trim()) { $prev = $line }
        }
    }
}
finally {
    Pop-Location
}

New-Item -ItemType Directory -Force $outDir | Out-Null

function Get-Median($values) {
    $nums = @($values | Where-Object { $null -ne $_ } | ForEach-Object { [double]$_ } | Sort-Object)
    if ($nums.Count -eq 0) { return $null }
    return $nums[[int][math]::Floor(($nums.Count - 1) / 2)]
}

$results = @{}
foreach ($folder in $folders) {
    $samples = @()
    for ($i = 1; $i -le $runs; $i++) {
        $json = Join-Path $outDir "$folder-$i.json"
        Remove-Item -LiteralPath $json -ErrorAction SilentlyContinue
        $dir = Join-Path $testdata $folder
        Write-Host "flit --metrics $folder (run $i/$runs)"
        $p = Start-Process -FilePath $flit -ArgumentList '--metrics', "`"$json`"", "`"$dir`"" -PassThru
        if (-not $p.WaitForExit($timeoutMs)) {
            $p.Kill()
            Write-Warning "$folder run $i timed out after $($timeoutMs / 1000) s, killed"
            continue
        }
        if (Test-Path -LiteralPath $json) {
            $samples += Get-Content -LiteralPath $json -Raw | ConvertFrom-Json
        }
        else {
            Write-Warning "$folder run $i exited ($($p.ExitCode)) without writing $json"
        }
    }
    $median = @{}
    foreach ($k in $keys) { $median[$k] = Get-Median ($samples | ForEach-Object { $_.$k }) }
    $median['runs'] = $samples.Count
    $results[$folder] = $median
}

function Get-DiskInfo([string]$path) {
    try {
        $letter = (Split-Path -Qualifier $path).TrimEnd(':')
        $part = Get-Partition -DriveLetter $letter
        $disk = Get-PhysicalDisk | Where-Object { $_.DeviceId -eq [string]$part.DiskNumber } | Select-Object -First 1
        return "${letter}: $($disk.FriendlyName) ($($disk.MediaType), $($disk.BusType))"
    }
    catch {
        return "${path}: unknown ($($_.Exception.Message))"
    }
}

$cpu = Get-CimInstance Win32_Processor | Select-Object -First 1
$cs = Get-CimInstance Win32_ComputerSystem
$os = Get-CimInstance Win32_OperatingSystem
$ubr = (Get-ItemProperty 'HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion' -ErrorAction SilentlyContinue).UBR
$gpus = (Get-CimInstance Win32_VideoController | ForEach-Object {
        if ($_.CurrentRefreshRate) { "$($_.Name) ($($_.CurrentRefreshRate) Hz)" } else { $_.Name }
    }) -join ', '

function Format-Value($v) {
    if ($null -eq $v) { return 'n/a' }
    return ([double]$v).ToString('0.##', [Globalization.CultureInfo]::InvariantCulture)
}

function Test-Target($r, $key, $op, $limit) {
    $v = $r[$key]
    if ($null -eq $v) { return @('n/a', 'FAIL') }
    $ok = switch ($op) { 'lt' { $v -lt $limit } 'le' { $v -le $limit } 'eq' { $v -eq $limit } }
    return @((Format-Value $v), $(if ($ok) { 'PASS' } else { 'FAIL' }))
}

$r10 = $results['flat_10k']
$r100 = $results['flat_100k']
$rows = @(
    @('Start -> first frame (10k)', '< 150 ms', $r10, 'start_to_first_frame_ms', 'lt', 150),
    @('Start -> first frame (100k)', '< 150 ms', $r100, 'start_to_first_frame_ms', 'lt', 150),
    @('10k first rows', '< 16 ms', $r10, 'first_rows_ms', 'lt', 16),
    @('10k full listing', '< 50 ms', $r10, 'full_listing_ms', 'lt', 50),
    @('10k scroll dropped frames', '0', $r10, 'dropped_frames', 'eq', 0),
    @('100k scroll dropped frames', '0', $r100, 'dropped_frames', 'eq', 0)
)
foreach ($pair in @(@('10k', $r10), @('100k', $r100))) {
    $period = $pair[1]['frame_period_ms']
    $limit = if ($null -ne $period) { $period * 1.5 } else { 0 }
    $rows += , @("Input -> frame max ($($pair[0]))", "<= 1 frame (max <= 1.5 x period $(Format-Value $period) ms)", $pair[1], 'input_to_frame_max_ms', 'le', $limit)
}
$rows += , @('Sort 100k', '< 30 ms', $r100, 'sort_ms', 'lt', 30)

$md = New-Object System.Collections.Generic.List[string]
$md.Add('# Flit metrics')
$md.Add('')
$md.Add("Generated by ``scripts/metrics.ps1`` on $(Get-Date -Format 'yyyy-MM-dd HH:mm'). Each scenario ran $runs times; values are medians.")
$md.Add('')
$md.Add('## Hardware')
$md.Add('')
$md.Add('| Component | Value |')
$md.Add('|---|---|')
$md.Add("| CPU | $($cpu.Name.Trim()) |")
$md.Add("| Cores / threads | $($cpu.NumberOfCores) / $($cpu.NumberOfLogicalProcessors) |")
$md.Add("| RAM | $(Format-Value ($cs.TotalPhysicalMemory / 1GB)) GB |")
$md.Add("| GPU | $gpus |")
$md.Add("| OS | $($os.Caption) $($os.Version)$(if ($ubr) { ".$ubr" }) (build $($os.BuildNumber)) |")
$md.Add("| System disk | $(Get-DiskInfo $env:SystemDrive) |")
$md.Add("| Test data disk | $(Get-DiskInfo $testdata) |")
$md.Add('')
$md.Add('## Results vs targets')
$md.Add('')
$md.Add('| Metric | Target | Measured (ms or count) | Result |')
$md.Add('|---|---|---|---|')
foreach ($row in $rows) {
    $t = Test-Target $row[2] $row[3] $row[4] $row[5]
    $md.Add("| $($row[0]) | $($row[1]) | $($t[0]) | $($t[1]) |")
}
$md.Add('')
$md.Add('## Raw medians')
$md.Add('')
$md.Add('| Key | ' + ($folders -join ' | ') + ' |')
$md.Add('|---|' + (($folders | ForEach-Object { '---' }) -join '|') + '|')
foreach ($k in @('runs') + $keys) {
    $md.Add("| $k | " + (($folders | ForEach-Object { Format-Value $results[$_][$k] }) -join ' | ') + ' |')
}
if ($benchLines.Count -gt 0) {
    $md.Add('')
    $md.Add('## flit-core benchmarks (criterion, low / estimate / high)')
    $md.Add('')
    $md.Add('| Benchmark | Time |')
    $md.Add('|---|---|')
    foreach ($b in $benchLines) { $md.Add($b) }
}

New-Item -ItemType Directory -Force (Split-Path -Parent $report) | Out-Null
[System.IO.File]::WriteAllLines($report, $md, (New-Object System.Text.UTF8Encoding $false))
Write-Host "wrote $report"
Write-Host ($md -join "`n")
