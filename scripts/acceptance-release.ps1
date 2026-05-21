param(
    [string]$OutputRoot = ".release-evidence",
    [string]$StressWorkspace = "C:\Users\Yanzh\Desktop\cli-stress-test",
    [switch]$RunBaseGates,
    [switch]$IncludeWorkspaceTests,
    [switch]$IncludeDesktopBundle,
    [switch]$IncludeProviderMatrix,
    [switch]$IncludeMcpMatrix,
    [switch]$IncludeRemoteE2E,
    [switch]$IncludeMobilePwaE2E,
    [switch]$IncludeTransportE2E,
    [switch]$IncludeTailscaleE2E,
    [switch]$UseProxy,
    [string]$RelayHostAuditReport,
    [string]$TailscaleEvidenceReport,
    [switch]$RequireComplete,
    [string]$ProxyUrl = "http://127.0.0.1:7890"
)

$ErrorActionPreference = "Stop"
$RepoRoot = Resolve-Path (Join-Path $PSScriptRoot "..")
$RepoRootPath = $RepoRoot.Path
Set-Location $RepoRootPath

if ($UseProxy) {
    $env:HTTP_PROXY = $ProxyUrl
    $env:HTTPS_PROXY = $ProxyUrl
    $env:ALL_PROXY = $ProxyUrl
    $env:http_proxy = $ProxyUrl
    $env:https_proxy = $ProxyUrl
    $env:all_proxy = $ProxyUrl
    $env:CARGO_HTTP_PROXY = $ProxyUrl
    $loopbackNoProxy = "localhost,127.0.0.1,::1"
    $env:NO_PROXY = if ([string]::IsNullOrWhiteSpace($env:NO_PROXY)) { $loopbackNoProxy } else { "$($env:NO_PROXY),$loopbackNoProxy" }
    $env:no_proxy = if ([string]::IsNullOrWhiteSpace($env:no_proxy)) { $loopbackNoProxy } else { "$($env:no_proxy),$loopbackNoProxy" }
    if ([string]::IsNullOrWhiteSpace($env:GIT_CONFIG_COUNT)) {
        $env:GIT_CONFIG_COUNT = "2"
        $env:GIT_CONFIG_KEY_0 = "http.proxy"
        $env:GIT_CONFIG_VALUE_0 = $ProxyUrl
        $env:GIT_CONFIG_KEY_1 = "https.proxy"
        $env:GIT_CONFIG_VALUE_1 = $ProxyUrl
    }
}

$Stamp = Get-Date -Format "yyyyMMdd-HHmmss"
$EvidenceRoot = Join-Path $RepoRootPath (Join-Path $OutputRoot $Stamp)
$LogRoot = Join-Path $EvidenceRoot "logs"
New-Item -ItemType Directory -Force -Path $LogRoot | Out-Null
New-Item -ItemType Directory -Force -Path $StressWorkspace | Out-Null

$ReportPath = Join-Path $EvidenceRoot "release-acceptance.md"
$Results = New-Object System.Collections.Generic.List[object]

function Add-Result(
    [string]$Area,
    [string]$Item,
    [string]$Status,
    [string]$Evidence,
    [string]$Notes
) {
    $Results.Add([pscustomobject]@{
        Area = $Area
        Item = $Item
        Status = $Status
        Evidence = $Evidence
        Notes = $Notes
    }) | Out-Null
}

function Add-UnrequestedSkip(
    [string]$Area,
    [string]$Item,
    [string]$Notes
) {
    if (-not $RequireComplete) {
        Add-Result $Area $Item "SKIP" "" $Notes
    }
}

function Sanitize-Text([string]$Text) {
    if ($null -eq $Text) {
        return ""
    }
    $patterns = @(
        "sk-[A-Za-z0-9_\-]{12,}",
        "sk-cp-[A-Za-z0-9_\-]{12,}",
        "kV[A-Za-z0-9_\-]{20,}",
        "Bearer\s+[A-Za-z0-9_\-\.]{12,}",
        "x-api-key:\s*\S+"
    )
    $redacted = $Text
    foreach ($pattern in $patterns) {
        $redacted = [regex]::Replace($redacted, $pattern, "<redacted>", "IgnoreCase")
    }
    return $redacted
}

function Run-LoggedStep([string]$Area, [string]$Item, [scriptblock]$Command) {
    $safeName = ($Area + "-" + $Item).ToLowerInvariant() -replace "[^a-z0-9]+", "-"
    $logPath = Join-Path $LogRoot "$safeName.log"
    Write-Host ""
    Write-Host "=== $Area / $Item ==="
    try {
        $global:LASTEXITCODE = 0
        $previousErrorAction = $ErrorActionPreference
        $ErrorActionPreference = "Continue"
        try {
            $output = & $Command 2>&1 | Out-String
            $exitCode = $global:LASTEXITCODE
        }
        finally {
            $ErrorActionPreference = $previousErrorAction
        }
        if ($exitCode -ne 0) {
            throw "command exited with code $exitCode`n$output"
        }
        $sanitized = Sanitize-Text $output
        Set-Content -Path $logPath -Value $sanitized -Encoding UTF8
        Add-Result $Area $Item "PASS" $logPath ""
    }
    catch {
        $message = Sanitize-Text ($_.Exception.Message)
        Set-Content -Path $logPath -Value $message -Encoding UTF8
        Add-Result $Area $Item "FAIL" $logPath $message
    }
}

function Env-Present([string[]]$Names) {
    foreach ($name in $Names) {
        if (-not [string]::IsNullOrWhiteSpace([Environment]::GetEnvironmentVariable($name))) {
            return $true
        }
    }
    return $false
}

function Assert-TailscaleEvidenceReport([string]$Content) {
    if ([string]::IsNullOrWhiteSpace($Content)) {
        throw "Tailscale evidence report is empty"
    }
    if ($Content -match "(?im)^\s*(FAIL|SKIP|MANUAL)\s*:") {
        throw "Tailscale evidence report contains blocking evidence markers"
    }

    $requiredMarkers = @(
        "tailnet-direct",
        "manual-strategy",
        "disabled-path",
        "failure-fallback",
        "e2ee",
        "approval",
        "artifact",
        "acl-device-trust"
    )
    foreach ($marker in $requiredMarkers) {
        $pattern = "(?im)^\s*PASS:\s*" + [regex]::Escape($marker) + "\b"
        if ($Content -notmatch $pattern) {
            throw "Tailscale evidence report missing required marker: PASS: $marker"
        }
    }
}

function RemoteCodeExe {
    $release = Join-Path $RepoRootPath "target\release\remote-code.exe"
    if (Test-Path $release) {
        return $release
    }
    return Join-Path $RepoRootPath "target\debug\remote-code.exe"
}

function Ensure-RemoteCodeExe {
    $exe = RemoteCodeExe
    if (-not (Test-Path $exe)) {
        cargo build -p remote-code --bin remote-code -j 1
    }
    return (RemoteCodeExe)
}

function Invoke-ProviderSmoke(
    [string]$Provider,
    [string]$BaseUrl,
    [string]$Model,
    [string]$Protocol,
    [string[]]$RequiredEnv
) {
    $exe = Ensure-RemoteCodeExe
    $profile = Join-Path $EvidenceRoot ("profile-" + $Provider)
    New-Item -ItemType Directory -Force -Path $profile | Out-Null
    & $exe --print --cwd $StressWorkspace --profile-dir $profile --provider $Provider --base-url $BaseUrl --model $Model --protocol $Protocol --permission-mode bypassPermissions --max-turns 3 "Reply exactly: RC_PROVIDER_SMOKE_OK"
}

function Invoke-JsonCommand([scriptblock]$Command) {
    $global:LASTEXITCODE = 0
    $jsonText = & $Command 2>&1 | Out-String
    $exitCode = $global:LASTEXITCODE
    return [pscustomobject]@{
        Text = $jsonText
        ExitCode = $exitCode
    }
}

function Write-McpAcceptanceConfig {
    $configPath = Join-Path $EvidenceRoot "mcp.acceptance.json"
    $servers = [ordered]@{
        context7 = @{
            command = "npx"
            args = @("-y", "@upstash/context7-mcp")
            env = @{ DEFAULT_MINIMUM_TOKENS = "" }
        }
        sequentialthinking = @{
            command = "npx"
            args = @("-y", "@modelcontextprotocol/server-sequential-thinking")
        }
        memory = @{
            command = "npx"
            args = @("-y", "@modelcontextprotocol/server-memory")
        }
        puppeteer = @{
            command = "npx"
            args = @("-y", "@modelcontextprotocol/server-puppeteer")
            startup_timeout_secs = 60
            request_timeout_secs = 60
        }
    }
    if (Env-Present @("MINIMAX_API_KEY", "MINIMAX_TOKEN_PLAN_API_KEY")) {
        $minimaxKeyEnv = if (-not [string]::IsNullOrWhiteSpace([Environment]::GetEnvironmentVariable("MINIMAX_API_KEY"))) {
            '${MINIMAX_API_KEY}'
        } else {
            '${MINIMAX_TOKEN_PLAN_API_KEY}'
        }
        $minimaxHostEnv = if (-not [string]::IsNullOrWhiteSpace([Environment]::GetEnvironmentVariable("MINIMAX_API_HOST"))) {
            '${MINIMAX_API_HOST}'
        } else {
            'https://api.minimaxi.com'
        }
        $servers.MiniMax = @{
            command = "uvx"
            args = @("minimax-coding-plan-mcp", "-y")
            env = @{
                MINIMAX_API_KEY = $minimaxKeyEnv
                MINIMAX_API_HOST = $minimaxHostEnv
            }
        }
    }
    $payload = @{ mcpServers = $servers } | ConvertTo-Json -Depth 8
    [System.IO.File]::WriteAllText(
        $configPath,
        $payload,
        [System.Text.UTF8Encoding]::new($false)
    )
    return $configPath
}

function Invoke-McpList([string]$Server, [string]$ConfigPath) {
    $exe = Ensure-RemoteCodeExe
    $result = Invoke-JsonCommand { & $exe mcp list --connect --json --server $Server --config $ConfigPath }
    Write-Output $result.Text
    if ($result.ExitCode -ne 0) {
        $global:LASTEXITCODE = $result.ExitCode
        return
    }
    try {
        $payload = $result.Text | ConvertFrom-Json
    }
    catch {
        Write-Output "command did not emit valid JSON: $($_.Exception.Message)"
        $global:LASTEXITCODE = 1
        return
    }
    foreach ($record in @($payload.servers)) {
        if ($record.live -and $record.live.status -ne "connected") {
            $global:LASTEXITCODE = 1
            $errorDetail = if ($record.live.error) { $record.live.error } else { "unknown MCP connection error" }
            Write-Output "MCP server '$($record.name)' did not connect: $errorDetail"
            return
        }
    }
    $global:LASTEXITCODE = 0
}

function Invoke-McpToolCall([string]$Server, [string]$ConfigPath) {
    $exe = Ensure-RemoteCodeExe
    switch ($Server) {
        "context7" {
            $result = Invoke-JsonCommand { & $exe mcp call --json --server $Server --tool resolve-library-id --arg "libraryName=tokio" --arg "query=Rust async runtime documentation" --config $ConfigPath }
        }
        "sequentialthinking" {
            $result = Invoke-JsonCommand { & $exe mcp call --json --server $Server --tool sequentialthinking --arg "thought=acceptance smoke test" --arg "nextThoughtNeeded=false" --arg "thoughtNumber=1" --arg "totalThoughts=1" --config $ConfigPath }
        }
        "memory" {
            $result = Invoke-JsonCommand { & $exe mcp call --json --server $Server --tool read_graph --config $ConfigPath }
        }
        "puppeteer" {
            $result = Invoke-JsonCommand { & $exe mcp call --json --server $Server --tool puppeteer_navigate --arg "url=about:blank" --config $ConfigPath }
        }
        "MiniMax" {
            $result = Invoke-JsonCommand { & $exe mcp call --json --server $Server --tool web_search --arg "query=Remote Code MCP acceptance" --config $ConfigPath }
        }
        default {
            Write-Output "Unsupported MCP server '$Server'"
            $global:LASTEXITCODE = 1
            return
        }
    }
    Write-Output $result.Text
    if ($result.ExitCode -ne 0) {
        $global:LASTEXITCODE = $result.ExitCode
        return
    }
    try {
        $payload = $result.Text | ConvertFrom-Json
    }
    catch {
        Write-Output "command did not emit valid JSON: $($_.Exception.Message)"
        $global:LASTEXITCODE = 1
        return
    }
    $toolIsError = $false
    if ($payload.response) {
        if ($null -ne $payload.response.is_error) {
            $toolIsError = [bool]$payload.response.is_error
        } elseif ($null -ne $payload.response.isError) {
            $toolIsError = [bool]$payload.response.isError
        } elseif ($payload.response.result -and $null -ne $payload.response.result.isError) {
            $toolIsError = [bool]$payload.response.result.isError
        } elseif ($payload.response.result -and $null -ne $payload.response.result.is_error) {
            $toolIsError = [bool]$payload.response.result.is_error
        }
    }
    if ($toolIsError) {
        $global:LASTEXITCODE = 1
        Write-Output "MCP tool call returned is_error=true for '$Server'"
        return
    }
    $global:LASTEXITCODE = 0
}

if ($RunBaseGates) {
    Run-LoggedStep "14.1" "base-gates" {
        $args = @("-ExecutionPolicy", "Bypass", "-File", (Join-Path $RepoRootPath "scripts\verify-release.ps1"), "-IncludeAudit", "-IncludeGitleaks")
        if ($IncludeWorkspaceTests) { $args += "-IncludeWorkspaceTests" }
        if ($IncludeDesktopBundle) { $args += "-IncludeDesktopBundle" }
        if ($UseProxy) { $args += @("-UseProxy", "-ProxyUrl", $ProxyUrl) }
        powershell @args
    }
} else {
    Add-UnrequestedSkip "14.1" "base-gates" "pass -RunBaseGates to execute local release gates"
}

if ($IncludeProviderMatrix) {
    $providerMatrix = @(
        @{ Name = "minimax-token-plan"; BaseUrl = "https://api.minimaxi.com/anthropic"; Model = "minimax-m2.7"; Protocol = "anthropic"; Env = @("MINIMAX_TOKEN_PLAN_API_KEY", "MINIMAX_API_KEY") },
        @{ Name = "kuaikat-coding"; BaseUrl = "https://wanqing.streamlakeapi.com/api/gateway/coding/kat-coder-pro-v2/claude-code-proxy"; Model = "kat-coder-pro-v2"; Protocol = "anthropic"; Env = @("KUAIKAT_CODING_PLAN_API_KEY", "KUAIKAT_API_KEY") },
        @{ Name = "deepseek-anthropic"; BaseUrl = "https://api.deepseek.com/anthropic"; Model = "deepseek-v4-flash"; Protocol = "anthropic"; Env = @("DEEPSEEK_API_KEY", "DEEPSEEK_CODING_PLAN_API_KEY") }
    )
    foreach ($provider in $providerMatrix) {
        if (-not (Env-Present $provider.Env)) {
            Add-Result "Provider" $provider.Name "SKIP" "" ("missing env: " + ($provider.Env -join " or "))
            continue
        }
        Run-LoggedStep "Provider" $provider.Name {
            Invoke-ProviderSmoke $provider.Name $provider.BaseUrl $provider.Model $provider.Protocol $provider.Env
        }
    }
} else {
    Add-UnrequestedSkip "Provider" "matrix" "pass -IncludeProviderMatrix with provider keys in environment"
}

if ($IncludeMcpMatrix) {
    $mcpConfig = Write-McpAcceptanceConfig
    foreach ($server in @("context7", "sequentialthinking", "memory", "puppeteer", "MiniMax")) {
        if ($server -eq "MiniMax" -and -not (Env-Present @("MINIMAX_API_KEY", "MINIMAX_TOKEN_PLAN_API_KEY"))) {
            Add-Result "MCP" "MiniMax" "SKIP" "" "missing MINIMAX_API_KEY or MINIMAX_TOKEN_PLAN_API_KEY"
            continue
        }
        Run-LoggedStep "MCP" "$server-discover" { Invoke-McpList $server $mcpConfig }
        Run-LoggedStep "MCP" "$server-call" { Invoke-McpToolCall $server $mcpConfig }
    }
} else {
    Add-UnrequestedSkip "MCP" "matrix" "pass -IncludeMcpMatrix to start and call MCP servers"
}

if ($IncludeRemoteE2E) {
    Run-LoggedStep "Remote E2E" "control-plane-runner-local" {
        cargo test -p claude-control-plane --all-targets -j 1 -- --nocapture
        $controlPlaneExit = $global:LASTEXITCODE
        cargo test -p remote-code-runner --all-targets -j 1 -- --nocapture
        $runnerExit = $global:LASTEXITCODE
        if ($controlPlaneExit -ne 0) {
            $global:LASTEXITCODE = $controlPlaneExit
        } elseif ($runnerExit -ne 0) {
            $global:LASTEXITCODE = $runnerExit
        } else {
            $global:LASTEXITCODE = 0
        }
    }
} else {
    Add-UnrequestedSkip "Remote E2E" "relay-runner-control-plane" "pass -IncludeRemoteE2E after provisioning relay and runner"
}

if ($IncludeMobilePwaE2E) {
    Run-LoggedStep "Mobile/PWA" "pairing-prompt-approval-artifact" {
        Push-Location (Join-Path $RepoRootPath "apps\remote-code-gui")
        try {
            npm test -- MobileRemoteApp RemoteApp RemoteAuthGate RemoteShell transport unified-transport connection-manager useConnection
        }
        finally {
            Pop-Location
        }
    }
} else {
    Add-UnrequestedSkip "Mobile/PWA" "pairing-prompt-approval-artifact" "pass -IncludeMobilePwaE2E after preparing real devices"
}

if ($IncludeTransportE2E) {
    Run-LoggedStep "Transport" "relay-direct-outbound-quic" {
        cargo test -p rc-remote-transport --features quic --lib -- --nocapture
        $transportExit = $global:LASTEXITCODE
        cargo test -p claude-control-plane --test quic_transport -- --nocapture
        $quicExit = $global:LASTEXITCODE
        Push-Location (Join-Path $RepoRootPath "apps\remote-code-gui")
        try {
            npm test -- transport unified-transport connection-manager useConnection
            $frontendTransportExit = $global:LASTEXITCODE
        }
        finally {
            Pop-Location
        }
        if ($transportExit -ne 0) {
            $global:LASTEXITCODE = $transportExit
        } elseif ($quicExit -ne 0) {
            $global:LASTEXITCODE = $quicExit
        } elseif ($frontendTransportExit -ne 0) {
            $global:LASTEXITCODE = $frontendTransportExit
        } else {
            $global:LASTEXITCODE = 0
        }
    }
} else {
    Add-UnrequestedSkip "Transport" "relay-direct-outbound-quic" "pass -IncludeTransportE2E after provisioning transport testbed"
}

if ($IncludeTailscaleE2E) {
    $tailscaleEnabled = -not [string]::IsNullOrWhiteSpace([Environment]::GetEnvironmentVariable("REMOTE_CODE_ACCEPTANCE_TAILSCALE_ENABLED"))
    if ($tailscaleEnabled) {
        if (-not [string]::IsNullOrWhiteSpace($TailscaleEvidenceReport)) {
            Run-LoggedStep "Tailscale" "tailnet-direct-fallback" {
                $resolved = Resolve-Path -LiteralPath $TailscaleEvidenceReport
                $content = Get-Content -LiteralPath $resolved.Path -Raw
                Write-Output $content
                Assert-TailscaleEvidenceReport $content
            }
        } else {
            Add-Result "Tailscale" "tailnet-direct-fallback" "MANUAL" "" "Tailscale is enabled; attach two-device tailnet evidence with -TailscaleEvidenceReport"
        }
    } else {
        Add-Result "Tailscale" "tailnet-direct-fallback" "N/A" "" "Tailscale optional path is not enabled for this release candidate; standard relay/direct/outbound/QUIC gates cover the required remote chain"
    }
} else {
    Add-UnrequestedSkip "Tailscale" "tailnet-direct-fallback" "pass -IncludeTailscaleE2E in a prepared tailnet"
}

if (-not [string]::IsNullOrWhiteSpace($RelayHostAuditReport)) {
    Run-LoggedStep "Secure deployment" "relay-host-audit" {
        $resolved = Resolve-Path -LiteralPath $RelayHostAuditReport
        $content = Get-Content -LiteralPath $resolved.Path -Raw
        Write-Output $content
        if ($content -match "(?m)^FAIL:") {
            throw "relay host audit contains failures"
        }
        if ($content -notmatch "Summary:\s+\d+\s+pass,\s+\d+\s+warning,\s+0\s+failure") {
            throw "relay host audit summary missing or not zero-failure"
        }
    }
} else {
    Add-UnrequestedSkip "Secure deployment" "relay-host-audit" "run deploy/tencent-cloud/audit-relay-host.sh on the relay host and pass -RelayHostAuditReport <path>"
}

$lines = New-Object System.Collections.Generic.List[string]
$lines.Add("# Release Acceptance Evidence")
$lines.Add("")
$lines.Add("- Date: $(Get-Date -Format o)")
$lines.Add("- Repository: $RepoRootPath")
$lines.Add("- Stress workspace: $StressWorkspace")
$lines.Add("- Output root: $EvidenceRoot")
$lines.Add("- Secret policy: logs are sanitized; provider keys must be supplied only through environment variables.")
$lines.Add("")
$lines.Add("| Area | Item | Status | Evidence | Notes |")
$lines.Add("| --- | --- | --- | --- | --- |")
foreach ($result in $Results) {
    $evidence = if ([string]::IsNullOrWhiteSpace($result.Evidence)) { "" } else { $result.Evidence.Replace($RepoRootPath, ".") }
    $notes = ($result.Notes -replace "\|", "/").Replace("`r", " ").Replace("`n", " ")
    $lines.Add("| $($result.Area) | $($result.Item) | $($result.Status) | $evidence | $notes |")
}
$lines.Add("")
$lines.Add("## Manual Sign-Off")
$lines.Add("")
$lines.Add("- Release engineer:")
$lines.Add("- Relay host:")
$lines.Add("- Desktop installer hash:")
$lines.Add("- Mobile/PWA device matrix:")
$lines.Add("- QUIC/Tailscale environment:")
$lines.Add("- Provider/MCP matrix owner:")

Set-Content -Path $ReportPath -Value $lines -Encoding UTF8
Write-Host ""
Write-Host "Acceptance evidence written to $ReportPath"

$blockingStatuses = @("FAIL")
if ($RequireComplete) {
    $blockingStatuses += @("SKIP", "MANUAL")
}
if ($RequireComplete -and $Results.Count -eq 0) {
    throw "-RequireComplete requires at least one requested acceptance gate"
}
$blocking = @($Results | Where-Object { $blockingStatuses -contains $_.Status })
if ($blocking.Count -gt 0) {
    $summary = ($blocking | ForEach-Object { "$($_.Area)/$($_.Item)=$($_.Status)" }) -join "; "
    throw "acceptance evidence contains blocking results: $summary"
}
