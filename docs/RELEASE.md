# Release Checklist

Use this checklist before sharing installers with users.

## Windows

1. Run the `Windows smoke test` workflow.
2. Run the `Build Windows installer` workflow.
3. Download the `windows-installers` artifact.
4. Install the MSI on a Windows machine without relying on a preinstalled Node.js.
5. Open the app, activate it, enter a test DeepSeek API Key, and click one-click deployment.
6. Reopen PowerShell or CMD.
7. Run:

```powershell
claude --version
claude
```

8. Confirm Claude Code uses the managed compatible version.

## macOS

1. Run the `Build macOS installer` workflow.
2. Download the `macos-installers` artifact.
3. Install the DMG and drag the app into `/Applications`.
4. If needed, clear quarantine:

```bash
xattr -cr "/Applications/Claude Code DeepSeek Configurator macOS.app"
```

5. Open the app, activate it, enter a test DeepSeek API Key, and click one-click deployment.
6. Reopen Terminal or iTerm.
7. Run:

```bash
claude --version
claude
```

8. Test one-click uninstall and then reopen Terminal to confirm managed Claude is removed.

## Before Publishing

- Run:

```bash
node scripts/check-version-consistency.mjs
```

- Confirm no API Key appears in logs, screenshots, or issue comments.
- Confirm diagnostic reports redact the DeepSeek API Key and only show whether it is configured.
- Confirm `尝试最新版` can install the latest Claude Code and `回退稳定版` returns to the pinned compatible version.
- Confirm artifact names are clear.
- Confirm only MSI is uploaded for Windows and only DMG is uploaded for macOS.
- Update `CHANGELOG.md` if the release contains user-facing changes.
