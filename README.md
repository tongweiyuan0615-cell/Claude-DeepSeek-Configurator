# Claude Code + DeepSeek V4 Configurator

[![Build Windows installer](https://github.com/tongweiyuan0615-cell/Claude-DeepSeek-Configurator/actions/workflows/build-windows.yml/badge.svg)](https://github.com/tongweiyuan0615-cell/Claude-DeepSeek-Configurator/actions/workflows/build-windows.yml)
[![Build macOS installer](https://github.com/tongweiyuan0615-cell/Claude-DeepSeek-Configurator/actions/workflows/build-macos.yml/badge.svg)](https://github.com/tongweiyuan0615-cell/Claude-DeepSeek-Configurator/actions/workflows/build-macos.yml)
[![Windows smoke test](https://github.com/tongweiyuan0615-cell/Claude-DeepSeek-Configurator/actions/workflows/windows-smoke.yml/badge.svg)](https://github.com/tongweiyuan0615-cell/Claude-DeepSeek-Configurator/actions/workflows/windows-smoke.yml)

一个面向普通用户的 Claude Code + DeepSeek V4 一键配置桌面工具。用户只需要输入 DeepSeek API Key，点击一键部署，软件会自动准备稳定版 Claude Code、内置 Node.js 运行时，并写入本机用户级配置。

## 核心特性

- 一键部署 Claude Code + DeepSeek V4
- 内置 Node.js 运行时，不要求用户提前安装 Node/npm
- 固定安装当前兼容版 Claude Code，避免自动升级导致 DeepSeek 接入异常
- API Key 只写入本机，不上传、不打印到日志
- Windows 写入用户级环境变量，不写系统级环境变量
- macOS 写入用户目录配置文件，并注入 shell profile
- 检测并修复 Claude 命令 PATH 优先级，尽量避免被用户已有环境干扰
- 支持修改 API Key、验证配置、一键卸载

## 支持平台

| 平台 | 产物 | 状态 | 说明 |
| --- | --- | --- | --- |
| Windows | `.msi` | 可用 | 普通用户安装后直接打开使用 |
| macOS | `.dmg` | 可用 | 当前为未签名测试包，首次使用可能需要清除 quarantine |

## 用户如何使用

### Windows

1. 下载 GitHub Actions 生成的 Windows MSI。
2. 安装并打开软件。
3. 输入激活码和 DeepSeek API Key。
4. 点击一键部署。
5. 重新打开 PowerShell / CMD / VS Code 终端。
6. 执行 `claude` 验证。

### macOS

1. 下载 GitHub Actions 生成的 macOS DMG。
2. 拖拽 App 到 `/Applications`。
3. 如系统提示无法打开，执行：

```bash
xattr -cr "/Applications/Claude Code DeepSeek Configurator macOS.app"
```

4. 打开软件，输入激活码和 DeepSeek API Key。
5. 点击一键部署。
6. 重新打开 Terminal / iTerm / VS Code 终端。
7. 执行 `claude` 验证。

## 仓库结构

```text
.
├── .github/workflows/       # Windows/macOS 打包和 Windows smoke test
├── docs/                    # 发布、排障和维护文档
├── windows/                 # Windows Tauri 应用
└── macos/                   # macOS Tauri 应用
```

两个平台目录故意隔离，避免 Windows 稳定版本和 macOS 移植版本互相污染。

## 构建产物

- Windows workflow 只上传 MSI。
- macOS workflow 只上传 DMG。DMG 内已经包含 `.app`，不会重复上传原始 `.app` 目录。

两个平台都会在 GitHub Actions 中下载 Node.js，并放入 `src-tauri/resources/node` 再打包。Node runtime 不提交到 Git，只保留 `.gitkeep`。

## 运行时目录

软件只管理自己安装的运行时，不会删除用户自己安装的 Node.js 或 Claude Code。

| 平台 | 管理目录 |
| --- | --- |
| Windows | `%LOCALAPPDATA%\ClaudeDeepSeekConfigurator` |
| macOS | `~/Library/Application Support/ClaudeDeepSeekConfigurator` |

## 本地开发

Windows:

```bash
cd windows
npm install
npm run build
```

macOS:

```bash
cd macos
npm install
npm run build
```

完整桌面打包建议使用 GitHub Actions，因为 Actions 会自动准备平台对应的 Node runtime 和 Tauri 打包环境。

## 相关文档

- [发布检查清单](docs/RELEASE.md)
- [常见问题排查](docs/TROUBLESHOOTING.md)
- [更新记录](CHANGELOG.md)
- [贡献指南](CONTRIBUTING.md)

## License

MIT License. See [LICENSE](LICENSE).
