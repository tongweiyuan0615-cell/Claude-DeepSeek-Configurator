# macOS Version

This folder is reserved for the macOS port.

The macOS implementation should stay isolated from the Windows app. Planned behavior:

- Bundle a macOS Node.js runtime.
- Install `@anthropic-ai/claude-code@2.1.148`.
- Write DeepSeek settings through a macOS shell environment file.
- Add a source line to the user's shell profile.
- Support one-click deploy, API key update, verification, and uninstall.

The first macOS build will be an unsigned test build. Apple Developer ID signing and notarization can be added later.
