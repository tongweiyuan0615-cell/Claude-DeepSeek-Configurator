param(
  [string]$ApiKey = "smoke-test-token-do-not-use"
)

$ErrorActionPreference = "Stop"

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

function Refresh-ProcessPath {
  $machinePath = [Environment]::GetEnvironmentVariable("Path", "Machine")
  $userPath = [Environment]::GetEnvironmentVariable("Path", "User")
  $env:Path = @($env:Path, $machinePath, $userPath) -join ";"
}

function Test-ClaudeCommand {
  Refresh-ProcessPath
  $version = cmd.exe /C "claude --version" 2>&1
  if ($LASTEXITCODE -ne 0) {
    throw "claude --version failed: $version"
  }
  Write-Host "Claude Code version: $version"
}

function Install-ClaudeCode {
  $existing = Get-Command claude -ErrorAction SilentlyContinue
  if ($existing) {
    Write-Host "Claude Code already exists at $($existing.Source); skipping install."
    return
  }

  Write-Host "Installing Claude Code with the official Windows installer."
  try {
    $script = Invoke-RestMethod "https://claude.ai/install.ps1"
    Invoke-Expression $script
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

  npm.cmd install -g "@anthropic-ai/claude-code"
  if ($LASTEXITCODE -ne 0) {
    throw "npm fallback failed with exit code $LASTEXITCODE"
  }

  Test-ClaudeCommand
}

try {
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
}
