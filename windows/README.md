# Windows Version

This folder contains the current Windows desktop app.

It bundles a portable Windows Node.js runtime, installs the pinned Claude Code version, and writes DeepSeek configuration to Windows user-level environment variables.

Use this folder for Windows-only fixes and releases.

## Build

Use the `Build Windows installer` GitHub Actions workflow. It downloads Node.js during CI and uploads the MSI artifact only.

Local generated folders such as `node_modules`, `dist`, `src-tauri/target`, and `src-tauri/resources/node` are ignored.

## Runtime Notes

The deployed runtime lives under `%LOCALAPPDATA%\ClaudeDeepSeekConfigurator`. Uninstalling or deleting the desktop app does not remove an already deployed Claude setup unless the user runs the app's one-click uninstall first.
