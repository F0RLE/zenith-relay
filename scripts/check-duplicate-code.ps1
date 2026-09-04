[CmdletBinding()]
param(
  [string]$BaseRef
)

$ErrorActionPreference = "Stop"

if ([string]::IsNullOrWhiteSpace($BaseRef)) {
  if ($env:GITHUB_EVENT_NAME -eq "pull_request" -and $env:GITHUB_BASE_REF) {
    $BaseRef = "origin/$($env:GITHUB_BASE_REF)"
  } elseif ($env:GITHUB_EVENT_NAME -eq "push") {
    $BaseRef = "HEAD^"
  } else {
    $BaseRef = "HEAD"
  }
}

& git rev-parse --verify --quiet "${BaseRef}^{commit}" | Out-Null
if ($LASTEXITCODE -ne 0) {
  throw "Duplicate-code check requires a valid base ref, got '$BaseRef'."
}

function Invoke-Jscpd {
  param(
    [Parameter(Mandatory = $true)][string[]]$Paths,
    [Parameter(Mandatory = $true)][string]$Format
  )

  Write-Host "Checking $Format clones introduced after $BaseRef"
  & bunx --bun jscpd@5.1.1 @Paths `
    --format $Format `
    --min-tokens 100 `
    --min-lines 10 `
    --skip-comments `
    --ignore "**/tests/**,**/tests.rs,**/*_test.rs,**/node_modules/**,**/.build/**,**/protocol/management.rs,**/protocol/management/account.rs,**/app/account_runtime.rs,**/store/usage.rs,**/store/usage/**,**/local_pool/commands/runtime.rs,**/local_pool/commands/automations.rs,**/local_pool/state.rs,**/local_pool/store/mod.rs,**/store/migrations.rs,**/usage_writer.rs" `
    --baseline-from-ref $BaseRef `
    --fail-on-new-clones 0 `
    --reporters console `
    --no-colors `
    --no-tips

  if ($LASTEXITCODE -ne 0) {
    exit $LASTEXITCODE
  }
}

Invoke-Jscpd -Paths @("crates", "relay-server", "src-tauri") -Format "rust"
Invoke-Jscpd -Paths @("src/src") -Format "typescript"
