# macOS Version

This folder contains the isolated macOS Tauri app.

It bundles a portable macOS Node.js runtime, installs the pinned Claude Code version, and writes DeepSeek configuration to `~/.claude-deepseek-env`.

The current macOS build is unsigned. Apple Developer ID signing and notarization can be added later.

## Build

Use the `Build macOS installer` GitHub Actions workflow. It downloads the matching macOS Node.js runtime during CI and uploads the DMG artifact only.

Local generated folders such as `node_modules`, `dist`, `src-tauri/target`, and `src-tauri/resources/node` are ignored.

## Runtime Notes

The deployed runtime lives under `~/Library/Application Support/ClaudeDeepSeekConfigurator`. The app also writes a shell profile block that sources `~/.claude-deepseek-env`.

Because the app is unsigned, users may need to clear quarantine after copying it to `/Applications`:

```bash
xattr -cr "/Applications/Claude Code DeepSeek Configurator macOS.app"
```
