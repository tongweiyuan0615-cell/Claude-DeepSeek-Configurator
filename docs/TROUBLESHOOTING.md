# Troubleshooting

## Generate a diagnostic report

Use the `Diagnostic report` button in the app when a user reports that deployment, verification, or uninstall did not work as expected.

The report is designed for support and must not contain the DeepSeek API Key. It reports only whether the key is configured, plus tool versions, managed paths, PATH priority, and missing environment variable names.

Before sharing a report publicly, still review it for local usernames or private filesystem paths.

## Latest Claude Code does not work with DeepSeek

The default deployment button installs the stable DeepSeek-compatible Claude Code version.

The `尝试最新版` button is experimental. It installs the latest Claude Code into a separate managed directory and switches this app's managed PATH priority to that version.

If the latest version does not work with DeepSeek, use `回退稳定版` to switch back to the pinned compatible version. The rollback does not remove user-installed Node.js or user-installed Claude Code.

## macOS says the app is damaged or cannot be opened

The current macOS build is unsigned. After copying the app to `/Applications`, run:

```bash
xattr -cr "/Applications/Claude Code DeepSeek Configurator macOS.app"
```

Then open the app again.

## `claude` still works after one-click uninstall

Close and reopen Terminal, iTerm, or VS Code terminal first. Already-open terminals keep the old PATH and command cache.

If `claude` still works in a new terminal, the user probably has another external Claude Code installation. The app only removes its own managed runtime:

```text
~/Library/Application Support/ClaudeDeepSeekConfigurator
```

## Windows still uses another Claude Code

Open a new PowerShell or CMD and run:

```powershell
where claude
```

The first result should point to the app-managed directory under:

```text
%LOCALAPPDATA%\ClaudeDeepSeekConfigurator
```

If a system-level Claude path appears first, it may require manual removal by the user or an administrator.

## One-click deployment fails during npm install

Check whether the machine can access npm registry and Node.js related network resources. Corporate proxies, antivirus tools, or network firewalls can block package installation.

The app uses its own bundled Node.js runtime, so users normally do not need to install Node.js manually.

## DeepSeek configuration is written but Claude requests fail

Ask the user to reopen the terminal and verify:

```bash
claude --version
```

Then confirm the DeepSeek API Key is valid and has available balance.
