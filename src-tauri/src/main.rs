#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use serde::Serialize;
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};
use tauri::Manager;

const DEEPSEEK_ENV_VARS: &[(&str, &str)] = &[
    ("ANTHROPIC_BASE_URL", "https://api.deepseek.com/anthropic"),
    ("ANTHROPIC_MODEL", "deepseek-v4-pro[1m]"),
    ("ANTHROPIC_DEFAULT_OPUS_MODEL", "deepseek-v4-pro[1m]"),
    ("ANTHROPIC_DEFAULT_SONNET_MODEL", "deepseek-v4-pro[1m]"),
    ("ANTHROPIC_DEFAULT_HAIKU_MODEL", "deepseek-v4-flash"),
    ("CLAUDE_CODE_SUBAGENT_MODEL", "deepseek-v4-flash"),
    ("CLAUDE_CODE_EFFORT_LEVEL", "max"),
];

const AUTH_TOKEN_ENV_VAR: &str = "ANTHROPIC_AUTH_TOKEN";
const CLAUDE_COMPAT_VERSION: &str = "2.1.148";
const CLAUDE_PACKAGE: &str = "@anthropic-ai/claude-code";
const CLAUDE_AUTOUPDATER_ENV_VAR: &str = "DISABLE_AUTOUPDATER";
const NODE_RUNTIME_VERSION: &str = "20.20.2";
const MANAGED_DIR_NAME: &str = "ClaudeDeepSeekConfigurator";

#[derive(Serialize)]
struct ToolCheck {
    installed: bool,
    version: Option<String>,
    meets_requirement: Option<bool>,
    message: String,
}

#[derive(Serialize)]
struct EnvironmentStatus {
    node: ToolCheck,
    npm: ToolCheck,
    claude: ToolCheck,
    deepseek_configured: bool,
    missing_env_vars: Vec<String>,
}

#[derive(Serialize)]
struct CommandResult {
    success: bool,
    message: String,
    output: Option<String>,
}

async fn run_blocking<F>(task: F) -> CommandResult
where
    F: FnOnce() -> CommandResult + Send + 'static,
{
    match tauri::async_runtime::spawn_blocking(task).await {
        Ok(result) => result,
        Err(error) => CommandResult {
            success: false,
            message: "后台任务执行失败".to_string(),
            output: Some(error.to_string()),
        },
    }
}

fn command_output(program: &str, args: &[&str]) -> Result<String, String> {
    let output = Command::new(program)
        .args(args)
        .output()
        .map_err(|error| format!("无法执行 {program}: {error}"))?;

    command_result_from_output(program, output)
}

fn command_output_with_timeout(program: &str, args: &[&str], timeout: Duration) -> Result<String, String> {
    let mut child = Command::new(program)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("无法执行 {program}: {error}"))?;

    let started_at = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => {
                let output = child
                    .wait_with_output()
                    .map_err(|error| format!("读取 {program} 输出失败: {error}"))?;
                return command_result_from_output(program, output);
            }
            Ok(None) => {
                if started_at.elapsed() >= timeout {
                    let _ = child.kill();
                    let output = child
                        .wait_with_output()
                        .map_err(|error| format!("{program} 执行超时，且读取输出失败: {error}"))?;
                    let details = command_text_from_output(&output);
                    return Err(if details.trim().is_empty() {
                        format!("{program} 执行超时")
                    } else {
                        format!("{program} 执行超时\n{details}")
                    });
                }

                thread::sleep(Duration::from_millis(200));
            }
            Err(error) => return Err(format!("检查 {program} 执行状态失败: {error}")),
        }
    }
}

fn command_result_from_output(program: &str, output: Output) -> Result<String, String> {
    let combined = command_text_from_output(&output);

    if output.status.success() {
        Ok(combined)
    } else {
        Err(if combined.is_empty() {
            format!("{program} exited with status {}", output.status)
        } else {
            combined
        })
    }
}

fn command_text_from_output(output: &Output) -> String {
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if stderr.is_empty() {
        stdout
    } else if stdout.is_empty() {
        stderr
    } else {
        format!("{stdout}\n{stderr}")
    }
}

fn bounded_output(output: String) -> String {
    const MAX_CHARS: usize = 5000;
    if output.chars().count() <= MAX_CHARS {
        return output;
    }

    let mut truncated: String = output.chars().take(MAX_CHARS).collect();
    truncated.push_str("\n...输出已截断");
    truncated
}

fn check_node(app: &tauri::AppHandle) -> ToolCheck {
    if let Ok(node_dir) = managed_node_dir() {
        let node_exe = node_dir.join("node.exe");
        if node_exe.exists() {
            return match command_output(&node_exe.to_string_lossy(), &["--version"]) {
                Ok(version) => ToolCheck {
                    installed: true,
                    version: Some(version.clone()),
                    meets_requirement: Some(true),
                    message: format!("内置运行时已启用 {version}"),
                },
                Err(error) => ToolCheck {
                    installed: false,
                    version: None,
                    meets_requirement: Some(false),
                    message: error,
                },
            };
        }
    }

    if bundled_node_dir(app).is_some() {
        return ToolCheck {
            installed: true,
            version: Some(format!("v{NODE_RUNTIME_VERSION}")),
            meets_requirement: Some(true),
            message: format!("已随软件内置 v{NODE_RUNTIME_VERSION}，部署时自动启用"),
        };
    }

    match command_output("node", &["--version"]) {
        Ok(version) => {
            let supported = parse_major_version(&version).map_or(false, |major| major >= 18);
            ToolCheck {
                installed: true,
                version: Some(version.clone()),
                meets_requirement: Some(supported),
                message: if supported {
                    format!("已安装 {version}")
                } else {
                    format!("版本过低：{version}，需要 >= 18")
                },
            }
        }
        Err(error) => ToolCheck {
            installed: false,
            version: None,
            meets_requirement: Some(false),
            message: error,
        },
    }
}

fn check_npm(app: &tauri::AppHandle) -> ToolCheck {
    if let Ok(node_dir) = managed_node_dir() {
        let npm_cmd = node_dir.join("npm.cmd");
        if npm_cmd.exists() {
            return match command_output(&npm_cmd.to_string_lossy(), &["--version"]) {
                Ok(version) => ToolCheck {
                    installed: true,
                    version: Some(version.clone()),
                    meets_requirement: None,
                    message: format!("内置 npm 已启用 {version}"),
                },
                Err(error) => ToolCheck {
                    installed: false,
                    version: None,
                    meets_requirement: None,
                    message: error,
                },
            };
        }
    }

    if bundled_node_dir(app).is_some() {
        return ToolCheck {
            installed: true,
            version: None,
            meets_requirement: None,
            message: "已随软件内置，部署时自动启用".to_string(),
        };
    }

    match command_output("npm.cmd", &["--version"]) {
        Ok(version) => ToolCheck {
            installed: true,
            version: Some(version.clone()),
            meets_requirement: None,
            message: format!("已安装 {version}"),
        },
        Err(error) => ToolCheck {
            installed: false,
            version: None,
            meets_requirement: None,
            message: error,
        },
    }
}

fn check_claude() -> ToolCheck {
    match command_output("cmd", &["/C", "claude", "--version"]) {
        Ok(version) => {
            let path_check = claude_tool_check(version);
            if path_check.meets_requirement != Some(false) {
                return path_check;
            }

            match command_output_from_candidates(&claude_candidates(), &["--version"]) {
                Ok(candidate_version) => {
                    let candidate_check = claude_tool_check(candidate_version);
                    if candidate_check.meets_requirement != Some(false) {
                        candidate_check
                    } else {
                        path_check
                    }
                }
                Err(_) => path_check,
            }
        }
        Err(path_error) => match command_output_from_candidates(&claude_candidates(), &["--version"]) {
            Ok(version) => claude_tool_check(version),
            Err(candidate_error) => ToolCheck {
                installed: false,
                version: None,
                meets_requirement: Some(false),
                message: format!("{path_error}\n{candidate_error}"),
            },
        },
    }
}

fn claude_tool_check(version: String) -> ToolCheck {
    let compatible = is_compatible_claude_version(&version);

    ToolCheck {
        installed: true,
        version: Some(version.clone()),
        meets_requirement: Some(compatible),
        message: if compatible {
            format!("已安装 {version}")
        } else {
            format!("已安装 {version}，但 DeepSeek 当前兼容版本需要 {CLAUDE_COMPAT_VERSION}")
        },
    }
}

fn is_compatible_claude_version(version: &str) -> bool {
    version.contains(CLAUDE_COMPAT_VERSION)
}

fn parse_major_version(version: &str) -> Option<u64> {
    version
        .trim()
        .trim_start_matches('v')
        .split('.')
        .next()
        .and_then(|major| major.parse::<u64>().ok())
}

fn deepseek_config_status() -> (bool, Vec<String>) {
    let mut missing = Vec::new();

    for (name, expected) in DEEPSEEK_ENV_VARS {
        match read_user_env_var(name) {
            Some(value) if value == *expected => {}
            _ => missing.push((*name).to_string()),
        }
    }

    match read_user_env_var(AUTH_TOKEN_ENV_VAR) {
        Some(value) if !value.trim().is_empty() => {}
        _ => missing.push(AUTH_TOKEN_ENV_VAR.to_string()),
    }

    (missing.is_empty(), missing)
}

#[tauri::command]
fn check_environment(app: tauri::AppHandle) -> EnvironmentStatus {
    let (deepseek_configured, missing_env_vars) = deepseek_config_status();

    EnvironmentStatus {
        node: check_node(&app),
        npm: check_npm(&app),
        claude: check_claude(),
        deepseek_configured,
        missing_env_vars,
    }
}

fn install_claude_native(app: &tauri::AppHandle) -> CommandResult {
    if let Err(error) = ensure_windows() {
        return error;
    }

    let mut log = Vec::new();
    if let Err(error) = write_user_env_var(CLAUDE_AUTOUPDATER_ENV_VAR, "1") {
        return CommandResult {
            success: false,
            message: format!("写入 {CLAUDE_AUTOUPDATER_ENV_VAR} 失败"),
            output: Some(error),
        };
    }
    std::env::set_var(CLAUDE_AUTOUPDATER_ENV_VAR, "1");
    log.push("已禁用 Claude Code 自动更新，避免自动升级到 DeepSeek 暂不兼容版本。".to_string());

    let (npm_cmd, node_dir) = match resolve_npm_cmd(app, &mut log) {
        Ok(result) => result,
        Err(error) => {
            return CommandResult {
                success: false,
                message: "Claude Code 安装失败".to_string(),
                output: Some(error),
            };
        }
    };

    let claude_prefix = match managed_claude_prefix_dir() {
        Ok(path) => path,
        Err(error) => {
            return CommandResult {
                success: false,
                message: "Claude Code 安装失败".to_string(),
                output: Some(error),
            };
        }
    };

    if let Err(error) = fs::create_dir_all(&claude_prefix) {
        return CommandResult {
            success: false,
            message: "Claude Code 安装失败".to_string(),
            output: Some(format!("创建 Claude Code 安装目录失败：{error}")),
        };
    }

    if let Err(error) = ensure_user_path_entry_first(&claude_prefix) {
        return CommandResult {
            success: false,
            message: "Claude Code 安装失败".to_string(),
            output: Some(error),
        };
    }

    if let Some(node_dir) = &node_dir {
        if let Err(error) = ensure_user_path_entry_first(node_dir) {
            return CommandResult {
                success: false,
                message: "Claude Code 安装失败".to_string(),
                output: Some(error),
            };
        }
        prepend_process_path(&[claude_prefix.clone(), node_dir.clone()]);
    } else {
        prepend_process_path(&[claude_prefix.clone()]);
    }

    let package_spec = format!("{CLAUDE_PACKAGE}@{CLAUDE_COMPAT_VERSION}");
    let prefix_arg = claude_prefix.to_string_lossy().to_string();
    let npm_program = npm_cmd.to_string_lossy().to_string();
    match command_output_with_timeout(
        &npm_program,
        &["install", "-g", "--prefix", &prefix_arg, &package_spec],
        Duration::from_secs(240),
    ) {
        Ok(output) => {
            if !output.trim().is_empty() {
                log.push(format!("Claude Code 安装输出：\n{}", bounded_output(output)));
            }

            remove_incompatible_claude_binaries(&mut log);
            refresh_process_path_from_registry();
            let check = check_claude();
            if check.installed && check.meets_requirement != Some(false) {
                CommandResult {
                    success: true,
                    message: format!("Claude Code {CLAUDE_COMPAT_VERSION} 安装完成"),
                    output: Some(log.join("\n\n")).filter(|value| !value.trim().is_empty()),
                }
            } else {
                CommandResult {
                    success: false,
                    message: "Claude Code 安装后验证失败".to_string(),
                    output: Some(format!("{}\n\n{}", log.join("\n\n"), check.message)),
                }
            }
        }
        Err(error) => {
            log.push(format!("npm 兜底安装失败：{}", bounded_output(error)));
            CommandResult {
                success: false,
                message: "Claude Code 安装失败".to_string(),
                output: Some(log.join("\n\n")),
            }
        }
    }
}

#[tauri::command]
fn configure_deepseek(api_key: String) -> CommandResult {
    configure_deepseek_internal(api_key)
}

#[tauri::command]
fn update_api_key(api_key: String) -> CommandResult {
    let mut result = configure_deepseek_internal(api_key);
    if result.success {
        result.message = "API Key 已更新。请重新打开 PowerShell / CMD / VS Code 终端。".to_string();
    }
    result
}

fn configure_deepseek_internal(api_key: String) -> CommandResult {
    if api_key.trim().is_empty() {
        return CommandResult {
            success: false,
            message: "请输入 DeepSeek API Key".to_string(),
            output: None,
        };
    }

    if let Err(error) = ensure_windows() {
        return error;
    }

    if let Err(error) = write_user_env_var(CLAUDE_AUTOUPDATER_ENV_VAR, "1") {
        return CommandResult {
            success: false,
            message: format!("写入 {CLAUDE_AUTOUPDATER_ENV_VAR} 失败"),
            output: Some(error),
        };
    }
    std::env::set_var(CLAUDE_AUTOUPDATER_ENV_VAR, "1");

    for (name, value) in DEEPSEEK_ENV_VARS {
        if let Err(error) = write_user_env_var(name, value) {
            return CommandResult {
                success: false,
                message: format!("写入 {name} 失败"),
                output: Some(error),
            };
        }
    }

    if let Err(error) = write_user_env_var(AUTH_TOKEN_ENV_VAR, api_key.trim()) {
        return CommandResult {
            success: false,
            message: "写入 ANTHROPIC_AUTH_TOKEN 失败".to_string(),
            output: Some(error),
        };
    }

    broadcast_environment_change();

    CommandResult {
        success: true,
        message: "配置完成。请重新打开 PowerShell / CMD / VS Code 终端。".to_string(),
        output: None,
    }
}

#[tauri::command]
async fn one_click_setup(app: tauri::AppHandle, api_key: String) -> CommandResult {
    run_blocking(move || one_click_setup_internal(&app, api_key)).await
}

fn one_click_setup_internal(app: &tauri::AppHandle, api_key: String) -> CommandResult {
    if api_key.trim().is_empty() {
        return CommandResult {
            success: false,
            message: "请输入 DeepSeek API Key".to_string(),
            output: None,
        };
    }

    let mut log = Vec::new();

    let claude_check = check_claude();
    if !claude_check.installed || claude_check.meets_requirement == Some(false) {
        if claude_check.installed {
            log.push(format!(
                "检测到 {}，将切换到 DeepSeek 当前兼容版本 {CLAUDE_COMPAT_VERSION}",
                claude_check.message
            ));
        }

        let install_result = install_claude_native(app);
        log.push(install_result.message.clone());

        if let Some(output) = install_result.output {
            log.push(output);
        }

        if !install_result.success {
            return CommandResult {
                success: false,
                message: "一键部署失败：Claude Code 安装未完成".to_string(),
                output: Some(log.join("\n\n")),
            };
        }
    } else {
        log.push(format!(
            "Claude Code 已安装且版本兼容（{CLAUDE_COMPAT_VERSION}），跳过安装步骤"
        ));
    }

    let configure_result = configure_deepseek_internal(api_key);
    log.push(configure_result.message.clone());

    if !configure_result.success {
        if let Some(output) = configure_result.output {
            log.push(output);
        }

        return CommandResult {
            success: false,
            message: "一键部署失败：DeepSeek 环境变量写入未完成".to_string(),
            output: Some(log.join("\n\n")),
        };
    }

    let verify_result = verify_claude();
    log.push(verify_result.message.clone());

    if let Some(output) = verify_result.output {
        log.push(output);
    }

    if verify_result.success {
        CommandResult {
            success: true,
            message: "一键部署完成。请重新打开 PowerShell / CMD / VS Code 终端。".to_string(),
            output: Some(log.join("\n\n")),
        }
    } else {
        CommandResult {
            success: false,
            message: "环境变量已写入，但 Claude Code 验证失败".to_string(),
            output: Some(log.join("\n\n")),
        }
    }
}

#[tauri::command]
fn verify_claude() -> CommandResult {
    refresh_process_path_from_registry();

    match command_output("cmd", &["/C", "claude", "--version"]) {
        Ok(output) => {
            let result = verify_claude_version_result(output);
            if result.success {
                return result;
            }

            match command_output_from_candidates(&claude_candidates(), &["--version"]) {
                Ok(candidate_output) => verify_claude_version_result(candidate_output),
                Err(_) => result,
            }
        }
        Err(path_error) => match command_output_from_candidates(&claude_candidates(), &["--version"]) {
            Ok(output) => verify_claude_version_result(output),
            Err(candidate_error) => CommandResult {
                success: false,
                message: "Claude Code 验证失败".to_string(),
                output: Some(format!("{path_error}\n{candidate_error}")),
            },
        },
    }
}

fn verify_claude_version_result(output: String) -> CommandResult {
    if is_compatible_claude_version(&output) {
        CommandResult {
            success: true,
            message: "Claude Code 可执行且版本兼容".to_string(),
            output: Some(output),
        }
    } else {
        CommandResult {
            success: false,
            message: format!("Claude Code 版本不兼容，需要 {CLAUDE_COMPAT_VERSION}"),
            output: Some(output),
        }
    }
}

fn clear_deepseek_config_internal() -> CommandResult {
    if let Err(error) = ensure_windows() {
        return error;
    }

    let mut errors = Vec::new();
    for (name, _) in DEEPSEEK_ENV_VARS {
        if let Err(error) = delete_user_env_var(name) {
            errors.push(format!("{name}: {error}"));
        }
    }

    if let Err(error) = delete_user_env_var(AUTH_TOKEN_ENV_VAR) {
        errors.push(format!("{AUTH_TOKEN_ENV_VAR}: {error}"));
    }

    if let Err(error) = delete_user_env_var(CLAUDE_AUTOUPDATER_ENV_VAR) {
        errors.push(format!("{CLAUDE_AUTOUPDATER_ENV_VAR}: {error}"));
    }

    broadcast_environment_change();

    if errors.is_empty() {
        CommandResult {
            success: true,
            message: "DeepSeek 环境变量已清除。请重新打开 PowerShell / CMD / VS Code 终端。".to_string(),
            output: None,
        }
    } else {
        CommandResult {
            success: false,
            message: "部分环境变量清除失败".to_string(),
            output: Some(errors.join("\n")),
        }
    }
}

#[tauri::command]
async fn one_click_uninstall() -> CommandResult {
    run_blocking(one_click_uninstall_internal).await
}

fn one_click_uninstall_internal() -> CommandResult {
    if let Err(error) = ensure_windows() {
        return error;
    }

    let mut log = Vec::new();
    let clear_result = clear_deepseek_config_internal();
    log.push(clear_result.message.clone());
    if let Some(output) = clear_result.output {
        log.push(output);
    }

    if let Ok(claude_prefix) = managed_claude_prefix_dir() {
        let _ = remove_user_path_entry(&claude_prefix);

        if let Ok(node_dir) = managed_node_dir() {
            let _ = remove_user_path_entry(&node_dir);
            let npm_cmd = node_dir.join("npm.cmd");
            if npm_cmd.exists() {
                let npm_program = npm_cmd.to_string_lossy().to_string();
                let prefix_arg = claude_prefix.to_string_lossy().to_string();
                match command_output_with_timeout(
                    &npm_program,
                    &["uninstall", "-g", "--prefix", &prefix_arg, CLAUDE_PACKAGE],
                    Duration::from_secs(120),
                ) {
                    Ok(output) => {
                        if !output.trim().is_empty() {
                            log.push(format!("内置 npm 卸载输出：\n{}", bounded_output(output)));
                        } else {
                            log.push("内置 npm 管理的 Claude Code 已卸载。".to_string());
                        }
                    }
                    Err(error) => {
                        log.push(format!("内置 npm 卸载失败，将继续删除本地目录：{}", bounded_output(error)));
                    }
                }
            }
        }
    }

    match command_output_with_timeout("npm.cmd", &["uninstall", "-g", CLAUDE_PACKAGE], Duration::from_secs(120)) {
        Ok(output) => {
            if !output.trim().is_empty() {
                log.push(format!("系统 npm 卸载输出：\n{}", bounded_output(output)));
            }
        }
        Err(error) => {
            log.push(format!("系统 npm 卸载跳过或失败：{}", bounded_output(error)));
        }
    }

    for path in claude_removal_candidates() {
        if !path.exists() {
            continue;
        }

        match fs::remove_file(&path) {
            Ok(()) => log.push(format!("已删除 {}", path.display())),
            Err(error) => log.push(format!("删除 {} 失败：{}", path.display(), error)),
        }
    }

    for path in claude_package_dirs() {
        if !path.exists() {
            continue;
        }

        match fs::remove_dir_all(&path) {
            Ok(()) => log.push(format!("已删除 {}", path.display())),
            Err(error) => log.push(format!("删除 {} 失败：{}", path.display(), error)),
        }
    }

    if let Ok(base_dir) = managed_base_dir() {
        if base_dir.exists() {
            match fs::remove_dir_all(&base_dir) {
                Ok(()) => log.push(format!("已删除内置 Node 与软件运行目录 {}", base_dir.display())),
                Err(error) => log.push(format!("删除软件运行目录失败：{} ({})", base_dir.display(), error)),
            }
        }
    }

    refresh_process_path_from_registry();
    let check = check_claude();
    if check.installed {
        CommandResult {
            success: false,
            message: "一键卸载已执行，但仍检测到 Claude Code".to_string(),
            output: Some(format!("{}\n\n仍检测到：{}", log.join("\n\n"), check.message)),
        }
    } else {
        let message = if clear_result.success {
            "一键卸载完成"
        } else {
            "Claude Code 与内置 Node 已卸载，但部分 DeepSeek 配置清除失败"
        };

        CommandResult {
            success: clear_result.success,
            message: message.to_string(),
            output: Some(log.join("\n\n")).filter(|value| !value.trim().is_empty()),
        }
    }
}

fn ensure_windows() -> Result<(), CommandResult> {
    if cfg!(windows) {
        Ok(())
    } else {
        Err(CommandResult {
            success: false,
            message: "第一版仅支持 Windows".to_string(),
            output: None,
        })
    }
}

fn managed_base_dir() -> Result<PathBuf, String> {
    std::env::var("LOCALAPPDATA")
        .map(|value| PathBuf::from(value).join(MANAGED_DIR_NAME))
        .map_err(|_| "无法读取 LOCALAPPDATA，不能准备内置运行时".to_string())
}

fn managed_node_dir() -> Result<PathBuf, String> {
    Ok(managed_base_dir()?.join("runtime").join("node"))
}

fn managed_claude_prefix_dir() -> Result<PathBuf, String> {
    Ok(managed_base_dir()?.join("claude-code"))
}

fn bundled_node_dir(app: &tauri::AppHandle) -> Option<PathBuf> {
    let mut candidates = Vec::new();

    if let Ok(resource_dir) = app.path().resource_dir() {
        candidates.push(resource_dir.join("resources").join("node"));
        candidates.push(resource_dir.join("node"));
    }

    if let Ok(current_dir) = std::env::current_dir() {
        candidates.push(current_dir.join("src-tauri").join("resources").join("node"));
        candidates.push(current_dir.join("resources").join("node"));
    }

    candidates
        .into_iter()
        .find(|path| path.join("npm.cmd").exists() && path.join("node.exe").exists())
}

fn ensure_node_runtime(app: &tauri::AppHandle, log: &mut Vec<String>) -> Result<PathBuf, String> {
    let target = managed_node_dir()?;
    if target.join("npm.cmd").exists() && target.join("node.exe").exists() {
        return Ok(target);
    }

    let source = bundled_node_dir(app).ok_or_else(|| {
        "安装包中没有找到内置 Node runtime；请重新下载最新版安装包。".to_string()
    })?;

    if target.exists() {
        fs::remove_dir_all(&target).map_err(|error| {
            format!("清理旧版内置 Node runtime 失败：{} ({error})", target.display())
        })?;
    }

    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("创建运行时目录失败：{} ({error})", parent.display()))?;
    }

    copy_dir_all(&source, &target)?;
    log.push(format!("已启用内置 Node.js v{NODE_RUNTIME_VERSION}。"));
    Ok(target)
}

fn copy_dir_all(source: &PathBuf, target: &PathBuf) -> Result<(), String> {
    fs::create_dir_all(target)
        .map_err(|error| format!("创建目录失败：{} ({error})", target.display()))?;

    for entry in fs::read_dir(source)
        .map_err(|error| format!("读取目录失败：{} ({error})", source.display()))?
    {
        let entry = entry.map_err(|error| error.to_string())?;
        let file_type = entry.file_type().map_err(|error| error.to_string())?;
        let from = entry.path();
        let to = target.join(entry.file_name());

        if file_type.is_dir() {
            copy_dir_all(&from, &to)?;
        } else {
            fs::copy(&from, &to)
                .map_err(|error| format!("复制文件失败：{} -> {} ({error})", from.display(), to.display()))?;
        }
    }

    Ok(())
}

fn resolve_npm_cmd(app: &tauri::AppHandle, log: &mut Vec<String>) -> Result<(PathBuf, Option<PathBuf>), String> {
    match ensure_node_runtime(app, log) {
        Ok(node_dir) => {
            prepend_process_path(&[node_dir.clone()]);
            Ok((node_dir.join("npm.cmd"), Some(node_dir)))
        }
        Err(runtime_error) => match command_output("npm.cmd", &["--version"]) {
            Ok(version) => {
                log.push(format!(
                    "内置 Node runtime 不可用，临时使用系统 npm {version}。原因：{runtime_error}"
                ));
                Ok((PathBuf::from("npm.cmd"), None))
            }
            Err(npm_error) => Err(format!("{runtime_error}\n\n系统 npm 也不可用：{npm_error}")),
        },
    }
}

fn ensure_user_path_entry_first(path: &PathBuf) -> Result<(), String> {
    let entry = path.to_string_lossy().to_string();
    let current = read_user_env_var("Path").unwrap_or_default();
    let mut parts: Vec<String> = current
        .split(';')
        .filter_map(|part| {
            let trimmed = part.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        })
        .collect();

    if parts
        .first()
        .map_or(false, |part| same_path_text(part, &entry))
    {
        return Ok(());
    }

    parts.retain(|part| !same_path_text(part, &entry));
    parts.insert(0, entry);
    write_user_env_var("Path", &parts.join(";"))?;
    broadcast_environment_change();

    Ok(())
}

fn remove_user_path_entry(path: &PathBuf) -> Result<(), String> {
    let entry = path.to_string_lossy().to_string();
    let current = match read_user_env_var("Path") {
        Some(value) => value,
        None => return Ok(()),
    };

    let parts: Vec<String> = current
        .split(';')
        .filter_map(|part| {
            let trimmed = part.trim();
            if trimmed.is_empty() || same_path_text(trimmed, &entry) {
                None
            } else {
                Some(trimmed.to_string())
            }
        })
        .collect();

    write_user_env_var("Path", &parts.join(";"))?;
    broadcast_environment_change();
    Ok(())
}

fn same_path_text(left: &str, right: &str) -> bool {
    left.trim_end_matches(&['\\', '/'][..])
        .eq_ignore_ascii_case(right.trim_end_matches(&['\\', '/'][..]))
}

fn prepend_process_path(paths: &[PathBuf]) {
    let mut parts: Vec<String> = paths
        .iter()
        .map(|path| path.to_string_lossy().to_string())
        .collect();

    if let Ok(current) = std::env::var("PATH") {
        parts.push(current);
    }

    std::env::set_var("PATH", parts.join(";"));
}

#[cfg(windows)]
fn read_user_env_var(name: &str) -> Option<String> {
    use winreg::{enums::HKEY_CURRENT_USER, RegKey};

    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let env = hkcu.open_subkey("Environment").ok()?;
    env.get_value::<String, _>(name).ok()
}

#[cfg(windows)]
fn read_machine_env_var(name: &str) -> Option<String> {
    use winreg::{enums::HKEY_LOCAL_MACHINE, RegKey};

    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
    let env = hklm
        .open_subkey("SYSTEM\\CurrentControlSet\\Control\\Session Manager\\Environment")
        .ok()?;
    env.get_value::<String, _>(name).ok()
}

#[cfg(not(windows))]
fn read_machine_env_var(_name: &str) -> Option<String> {
    None
}

#[cfg(not(windows))]
fn read_user_env_var(_name: &str) -> Option<String> {
    None
}

#[cfg(windows)]
fn write_user_env_var(name: &str, value: &str) -> Result<(), String> {
    use winreg::{enums::HKEY_CURRENT_USER, RegKey};

    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let (env, _) = hkcu
        .create_subkey("Environment")
        .map_err(|error| error.to_string())?;
    env.set_value(name, &value)
        .map_err(|error| error.to_string())
}

#[cfg(not(windows))]
fn write_user_env_var(_name: &str, _value: &str) -> Result<(), String> {
    Err("第一版仅支持 Windows".to_string())
}

#[cfg(windows)]
fn delete_user_env_var(name: &str) -> Result<(), String> {
    use std::io::ErrorKind;
    use winreg::{
        enums::{HKEY_CURRENT_USER, KEY_READ, KEY_WRITE},
        RegKey,
    };

    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let env = match hkcu.open_subkey_with_flags("Environment", KEY_READ | KEY_WRITE) {
        Ok(env) => env,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.to_string()),
    };

    match env.delete_value(name) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

#[cfg(not(windows))]
fn delete_user_env_var(_name: &str) -> Result<(), String> {
    Err("第一版仅支持 Windows".to_string())
}

#[cfg(windows)]
fn broadcast_environment_change() {
    use windows_sys::Win32::Foundation::{LPARAM, WPARAM};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        SendMessageTimeoutW, HWND_BROADCAST, SMTO_ABORTIFHUNG, WM_SETTINGCHANGE,
    };

    let target: Vec<u16> = "Environment".encode_utf16().chain(Some(0)).collect();
    unsafe {
        SendMessageTimeoutW(
            HWND_BROADCAST,
            WM_SETTINGCHANGE,
            WPARAM::default(),
            target.as_ptr() as LPARAM,
            SMTO_ABORTIFHUNG,
            5000,
            std::ptr::null_mut(),
        );
    }
}

#[cfg(not(windows))]
fn broadcast_environment_change() {}

#[cfg(windows)]
fn refresh_process_path_from_registry() {
    let mut paths = Vec::new();

    if let Ok(current) = std::env::var("PATH") {
        paths.push(current);
    }

    if let Some(machine_path) = read_machine_env_var("Path") {
        paths.push(machine_path);
    }

    if let Some(user_path) = read_user_env_var("Path") {
        paths.push(user_path);
    }

    if !paths.is_empty() {
        std::env::set_var("PATH", paths.join(";"));
    }
}

#[cfg(not(windows))]
fn refresh_process_path_from_registry() {}

#[cfg(windows)]
fn claude_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();

    if let Ok(prefix) = managed_claude_prefix_dir() {
        candidates.push(prefix.join("claude.cmd"));
        candidates.push(prefix.join("claude.exe"));
    }

    if let Ok(appdata) = std::env::var("APPDATA") {
        let npm_dir = PathBuf::from(appdata).join("npm");
        candidates.push(npm_dir.join("claude.cmd"));
        candidates.push(npm_dir.join("claude.exe"));
    }

    if let Ok(userprofile) = std::env::var("USERPROFILE") {
        candidates.push(PathBuf::from(userprofile).join(".local").join("bin").join("claude.exe"));
    }

    if let Ok(localappdata) = std::env::var("LOCALAPPDATA") {
        candidates.push(PathBuf::from(&localappdata).join("Programs").join("Claude").join("claude.exe"));
        candidates.push(PathBuf::from(localappdata).join("Claude").join("claude.exe"));
    }

    candidates
}

#[cfg(not(windows))]
fn claude_candidates() -> Vec<PathBuf> {
    Vec::new()
}

#[cfg(windows)]
fn claude_removal_candidates() -> Vec<PathBuf> {
    let mut candidates = claude_candidates();

    if let Ok(appdata) = std::env::var("APPDATA") {
        candidates.push(PathBuf::from(appdata).join("npm").join("claude.ps1"));
    }

    candidates
}

#[cfg(not(windows))]
fn claude_removal_candidates() -> Vec<PathBuf> {
    Vec::new()
}

#[cfg(windows)]
fn claude_package_dirs() -> Vec<PathBuf> {
    let mut candidates = Vec::new();

    if let Ok(prefix) = managed_claude_prefix_dir() {
        candidates.push(
            prefix
                .join("node_modules")
                .join("@anthropic-ai")
                .join("claude-code"),
        );
    }

    if let Ok(appdata) = std::env::var("APPDATA") {
        candidates.push(
            PathBuf::from(appdata)
                .join("npm")
                .join("node_modules")
                .join("@anthropic-ai")
                .join("claude-code"),
        );
    }

    candidates
}

#[cfg(not(windows))]
fn claude_package_dirs() -> Vec<PathBuf> {
    Vec::new()
}

fn remove_incompatible_claude_binaries(log: &mut Vec<String>) {
    for path in claude_removal_candidates() {
        if !path.exists() {
            continue;
        }

        if path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("ps1"))
        {
            match fs::remove_file(&path) {
                Ok(()) => log.push(format!("已删除旧 Claude Code PowerShell 启动脚本：{}", path.display())),
                Err(error) => log.push(format!(
                    "删除旧 Claude Code PowerShell 启动脚本失败：{} ({})",
                    path.display(),
                    error
                )),
            }
            continue;
        }

        match Command::new(&path).arg("--version").output() {
            Ok(output) if output.status.success() => {
                let version = command_text_from_output(&output);
                if is_compatible_claude_version(&version) {
                    continue;
                }

                match fs::remove_file(&path) {
                    Ok(()) => log.push(format!(
                        "已删除不兼容的 Claude Code：{} ({})",
                        path.display(),
                        version
                    )),
                    Err(error) => log.push(format!(
                        "删除不兼容的 Claude Code 失败：{} ({})",
                        path.display(),
                        error
                    )),
                }
            }
            Ok(_) => {}
            Err(error) => log.push(format!("检查 {} 版本失败：{}", path.display(), error)),
        }
    }
}

fn command_output_from_candidates(paths: &[PathBuf], args: &[&str]) -> Result<String, String> {
    let mut checked = Vec::new();
    let mut first_success = None;

    for path in paths {
        if !path.exists() {
            checked.push(format!("未找到 {}", path.display()));
            continue;
        }

        match Command::new(path).args(args).output() {
            Ok(output) => {
                let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
                let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
                let combined = if stderr.is_empty() {
                    stdout
                } else if stdout.is_empty() {
                    stderr
                } else {
                    format!("{stdout}\n{stderr}")
                };

                if output.status.success() {
                    if is_compatible_claude_version(&combined) {
                        return Ok(combined);
                    }

                    if first_success.is_none() {
                        first_success = Some(combined.clone());
                    }

                    checked.push(format!("{}: 版本不兼容 {}", path.display(), combined));
                    continue;
                }

                checked.push(format!("{}: {}", path.display(), combined));
            }
            Err(error) => checked.push(format!("{}: {}", path.display(), error)),
        }
    }

    if let Some(output) = first_success {
        return Ok(output);
    }

    Err(if checked.is_empty() {
        "未找到 Claude Code 常见安装路径".to_string()
    } else {
        checked.join("\n")
    })
}

fn main() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            check_environment,
            configure_deepseek,
            update_api_key,
            one_click_setup,
            verify_claude,
            one_click_uninstall
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
