[CmdletBinding()]
param()

$ErrorActionPreference = "Stop"

# Relay contains legacy assertion-heavy test helpers. Keep the production gate
# differential so new agent output cannot add another panic/escape hatch while
# existing tests remain readable and stable.
$diffParts = @()
$baseRef = $env:GITHUB_BASE_REF
if ($baseRef) {
  & git fetch --no-tags --quiet origin $baseRef
  $diffParts += & git diff --unified=0 "origin/$baseRef...HEAD" -- crates relay-server src-tauri
} elseif (git rev-parse --verify HEAD^ 2>$null) {
  $diffParts += & git diff --unified=0 HEAD^ HEAD -- crates relay-server src-tauri
}
$diffParts += & git diff --unified=0 -- crates relay-server src-tauri
$diffParts += & git diff --cached --unified=0 -- crates relay-server src-tauri

$rules = @(
  @{ Name = "panic"; Pattern = '\bpanic!\s*\(' },
  @{ Name = "todo/unimplemented"; Pattern = '\b(todo|unimplemented)!\s*\(' },
  @{ Name = "debug/print"; Pattern = '\b(dbg|print|eprint|println|eprintln)!\s*\(' },
  @{ Name = "mem::forget"; Pattern = '\bmem::forget\s*\(' }
)

$currentPath = ""
$violations = foreach ($line in $diffParts) {
  if ($line -match '^diff --git a/(.+) b/(.+)$') {
    $currentPath = $Matches[2]
    continue
  }
  if ($line -notmatch '^\+' -or $line -match '^\+\+\+' -or
      $currentPath -match '(^|/|\\)tests?(/|\\)|(^|/|\\)[^/\\]+_test\.rs$') {
    continue
  }
  # Compile-time fixture loaders are test-only helpers even when they live in
  # a shared module; their static invariant is explicit and harmless.
  if ($line -match 'fixture' -and $line -match '\.(unwrap|expect)\s*\(') {
    continue
  }
  foreach ($rule in $rules) {
    if ($line -match $rule.Pattern) {
      "[$($rule.Name)] $line"
    }
  }
}
if ($violations) {
  $violations | Select-Object -First 40 | Write-Error
  throw "New production guardrail violations detected. Use typed error paths instead."
}
