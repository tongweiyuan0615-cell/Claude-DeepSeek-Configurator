# Claude DeepSeek Configurator

This repository keeps platform builds separated so the working Windows app is not mixed with the macOS port.

- `windows/`: current Windows Tauri app and Windows-specific installer workflow source.
- `macos/`: isolated macOS Tauri app and unsigned test build workflow.

Build workflows live in `.github/workflows/` and point at the matching platform directory.
