# Claude DeepSeek Configurator

Desktop configurator for Claude Code + DeepSeek V4. The app asks for a DeepSeek API Key, installs the pinned Claude Code version, writes local user-level configuration, and keeps the managed Node/Claude runtime separate from the user's existing environment.

The repository intentionally keeps platform builds separated:

- `windows/`: Windows Tauri app, MSI build source, and Windows smoke test.
- `macos/`: macOS Tauri app and unsigned DMG build source.

Build workflows live in `.github/workflows/` and point at the matching platform directory.

## Release Artifacts

- Windows workflow uploads only the MSI installer.
- macOS workflow uploads only the DMG installer. The DMG already contains the `.app`, so the raw `.app` bundle is not uploaded separately.

Both platform builds download Node.js during GitHub Actions and place it under `src-tauri/resources/node` before packaging. The Node runtime is intentionally ignored in Git and represented by `.gitkeep` only.

## Runtime Ownership

The app manages only its own installed runtime:

- Windows: `%LOCALAPPDATA%\ClaudeDeepSeekConfigurator`
- macOS: `~/Library/Application Support/ClaudeDeepSeekConfigurator`

Existing external Node.js or Claude Code installations are not deleted. The app instead prioritizes its managed Claude/Node paths and reports when another Claude command is ahead in PATH.
