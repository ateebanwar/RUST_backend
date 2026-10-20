<#
.SYNOPSIS
    Bounded Real-Groq HTTP Performance Verification Script for ai-backend.

.DESCRIPTION
    Executes a strictly bounded, low-concurrency verification against the running
    ai-backend daemon connected to the real Groq API.
    Enforces low request counts (N=5) and small token budgets (max_tokens=30)
    to verify live cloud provider latency, streaming TTFC, and error handling
    without rate-limit or quota impact.
#>

param (
    [string]$BaseUrl = "http://127.0.0.1:8080",
    [string]$ApiKey = "test_backend_api_key_secure",
    [int]$Concurrency = 1,
    [int]$RequestsPerEndpoint = 5
)

$ErrorActionPreference = "Stop"
Add-Type -AssemblyName System.Net.Http -ErrorAction SilentlyContinue

Write-Host "================================================================================" -ForegroundColor Cyan
Write-Host " ai-backend Real Groq Cloud Live Performance Verification" -ForegroundColor Cyan
Write-Host " Target: $BaseUrl | Concurrency: $Concurrency | Requests/Endpoint: $RequestsPerEndpoint" -ForegroundColor Cyan
Write-Host " Upstream: Groq Cloud API (Live)" -ForegroundColor Cyan
Write-Host "================================================================================" -ForegroundColor Cyan

# Health check
try {
    $health = Invoke-RestMethod -Uri "$BaseUrl/api/v1/health" -Method Get -TimeoutSec 5
    Write-Host "[OK] Backend reachable. Status: $($health.status)" -ForegroundColor Green
} catch {
    Write-Host "[ERROR] Could not connect to $BaseUrl/api/v1/health: $_" -ForegroundColor Red
    exit 1
}

$endpoints = @(
    @{
        Name = "GET /api/v1/health (Internal)"
        Path = "/api/v1/health"
        Method = "GET"
        Body = $null
        Streaming = $false
    },
    @{
        Name = "POST /api/v1/routing/preview (Internal)"
        Path = "/api/v1/routing/preview"
        Method = "POST"
        Body = '{"prompt":"Write a Rust async function to parse ELF headers.","requested_modality":"code","routing_mode":"auto"}'
        Streaming = $false
    },
    @{
        Name = "POST /api/v1/chat/completions (Live Groq)"
        Path = "/api/v1/chat/completions"
        Method = "POST"
        Body = '{"messages":[{"role":"user","content":"Say Hello in one word."}],"routing_mode":"manual","requested_model_id":"fast-chat-v1","max_tokens":25}'
        Streaming = $false
    },
    @{
        Name = "POST /api/v1/chat/stream (Live Groq SSE)"
        Path = "/api/v1/chat/stream"
        Method = "POST"
        Body = '{"messages":[{"role":"user","content":"Say Streaming in one word."}],"routing_mode":"manual","requested_model_id":"fast-chat-v1","max_tokens":25,"stream":true}'
        Streaming = $true
    }
)

$results = [System.Collections.Generic.List[PSCustomObject]]::new()

foreach ($ep in $endpoints) {
    Write-Host "`n--> Testing: $($ep.Name)..." -ForegroundColor Yellow

    $client = New-Object System.Net.Http.HttpClient
    $client.Timeout = [TimeSpan]::FromSeconds(30)
    if ($ApiKey) {
        $client.DefaultRequestHeaders.Add("x-api-key", $ApiKey)
    }

    $uri = [System.Uri]"$BaseUrl$($ep.Path)"
    $latencies = [System.Collections.Generic.List[double]]::new()
    $headersLatencies = [System.Collections.Generic.List[double]]::new()
    $ttfcLatencies = [System.Collections.Generic.List[double]]::new()
    $success = 0
    $failed = 0

    $swStage = [System.Diagnostics.Stopwatch]::StartNew()

    for ($i = 0; $i -lt $RequestsPerEndpoint; $i++) {
        $swReq = [System.Diagnostics.Stopwatch]::StartNew()
        try {
            if ($ep.Method -eq "GET") {
                $resp = $client.GetAsync($uri).Result
                $swReq.Stop()
                if ($resp.IsSuccessStatusCode) {
                    $null = $resp.Content.ReadAsStringAsync().Result
                    $success++
                    $latencies.Add($swReq.Elapsed.TotalMilliseconds)
                } else {
                    $failed++
                }
                $resp.Dispose()
            } elseif ($ep.Streaming) {
                $reqMsg = New-Object System.Net.Http.HttpRequestMessage([System.Net.Http.HttpMethod]::Post, $uri)
                $reqMsg.Content = New-Object System.Net.Http.StringContent($ep.Body, [System.Text.Encoding]::UTF8, "application/json")
                $reqMsg.Headers.Accept.Add([System.Net.Http.Headers.MediaTypeWithQualityHeaderValue]::new("text/event-stream"))

                $resp = $client.SendAsync($reqMsg, [System.Net.Http.HttpCompletionOption]::ResponseHeadersRead).Result
                $timeToHeaders = $swReq.Elapsed.TotalMilliseconds
                if ($resp.IsSuccessStatusCode) {
                    $stream = $resp.Content.ReadAsStreamAsync().Result
                    $reader = New-Object System.IO.StreamReader($stream, [System.Text.Encoding]::UTF8)
                    $firstTokenTTFC = $null

                    while (($line = $reader.ReadLine()) -ne $null) {
                        if ($line.StartsWith("data:")) {
                            $payload = $line.Substring(5).Trim()
                            if ($payload -ne "[DONE]" -and $payload.Length -gt 0) {
                                try {
                                    $json = $payload | ConvertFrom-Json
                                    if ($json.delta -and $json.delta.Length -gt 0 -and $firstTokenTTFC -eq $null) {
                                        $firstTokenTTFC = $swReq.Elapsed.TotalMilliseconds
                                    }
                                } catch {}
                            }
                        }
                    }
                    $swReq.Stop()
                    $totalDuration = $swReq.Elapsed.TotalMilliseconds
                    $reader.Dispose()
                    $stream.Dispose()
                    $success++
                    $latencies.Add($totalDuration)
                    $headersLatencies.Add($timeToHeaders)
                    if ($firstTokenTTFC -ne $null) {
                        $ttfcLatencies.Add($firstTokenTTFC)
                    }
                } else {
                    $swReq.Stop()
                    $failed++
                }
                $resp.Dispose()
                $reqMsg.Dispose()
            } else {
                $content = New-Object System.Net.Http.StringContent($ep.Body, [System.Text.Encoding]::UTF8, "application/json")
                $resp = $client.PostAsync($uri, $content).Result
                $swReq.Stop()
                if ($resp.IsSuccessStatusCode) {
                    $null = $resp.Content.ReadAsStringAsync().Result
                    $success++
                    $latencies.Add($swReq.Elapsed.TotalMilliseconds)
                } else {
                    $failed++
                }
                $resp.Dispose()
            }
        } catch {
            $swReq.Stop()
            $failed++
        }
    }

    $swStage.Stop()
    $client.Dispose()

    $latArray = $latencies.ToArray()
    [System.Array]::Sort($latArray)

    $cnt = $latArray.Length
    $avg = if ($cnt -gt 0) { ($latArray | Measure-Object -Average).Average } else { 0 }
    $med = if ($cnt -gt 0) { $latArray[[int]($cnt / 2)] } else { 0 }
    $p95 = if ($cnt -gt 0) { $latArray[[Math]::Min($cnt - 1, [int]($cnt * 0.95))] } else { 0 }
    $p99 = if ($cnt -gt 0) { $latArray[[Math]::Min($cnt - 1, [int]($cnt * 0.99))] } else { 0 }
    $rps = if ($swStage.Elapsed.TotalSeconds -gt 0) { $success / $swStage.Elapsed.TotalSeconds } else { 0 }

    $ttfcArray = $ttfcLatencies.ToArray()
    $ttfcAvg = if ($ttfcArray.Length -gt 0) { [Math]::Round(($ttfcArray | Measure-Object -Average).Average, 2) } else { $null }

    $headersArray = $headersLatencies.ToArray()
    $headersAvg = if ($headersArray.Length -gt 0) { [Math]::Round(($headersArray | Measure-Object -Average).Average, 2) } else { $null }

    $r = [PSCustomObject]@{
        Endpoint = $ep.Name
        Concurrency = $Concurrency
        Requests = $RequestsPerEndpoint
        Success = $success
        Failed = $failed
        RPS = [Math]::Round($rps, 2)
        AvgMs = [Math]::Round($avg, 2)
        MedMs = [Math]::Round($med, 2)
        P95Ms = [Math]::Round($p95, 2)
        P99Ms = [Math]::Round($p99, 2)
        TimeToHeadersMs = $headersAvg
        ClientTTFCMs = $ttfcAvg
    }
    $results.Add($r)

    $ttfcDisplay = if ($r.ClientTTFCMs -ne $null) {
        " | Headers: {0,6:F2}ms | ClientTTFC: {1,6:F2}ms" -f $r.TimeToHeadersMs, $r.ClientTTFCMs
    } else { "" }
    Write-Host ("    [C={0:D2}, N={1:D3}] -> RPS: {2,5:F2} | Avg: {3,6:F2}ms | Med: {4,6:F2}ms | P95: {5,6:F2}ms{6} | Fail: {7}" -f $r.Concurrency, $r.Requests, $r.RPS, $r.AvgMs, $r.MedMs, $r.P95Ms, $ttfcDisplay, $r.Failed)
}

Write-Host "`n================================================================================" -ForegroundColor Cyan
Write-Host " REAL GROQ PERFORMANCE SUMMARY" -ForegroundColor Cyan
Write-Host "================================================================================" -ForegroundColor Cyan
$results | Format-Table -Property Endpoint, Concurrency, Requests, Success, Failed, RPS, AvgMs, MedMs, P95Ms, TimeToHeadersMs, ClientTTFCMs

Write-Host "`n### Markdown Table for Documentation" -ForegroundColor Cyan
Write-Host "| Endpoint | Concurrency ($C$) | Requests ($N$) | Success | Fail | Throughput (RPS) | Avg Latency | Median (p50) | p95 Latency | Time To Headers | Client TTFC |"
Write-Host "| :--- | :---: | :---: | :---: | :---: | :---: | :---: | :---: | :---: | :---: | :---: |"
foreach ($r in $results) {
    $hdrStr = if ($r.TimeToHeadersMs -ne $null) { "$($r.TimeToHeadersMs) ms" } else { "-" }
    $ttfcStr = if ($r.ClientTTFCMs -ne $null) { "$($r.ClientTTFCMs) ms" } else { "-" }
    Write-Host ("| `{0}` | {1} | {2} | {3} | {4} | **{5}** | {6} ms | {7} ms | {8} ms | {9} | {10} |" -f $r.Endpoint, $r.Concurrency, $r.Requests, $r.Success, $r.Failed, $r.RPS, $r.AvgMs, $r.MedMs, $r.P95Ms, $hdrStr, $ttfcStr)
}
