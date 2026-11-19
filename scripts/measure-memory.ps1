param([Parameter(Mandatory=$true)][int]$AppProcessId, [int]$Seconds=30)
$ErrorActionPreference='Stop'
$samples = [System.Collections.Generic.List[object]]::new()
$timer = [System.Diagnostics.Stopwatch]::StartNew()
while ($timer.Elapsed.TotalSeconds -lt $Seconds) {
    $tree = @($AppProcessId)
    $processes = Get-CimInstance Win32_Process -Property ProcessId,ParentProcessId,Name
    if (-not ($processes | Where-Object ProcessId -eq $AppProcessId)) { break }
    do {
        $next = @($processes | Where-Object { $_.ParentProcessId -in $tree -and $_.ProcessId -notin $tree } | ForEach-Object { [int]$_.ProcessId })
        $tree += $next
    } while ($next.Count -gt 0)
    $counters = @(Get-CimInstance Win32_PerfFormattedData_PerfProc_Process | Where-Object { $_.IDProcess -in $tree })
    $samples.Add([pscustomobject]@{
        elapsedSeconds = [math]::Round($timer.Elapsed.TotalSeconds,2)
        processes = $counters.Count
        privateWorkingSetMiB = [math]::Round(($counters | Measure-Object WorkingSetPrivate -Sum).Sum / 1MB,2)
        privateCommitMiB = [math]::Round(($counters | Measure-Object PrivateBytes -Sum).Sum / 1MB,2)
    })
    Start-Sleep -Milliseconds 500
}
[pscustomobject]@{
    peakObservedPrivateWorkingSetMiB = ($samples | Measure-Object privateWorkingSetMiB -Maximum).Maximum
    peakObservedPrivateCommitMiB = ($samples | Measure-Object privateCommitMiB -Maximum).Maximum
    samples = $samples
} | ConvertTo-Json -Depth 4
