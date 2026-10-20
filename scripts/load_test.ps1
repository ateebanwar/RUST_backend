<#
.SYNOPSIS
    Controlled Local Load & Performance Testing Script for ai-backend.

.DESCRIPTION
    Executes controlled, graduated concurrency benchmarks against a local ai-backend instance.
    Runs concurrency stages (1, 5, 10, 20) with bounded requests per stage.
    Uses non-blocking .NET HttpClient async tasks managed on the PowerShell event loop
    with strict timeout guards and resource disposal.
    Measures latency distributions (min, avg, median, p95, p99), RPS throughput,
    streaming Time-to-First-Chunk (TTFC), error rates, and server process RAM/CPU.

.PARAMETER BaseUrl
    Base URL of the running ai-backend daemon (default: http://127.0.0.1:8080)

.PARAMETER ApiKey
    Backend authentication key (default: test_backend_api_key_secure)

.PARAMETER ConcurrencyTiers
    Array of concurrency levels to test (default: 1, 5, 10, 20)

.PARAMETER IncludeInference
    Include authenticated chat completions and SSE streaming tests (default: $true)

.EXAMPLE
    .\scripts\load_test.ps1 -BaseUrl "http://127.0.0.1:8090" -ApiKey "test_backend_api_key_secure"
#>

param (
    [string]$BaseUrl = "http://127.0.0.1:8080",
    [string]$ApiKey = "test_backend_api_key_secure",
    [int[]]$ConcurrencyTiers = @(1, 5, 10, 20),
    [switch]$IncludeInference = $true
)

$ErrorActionPreference = "Stop"

# Load .NET HTTP assembly required by Windows PowerShell 5.1
Add-Type -AssemblyName System.Net.Http -ErrorAction SilentlyContinue

# Elevate .NET Framework connection limit for concurrent benchmark workers
try {
    [System.Net.ServicePointManager]::DefaultConnectionLimit = 128
    [System.Net.ServicePointManager]::Expect100Continue = $false
} catch { }

Write-Host "================================================================================" -ForegroundColor Cyan
Write-Host " ai-backend Controlled Local Load & Performance Benchmark" -ForegroundColor Cyan
Write-Host " Target Daemon: $BaseUrl" -ForegroundColor Cyan
Write-Host " Concurrency Tiers: $($ConcurrencyTiers -join ', ')" -ForegroundColor Cyan
Write-Host "================================================================================" -ForegroundColor Cyan

# Check server reachability
try {
    $health = Invoke-RestMethod -Uri "$BaseUrl/api/v1/health" -Method Get -TimeoutSec 3
    Write-Host "[OK] Server is reachable at $BaseUrl. Status: $($health.status)" -ForegroundColor Green
} catch {
    Write-Host "[ERROR] Could not connect to $BaseUrl/api/v1/health: $_" -ForegroundColor Red
    exit 1
}

$endpoints = [System.Collections.Generic.List[hashtable]]::new()

$endpoints.Add(@{
    Name = "GET /api/v1/health (Liveness)"
    Path = "/api/v1/health"
    Method = "GET"
    Body = $null
    Streaming = $false
})

$endpoints.Add(@{
    Name = "POST /api/v1/routing/preview (Classifier & Routing Engine)"
    Path = "/api/v1/routing/preview"
    Method = "POST"
    Body = '{"prompt":"Write a Rust async function to parse ELF headers.","requested_modality":"code","routing_mode":"auto"}'
    Streaming = $false
})

if ($IncludeInference) {
    $endpoints.Add(@{
        Name = "POST /api/v1/chat/completions (Full Buffered Pipeline)"
        Path = "/api/v1/chat/completions"
        Method = "POST"
        Body = '{"messages":[{"role":"user","content":"Hello, how are you today?"}],"routing_mode":"auto"}'
        Streaming = $false
    })

    $endpoints.Add(@{
        Name = "POST /api/v1/chat/stream (SSE Streaming Pipeline)"
        Path = "/api/v1/chat/stream"
        Method = "POST"
        Body = '{"messages":[{"role":"user","content":"Stream advantages of Rust."}],"routing_mode":"auto","stream":true}'
        Streaming = $true
    })
}

function Start-HttpRequestTask {
    param (
        [System.Net.Http.HttpClient]$Client,
        [string]$Method,
        [System.Uri]$Uri,
        [string]$Body,
        [bool]$Streaming
    )

    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    $task = if ($Method -eq "GET") {
        $Client.GetAsync($Uri)
    } elseif ($Streaming) {
        $reqMsg = New-Object System.Net.Http.HttpRequestMessage([System.Net.Http.HttpMethod]::Post, $Uri)
        $reqMsg.Content = New-Object System.Net.Http.StringContent($Body, [System.Text.Encoding]::UTF8, "application/json")
        $reqMsg.Headers.Accept.Add([System.Net.Http.Headers.MediaTypeWithQualityHeaderValue]::new("text/event-stream"))
        $Client.SendAsync($reqMsg, [System.Net.Http.HttpCompletionOption]::ResponseHeadersRead)
    } else {
        $content = New-Object System.Net.Http.StringContent($Body, [System.Text.Encoding]::UTF8, "application/json")
        $Client.PostAsync($Uri, $content)
    }

    [PSCustomObject]@{
        Task = $task
        Stopwatch = $sw
        IsStreaming = $Streaming
    }
}

function Run-LoadStage {
    param (
        [string]$Name,
        [string]$Path,
        [string]$Method,
        [string]$Body,
        [bool]$Streaming,
        [int]$Concurrency,
        [int]$TotalRequests
    )

    $client = New-Object System.Net.Http.HttpClient
    $client.Timeout = [System.TimeSpan]::FromSeconds(5)
    if ($ApiKey) {
        $client.DefaultRequestHeaders.Add("x-api-key", $ApiKey)
    }

    $uri = [System.Uri]"$BaseUrl$Path"
    $latencies = [System.Collections.Generic.List[double]]::new()
    $ttfcLatencies = [System.Collections.Generic.List[double]]::new()
    $success = 0
    $failed = 0

    # Sample server process stats before stage
    $procBefore = Get-Process -Name "ai-backend" -ErrorAction SilentlyContinue | Select-Object -First 1
    $cpuBefore = if ($procBefore) { $procBefore.TotalProcessorTime.TotalSeconds } else { $null }

    $swTotal = [System.Diagnostics.Stopwatch]::StartNew()
    $inFlight = [System.Collections.Generic.List[PSCustomObject]]::new()

    $processItem = {
        param ($item)
        $sw = $item.Stopwatch
        $task = $item.Task
        $isStream = $item.IsStreaming

        if ($task.IsCompleted -and -not $task.IsFaulted -and -not $task.IsCanceled) {
            try {
                $resp = $task.Result
                if ($resp.IsSuccessStatusCode) {
                    if ($isStream) {
                        # Measure TTFC from request start to headers read
                        $ttfcMs = $sw.Elapsed.TotalMilliseconds
                        $ttfcLatencies.Add($ttfcMs)

                        # Read stream bytes to completion
                        $stream = $resp.Content.ReadAsStreamAsync().Result
                        $buffer = New-Object byte[] 4096
                        while (($bytesRead = $stream.Read($buffer, 0, $buffer.Length)) -gt 0) { }
                        $sw.Stop()
                        $stream.Dispose()
                    } else {
                        $null = $resp.Content.ReadAsStringAsync().Result
                        $sw.Stop()
                    }
                    $latencies.Add($sw.Elapsed.TotalMilliseconds)
                    $script:stageSuccess++
                } else {
                    $sw.Stop()
                    $script:stageFailed++
                }
                $resp.Dispose()
            } catch {
                $sw.Stop()
                $script:stageFailed++
            }
        } else {
            $sw.Stop()
            $script:stageFailed++
        }
    }

    $script:stageSuccess = 0
    $script:stageFailed = 0

    for ($i = 0; $i -lt $TotalRequests; $i++) {
        # If in-flight count reached concurrency, wait for at least one to complete
        while ($inFlight.Count -ge $Concurrency) {
            $taskArr = [System.Threading.Tasks.Task[]]($inFlight | ForEach-Object { $_.Task })
            $idx = [System.Threading.Tasks.Task]::WaitAny($taskArr, 5000)
            if ($idx -ge 0) {
                $item = $inFlight[$idx]
                $inFlight.RemoveAt($idx)
                & $processItem $item
            } else {
                # Task timed out
                $item = $inFlight[0]
                $inFlight.RemoveAt(0)
                & $processItem $item
            }
        }

        # Launch next request
        $item = Start-HttpRequestTask -Client $client -Method $Method -Uri $uri -Body $Body -Streaming $Streaming
        $inFlight.Add($item)
    }

    # Drain remaining in-flight tasks
    while ($inFlight.Count -gt 0) {
        $taskArr = [System.Threading.Tasks.Task[]]($inFlight | ForEach-Object { $_.Task })
        $idx = [System.Threading.Tasks.Task]::WaitAny($taskArr, 5000)
        if ($idx -ge 0) {
            $item = $inFlight[$idx]
            $inFlight.RemoveAt($idx)
            & $processItem $item
        } else {
            $item = $inFlight[0]
            $inFlight.RemoveAt(0)
            & $processItem $item
        }
    }

    $swTotal.Stop()
    $client.Dispose()

    # Sample server process stats after stage
    $procAfter = Get-Process -Name "ai-backend" -ErrorAction SilentlyContinue | Select-Object -First 1
    $ramAfterMb = if ($procAfter) { [Math]::Round($procAfter.WorkingSet64 / 1MB, 1) } else { 0 }
    $cpuDelta = if ($procAfter -and $cpuBefore -ne $null) { [Math]::Round($procAfter.TotalProcessorTime.TotalSeconds - $cpuBefore, 2) } else { 0 }

    $latArray = $latencies.ToArray()
    [System.Array]::Sort($latArray)

    $cnt = $latArray.Length
    $avg = if ($cnt -gt 0) { ($latArray | Measure-Object -Average).Average } else { 0 }
    $med = if ($cnt -gt 0) { $latArray[[int]($cnt / 2)] } else { 0 }
    $p95 = if ($cnt -gt 0) { $latArray[[Math]::Min($cnt - 1, [int]($cnt * 0.95))] } else { 0 }
    $p99 = if ($cnt -gt 0) { $latArray[[Math]::Min($cnt - 1, [int]($cnt * 0.99))] } else { 0 }
    $rps = if ($swTotal.Elapsed.TotalSeconds -gt 0) { $script:stageSuccess / $swTotal.Elapsed.TotalSeconds } else { 0 }

    $ttfcArray = $ttfcLatencies.ToArray()
    $ttfcAvg = if ($ttfcArray.Length -gt 0) { [Math]::Round(($ttfcArray | Measure-Object -Average).Average, 2) } else { $null }

    [PSCustomObject]@{
        Endpoint = $Name
        Concurrency = $Concurrency
        Requests = $TotalRequests
        Success = $script:stageSuccess
        Failed = $script:stageFailed
        RPS = [Math]::Round($rps, 1)
        AvgMs = [Math]::Round($avg, 2)
        MedMs = [Math]::Round($med, 2)
        P95Ms = [Math]::Round($p95, 2)
        P99Ms = [Math]::Round($p99, 2)
        TTFCMs = $ttfcAvg
        ServerRamMB = $ramAfterMb
        ServerCpuSec = $cpuDelta
    }
}

$results = [System.Collections.Generic.List[PSCustomObject]]::new()

foreach ($ep in $endpoints) {
    Write-Host "`n--> Benchmarking: $($ep.Name)..." -ForegroundColor Yellow
    foreach ($conc in $ConcurrencyTiers) {
        $n = switch ($conc) {
            1 { 20 }
            5 { 50 }
            10 { 100 }
            20 { 100 }
            default { 50 }
        }
        $r = Run-LoadStage -Name $ep.Name -Path $ep.Path -Method $ep.Method -Body $ep.Body -Streaming $ep.Streaming -Concurrency $conc -TotalRequests $n
        $results.Add($r)
        $ttfcDisplay = if ($r.TTFCMs -ne $null) { " | TTFC: {0,6:F2}ms" -f $r.TTFCMs } else { "" }
        Write-Host ("    [C={0:D2}, N={1:D3}] -> RPS: {2,7:F1} | Avg: {3,6:F2}ms | Med: {4,6:F2}ms | P95: {5,6:F2}ms{6} | RAM: {7}MB | Fail: {8}" -f $r.Concurrency, $r.Requests, $r.RPS, $r.AvgMs, $r.MedMs, $r.P95Ms, $ttfcDisplay, $r.ServerRamMB, $r.Failed)
    }
}

Write-Host "`n================================================================================" -ForegroundColor Cyan
Write-Host " BENCHMARK SUMMARY TABLE" -ForegroundColor Cyan
Write-Host "================================================================================" -ForegroundColor Cyan
$results | Format-Table -Property Endpoint, Concurrency, Requests, Success, Failed, RPS, AvgMs, MedMs, P95Ms, P99Ms, TTFCMs, ServerRamMB

Write-Host "`n### Markdown Summary (for Documentation)" -ForegroundColor Cyan
Write-Host "| Endpoint | Concurrency (C) | Requests (N) | Success | Fail | RPS | Avg Latency | Med Latency | P95 Latency | P99 Latency | TTFC | Server RAM |"
Write-Host "| :--- | :---: | :---: | :---: | :---: | :---: | :---: | :---: | :---: | :---: | :---: | :---: |"
foreach ($r in $results) {
    $ttfcStr = if ($r.TTFCMs -ne $null) { "$($r.TTFCMs) ms" } else { "-" }
    Write-Host ("| `{0}` | {1} | {2} | {3} | {4} | {5} | {6} ms | {7} ms | {8} ms | {9} ms | {10} | {11} MB |" -f $r.Endpoint, $r.Concurrency, $r.Requests, $r.Success, $r.Failed, $r.RPS, $r.AvgMs, $r.MedMs, $r.P95Ms, $r.P99Ms, $ttfcStr, $r.ServerRamMB)
}

