# Contributing

Thanks for improving Claude Code + DeepSeek V4 Configurator.

## Development Rules

- Keep Windows and macOS changes isolated unless a shared product behavior needs to change.
- Do not commit generated runtime folders such as `node_modules`, `dist`, `src-tauri/target`, or `src-tauri/resources/node`.
- Do not log or print API Keys.
- Do not add behavior that writes system-level environment variables without explicit discussion.
- Keep Claude Code pinned until DeepSeek compatibility with newer versions is verified.

## Local Checks

Windows frontend:

```bash
cd windows
npm install
npm run build
```

macOS frontend:

```bash
cd macos
npm install
npm run build
```

Platform installers are built by GitHub Actions.

## Pull Requests

Please include:

- What changed
- Which platform is affected
- How it was tested
- Any user-facing behavior changes
