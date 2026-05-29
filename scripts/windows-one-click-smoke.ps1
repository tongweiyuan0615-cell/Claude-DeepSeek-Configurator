param(
  [string]$ApiKey = "smoke-test-token-do-not-use"
)

$ErrorActionPreference = "Stop"
$ClaudeCompatVersion = "2.1.148"

$deepseekEnv = [ordered]@{
  "ANTHROPIC_BASE_URL" = "https://api.deepseek.com/anthropic"
  "ANTHROPIC_AUTH_TOKEN" = $ApiKey
  "ANTHROPIC_MODEL" = "deepseek-v4-pro[1m]"
  "ANTHROPIC_DEFAULT_OPUS_MODEL" = "deepseek-v4-pro[1m]"
  "ANTHROPIC_DEFAULT_SONNET_MODEL" = "deepseek-v4-pro[1m]"
  "ANTHROPIC_DEFAULT_HAIKU_MODEL" = "deepseek-v4-flash"
  "CLAUDE_CODE_SUBAGENT_MODEL" = "deepseek-v4-flash"
  "CLAUDE_CODE_EFFORT_LEVEL" = "max"
}

$compatEnv = [ordered]@{
  "DISABLE_AUTOUPDATER" = "1"
}

function Refresh-ProcessPath {
  $machinePath = [Environment]::GetEnvironmentVariable("Path", "Machine")
  $userPath = [Environment]::GetEnvironmentVariable("Path", "User")
  $candidatePaths = @(
    $env:Path,
    $machinePath,
    $userPath,
    "$env:USERPROFILE\.local\bin",
    "$env:APPDATA\npm"
  )
  $env:Path = ($candidatePaths | Where-Object { $_ }) -join ";"
}

function Get-ClaudeCandidates {
  @(
    "$env:USERPROFILE\.local\bin\claude.exe",
    "$env:APPDATA\npm\claude.cmd",
    "$env:APPDATA\npm\claude.exe"
  )
}

function Test-ClaudeCommand {
  Refresh-ProcessPath
  $version = cmd.exe /C "claude --version" 2>&1
  if ($LASTEXITCODE -eq 0 -and "$version" -like "*$ClaudeCompatVersion*") {
    Write-Host "Claude Code version: $version"
    return
  }

  foreach ($candidate in Get-ClaudeCandidates) {
    if (-not (Test-Path $candidate)) {
      continue
    }

    $version = & $candidate --version 2>&1
    if ($LASTEXITCODE -eq 0 -and "$version" -like "*$ClaudeCompatVersion*") {
      Write-Host "Claude Code version: $version"
      Write-Host "Claude Code path: $candidate"
      return
    }
  }

  throw "claude --version failed or incompatible: $version"
}

function Install-ClaudeCode {
  $existing = Get-Command claude -ErrorAction SilentlyContinue
  if ($existing) {
    $existingVersion = claude --version 2>&1
    if ($LASTEXITCODE -eq 0 -and "$existingVersion" -like "*$ClaudeCompatVersion*") {
      Write-Host "Claude Code already exists at $($existing.Source); skipping install."
      return
    }

    Write-Host "Claude Code exists but is not the DeepSeek-compatible version: $existingVersion"
  }

  Write-Host "Installing Claude Code $ClaudeCompatVersion with the official Windows installer."
  try {
    $script = Invoke-RestMethod "https://claude.ai/install.ps1"
    & ([scriptblock]::Create($script)) $ClaudeCompatVersion
    Test-ClaudeCommand
    return
  } catch {
    Write-Host "Official installer failed; trying npm fallback when npm is available."
    Write-Host $_
  }

  $npm = Get-Command npm.cmd -ErrorAction SilentlyContinue
  if (-not $npm) {
    throw "Claude Code is not installed and npm fallback is not available."
  }

  npm.cmd install -g "@anthropic-ai/claude-code@$ClaudeCompatVersion"
  if ($LASTEXITCODE -ne 0) {
    throw "npm fallback failed with exit code $LASTEXITCODE"
  }

  Test-ClaudeCommand
}

try {
  foreach ($entry in $compatEnv.GetEnumerator()) {
    [Environment]::SetEnvironmentVariable($entry.Key, $entry.Value, "User")
  }

  Install-ClaudeCode

  foreach ($entry in $deepseekEnv.GetEnumerator()) {
    [Environment]::SetEnvironmentVariable($entry.Key, $entry.Value, "User")
  }

  foreach ($entry in $deepseekEnv.GetEnumerator()) {
    $actual = [Environment]::GetEnvironmentVariable($entry.Key, "User")
    if ($actual -ne $entry.Value) {
      throw "User environment variable check failed for $($entry.Key)"
    }
  }

  Test-ClaudeCommand
  Write-Host "Windows one-click smoke test passed."
} finally {
  foreach ($name in $deepseekEnv.Keys) {
    [Environment]::SetEnvironmentVariable($name, $null, "User")
  }
  foreach ($name in $compatEnv.Keys) {
    [Environment]::SetEnvironmentVariable($name, $null, "User")
  }
}
