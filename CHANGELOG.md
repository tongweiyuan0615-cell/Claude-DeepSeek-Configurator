# Changelog

All notable changes to this project are documented here.

## Unreleased

- Added experimental latest Claude Code installation and one-click rollback to the stable compatible version.
- Added redacted diagnostic reports for Windows and macOS support.
- Added CI version consistency checks across Windows and macOS manifests.
- Aligned Windows package metadata with release version 0.2.0.
- Hardened Windows and macOS Claude PATH priority handling.
- Added deterministic macOS one-click uninstall behavior.
- Reduced macOS GitHub Actions artifact size by uploading only DMG files.
- Separated Windows and macOS apps into isolated project directories.
- Added bundled Node.js runtime packaging for both platforms.
- Pinned Claude Code to the current DeepSeek-compatible version.

## 0.2.0

- Added macOS unsigned DMG build.
- Added macOS shell profile integration for DeepSeek environment variables.
- Added macOS managed Node.js and Claude Code runtime support.

## 0.1.3

- Added Windows managed Node.js runtime.
- Added Windows one-click deployment, API Key update, verification, and uninstall.
- Added PATH priority checks for managed Claude Code.
