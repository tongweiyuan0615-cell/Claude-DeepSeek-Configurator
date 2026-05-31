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
const CLAUDE_LATEST_DIR_NAME: &str = "claude-code-latest";

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
    claude_path: ToolCheck,
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
            let channel = first_claude_on_windows_path()
                .and_then(|path| managed_claude_channel_for_path(&path));
            claude_tool_check_for_channel(version, channel)
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
    claude_tool_check_for_channel(version, None)
}

fn claude_tool_check_for_channel(version: String, channel: Option<&'static str>) -> ToolCheck {
    let compatible = is_compatible_claude_version(&version);
    let managed_latest = channel == Some("latest");

    ToolCheck {
        installed: true,
        version: Some(version.clone()),
        meets_requirement: Some(compatible || managed_latest),
        message: if managed_latest && !compatible {
            format!("已启用实验最新版 {version}；如 DeepSeek 不兼容，请点击回退稳定版")
        } else if managed_latest {
            format!("已启用实验最新版 {version}")
        } else if compatible {
            format!("已安装稳定兼容版 {version}")
        } else {
            format!("已安装 {version}，但 DeepSeek 当前兼容版本需要 {CLAUDE_COMPAT_VERSION}")
        },
    }
}

fn check_managed_claude() -> ToolCheck {
    let path_entries = managed_tool_path_entries();
    prepend_process_path(&path_entries);

    match command_output_from_candidates(&managed_claude_candidates(), &["--version"]) {
        Ok(version) => claude_tool_check(version),
        Err(error) => ToolCheck {
            installed: false,
            version: None,
            meets_requirement: Some(false),
            message: error,
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
        claude_path: check_claude_path_priority(),
        deepseek_configured,
        missing_env_vars,
    }
}

#[tauri::command]
fn generate_diagnostic_report(app: tauri::AppHandle) -> CommandResult {
    let status = check_environment(app);
    let mut lines = Vec::new();

    lines.push("Claude Code + DeepSeek Configurator diagnostic report".to_string());
    lines.push("Platform: Windows".to_string());
    lines.push(format!("App version: {}", env!("CARGO_PKG_VERSION")));
    lines.push(format!("Claude compatible version: {CLAUDE_COMPAT_VERSION}"));
    lines.push(format!("Bundled Node.js version: v{NODE_RUNTIME_VERSION}"));
    lines.push(String::new());

    lines.push("Managed paths:".to_string());
    add_path_report(&mut lines, "Managed base", managed_base_dir());
    add_path_report(&mut lines, "Managed Node", managed_node_dir());
    add_path_report(&mut lines, "Managed stable Claude prefix", managed_claude_prefix_dir());
    add_path_report(
        &mut lines,
        "Managed latest Claude prefix",
        managed_claude_latest_prefix_dir(),
    );
    lines.push(String::new());

    lines.push("Tool checks:".to_string());
    lines.push(format_tool_check("Node.js", &status.node));
    lines.push(format_tool_check("npm", &status.npm));
    lines.push(format_tool_check("Claude Code", &status.claude));
    lines.push(format_tool_check("Claude PATH priority", &status.claude_path));
    lines.push(String::new());

    lines.push("DeepSeek environment:".to_string());
    lines.push(format!(
        "{AUTH_TOKEN_ENV_VAR}: {}",
        if read_user_env_var(AUTH_TOKEN_ENV_VAR)
            .map(|value| !value.trim().is_empty())
            .unwrap_or(false)
        {
            "configured (redacted)"
        } else {
            "missing"
        }
    ));
    lines.push(format_env_state(CLAUDE_AUTOUPDATER_ENV_VAR, "1"));
    for (name, expected) in DEEPSEEK_ENV_VARS {
        lines.push(format_env_state(name, expected));
    }
    lines.push(format!(
        "Overall DeepSeek status: {}",
        if status.deepseek_configured {
            "configured"
        } else {
            "incomplete"
        }
    ));
    lines.push(format!(
        "Missing variables: {}",
        if status.missing_env_vars.is_empty() {
            "none".to_string()
        } else {
            status.missing_env_vars.join(", ")
        }
    ));
    lines.push(String::new());
    lines.push("Secret policy: API Key values are never included in this report.".to_string());

    CommandResult {
        success: true,
        message: "诊断报告已生成，API Key 已自动脱敏。".to_string(),
        output: Some(redact_sensitive_text(&lines.join("\n"))),
    }
}

fn add_path_report(lines: &mut Vec<String>, label: &str, path: Result<PathBuf, String>) {
    match path {
        Ok(path) => lines.push(format!("{label}: {} (exists: {})", path.display(), path.exists())),
        Err(error) => lines.push(format!("{label}: unavailable ({error})")),
    }
}

fn format_tool_check(label: &str, check: &ToolCheck) -> String {
    format!(
        "{label}: installed={}, version={}, meets_requirement={}, message={}",
        check.installed,
        check.version.as_deref().unwrap_or("unknown"),
        match check.meets_requirement {
            Some(true) => "true",
            Some(false) => "false",
            None => "unknown",
        },
        redact_sensitive_text(&check.message)
    )
}

fn format_env_state(name: &str, expected: &str) -> String {
    let state = match read_user_env_var(name) {
        Some(value) if value == expected => "ok",
        Some(_) => "configured but unexpected",
        None => "missing",
    };
    format!("{name}: {state}")
}

fn redact_sensitive_text(input: &str) -> String {
    let mut output = input.to_string();
    if let Some(token) = read_user_env_var(AUTH_TOKEN_ENV_VAR) {
        let token = token.trim();
        if !token.is_empty() {
            output = output.replace(token, "[REDACTED]");
        }
    }

    redact_sk_tokens(&output)
}

fn redact_sk_tokens(input: &str) -> String {
    let chars: Vec<char> = input.chars().collect();
    let mut output = String::new();
    let mut index = 0;

    while index < chars.len() {
        if index + 3 <= chars.len() && chars[index] == 's' && chars[index + 1] == 'k' && chars[index + 2] == '-' {
            output.push_str("[REDACTED]");
            index += 3;
            while index < chars.len()
                && (chars[index].is_ascii_alphanumeric()
                    || matches!(chars[index], '-' | '_' | '.'))
            {
                index += 1;
            }
            continue;
        }

        output.push(chars[index]);
        index += 1;
    }

    output
}

fn install_claude_native(app: &tauri::AppHandle) -> CommandResult {
    install_claude_channel(
        app,
        managed_claude_prefix_dir(),
        format!("{CLAUDE_PACKAGE}@{CLAUDE_COMPAT_VERSION}"),
        format!("Claude Code {CLAUDE_COMPAT_VERSION} 稳定版"),
        true,
        true,
    )
}

fn install_claude_channel(
    app: &tauri::AppHandle,
    claude_prefix: Result<PathBuf, String>,
    package_spec: String,
    channel_label: String,
    require_compatible: bool,
    cleanup_incompatible: bool,
) -> CommandResult {
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

    let claude_prefix = match claude_prefix {
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

            if cleanup_incompatible {
                remove_incompatible_claude_binaries(&mut log);
            }
            remove_managed_powershell_launcher(&claude_prefix, &mut log);
            refresh_process_path_from_registry();
            prepend_process_path(&[claude_prefix.clone()]);
            match command_output_from_candidates(&claude_candidates_for_prefix(&claude_prefix), &["--version"]) {
                Ok(version) if !require_compatible || is_compatible_claude_version(&version) => {
                    log.push(format!("{channel_label} 当前版本：{version}"));
                    CommandResult {
                        success: true,
                        message: format!("{channel_label} 安装完成"),
                        output: Some(log.join("\n\n")).filter(|value| !value.trim().is_empty()),
                    }
                }
                Ok(version) => CommandResult {
                    success: false,
                    message: "Claude Code 安装后版本验证失败".to_string(),
                    output: Some(format!(
                        "{}\n\n检测到版本：{}\n需要稳定兼容版本：{}",
                        log.join("\n\n"),
                        version,
                        CLAUDE_COMPAT_VERSION
                    )),
                },
                Err(error) => CommandResult {
                    success: false,
                    message: "Claude Code 安装后验证失败".to_string(),
                    output: Some(format!("{}\n\n{}", log.join("\n\n"), error)),
                },
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
async fn install_latest_claude(app: tauri::AppHandle) -> CommandResult {
    run_blocking(move || install_latest_claude_internal(&app)).await
}

fn install_latest_claude_internal(app: &tauri::AppHandle) -> CommandResult {
    let mut result = install_claude_channel(
        app,
        managed_claude_latest_prefix_dir(),
        format!("{CLAUDE_PACKAGE}@latest"),
        "Claude Code 最新版".to_string(),
        false,
        false,
    );

    if result.success {
        result.message = "最新版 Claude Code 已启用。若 DeepSeek 不兼容，可点击“回退稳定版”。".to_string();
        let note = "注意：最新版属于实验通道，只切换本软件管理的 Claude 路径，不会删除用户自己安装的 Claude。";
        result.output = Some(match result.output {
            Some(output) if !output.trim().is_empty() => format!("{output}\n\n{note}"),
            _ => note.to_string(),
        });
    }

    result
}

#[tauri::command]
async fn rollback_stable_claude(app: tauri::AppHandle) -> CommandResult {
    run_blocking(move || rollback_stable_claude_internal(&app)).await
}

fn rollback_stable_claude_internal(app: &tauri::AppHandle) -> CommandResult {
    let mut result = install_claude_native(app);

    if result.success {
        result.message = format!("已回退到稳定版 Claude Code {CLAUDE_COMPAT_VERSION}。请重新打开 PowerShell / CMD / VS Code 终端。");
        let note = "已将稳定版路径重新放到用户 PATH 前面；最新版目录会保留，方便以后再次尝试。";
        result.output = Some(match result.output {
            Some(output) if !output.trim().is_empty() => format!("{output}\n\n{note}"),
            _ => note.to_string(),
        });
    }

    result
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

    let managed_claude_check = check_managed_claude();
    let claude_check = check_claude();
    if !managed_claude_check.installed || managed_claude_check.meets_requirement == Some(false) {
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
            "本软件管理的 Claude Code 已安装且版本兼容（{CLAUDE_COMPAT_VERSION}），跳过安装步骤"
        ));
        remove_incompatible_claude_binaries(&mut log);
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
        log.push(ensure_managed_claude_priority());

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
            let channel = first_claude_on_windows_path()
                .and_then(|path| managed_claude_channel_for_path(&path));
            let result = verify_claude_version_result_for_channel(output, channel);
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
    verify_claude_version_result_for_channel(output, None)
}

fn verify_claude_version_result_for_channel(
    output: String,
    channel: Option<&'static str>,
) -> CommandResult {
    if is_compatible_claude_version(&output) {
        CommandResult {
            success: true,
            message: "Claude Code 可执行且稳定版兼容".to_string(),
            output: Some(output),
        }
    } else if channel == Some("latest") {
        CommandResult {
            success: true,
            message: "Claude Code 最新版可执行；这是实验通道，DeepSeek 兼容性以实际对话为准".to_string(),
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

    if let Ok(latest_prefix) = managed_claude_latest_prefix_dir() {
        let _ = remove_user_path_entry(&latest_prefix);
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

fn managed_claude_latest_prefix_dir() -> Result<PathBuf, String> {
    Ok(managed_base_dir()?.join(CLAUDE_LATEST_DIR_NAME))
}

fn managed_tool_path_entries() -> Vec<PathBuf> {
    let mut entries = Vec::new();
    if let Ok(claude_prefix) = managed_claude_prefix_dir() {
        entries.push(claude_prefix);
    }
    if let Ok(node_dir) = managed_node_dir() {
        entries.push(node_dir);
    }
    entries
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

fn check_claude_path_priority() -> ToolCheck {
    let Some(path) = first_claude_on_windows_path() else {
        return ToolCheck {
            installed: false,
            version: None,
            meets_requirement: Some(false),
            message: "当前终端 PATH 还没有命中 claude；一键部署会自动添加本软件管理路径。".to_string(),
        };
    };

    let managed_channel = managed_claude_channel_for_path(&path);
    let managed = managed_channel.is_some();
    ToolCheck {
        installed: true,
        version: None,
        meets_requirement: Some(managed),
        message: if managed {
            let channel = managed_channel.unwrap_or("unknown");
            format!("当前终端会优先使用本软件管理的 {channel} Claude：{}", path.display())
        } else {
            format!(
                "当前终端优先命中其他 Claude：{}；一键部署会尝试把本软件管理路径放到最前面。",
                path.display()
            )
        },
    }
}

fn ensure_managed_claude_priority() -> String {
    let mut log = Vec::new();

    for path in managed_tool_path_entries() {
        if let Err(error) = ensure_user_path_entry_first(&path) {
            log.push(format!("修复 PATH 优先级失败：{} ({error})", path.display()));
        }
    }

    refresh_process_path_from_registry();
    prepend_process_path(&managed_tool_path_entries());

    let priority = check_claude_path_priority();
    if priority.meets_requirement == Some(true) {
        format!("Claude 命令优先级已确认：{}", priority.message)
    } else {
        format!(
            "Claude 命令优先级提醒：{} 如果客户电脑存在系统级 Claude 路径，可能需要管理员手动移除旧路径。",
            priority.message
        )
    }
}

fn first_claude_on_windows_path() -> Option<PathBuf> {
    for dir in windows_terminal_path_entries() {
        for name in ["claude.cmd", "claude.exe", "claude.bat", "claude.ps1"] {
            let candidate = dir.join(name);
            if candidate.exists() {
                return Some(candidate);
            }
        }
    }

    None
}

fn windows_terminal_path_entries() -> Vec<PathBuf> {
    let mut entries = Vec::new();

    if let Some(machine_path) = read_machine_env_var("Path") {
        entries.extend(split_windows_path(&machine_path));
    }

    if let Some(user_path) = read_user_env_var("Path") {
        entries.extend(split_windows_path(&user_path));
    }

    if entries.is_empty() {
        if let Ok(process_path) = std::env::var("PATH") {
            entries.extend(split_windows_path(&process_path));
        }
    }

    entries
}

fn split_windows_path(value: &str) -> Vec<PathBuf> {
    value
        .split(';')
        .filter_map(|part| {
            let trimmed = part.trim().trim_matches('"');
            if trimmed.is_empty() {
                None
            } else {
                Some(PathBuf::from(expand_windows_env_vars(trimmed)))
            }
        })
        .collect()
}

fn expand_windows_env_vars(value: &str) -> String {
    let mut output = String::new();
    let chars: Vec<char> = value.chars().collect();
    let mut index = 0;

    while index < chars.len() {
        if chars[index] == '%' {
            if let Some(end) = chars[index + 1..].iter().position(|ch| *ch == '%') {
                let name: String = chars[index + 1..index + 1 + end].iter().collect();
                if let Ok(env_value) = std::env::var(&name) {
                    output.push_str(&env_value);
                    index += end + 2;
                    continue;
                }
            }
        }

        output.push(chars[index]);
        index += 1;
    }

    output
}

fn is_managed_claude_path(path: &PathBuf) -> bool {
    managed_claude_channel_for_path(path).is_some()
}

fn managed_claude_channel_for_path(path: &PathBuf) -> Option<&'static str> {
    let Ok(prefix) = managed_claude_prefix_dir() else {
        return None;
    };

    let path_text = normalize_path_text(&path.to_string_lossy());
    let prefix_text = normalize_path_text(&prefix.to_string_lossy());
    if path_text == prefix_text || path_text.starts_with(&(prefix_text + "\\")) {
        return Some("stable");
    }

    let Ok(latest_prefix) = managed_claude_latest_prefix_dir() else {
        return None;
    };
    let latest_prefix_text = normalize_path_text(&latest_prefix.to_string_lossy());
    if path_text == latest_prefix_text || path_text.starts_with(&(latest_prefix_text + "\\")) {
        return Some("latest");
    }

    None
}

fn normalize_path_text(value: &str) -> String {
    value
        .trim()
        .trim_matches('"')
        .trim_end_matches(&['\\', '/'][..])
        .replace('/', "\\")
        .to_ascii_lowercase()
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
fn managed_claude_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();

    if let Ok(prefix) = managed_claude_prefix_dir() {
        candidates.extend(claude_candidates_for_prefix(&prefix));
    }

    candidates
}

#[cfg(not(windows))]
fn managed_claude_candidates() -> Vec<PathBuf> {
    Vec::new()
}

#[cfg(windows)]
fn claude_candidates() -> Vec<PathBuf> {
    let mut candidates = managed_claude_candidates();

    if let Ok(prefix) = managed_claude_latest_prefix_dir() {
        candidates.extend(claude_candidates_for_prefix(&prefix));
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
    let mut candidates = managed_claude_candidates();

    if let Ok(prefix) = managed_claude_prefix_dir() {
        candidates.push(prefix.join("claude.ps1"));
    }

    if let Ok(appdata) = std::env::var("APPDATA") {
        let npm_dir = PathBuf::from(&appdata).join("npm");
        candidates.push(npm_dir.join("claude.cmd"));
        candidates.push(npm_dir.join("claude.exe"));
        candidates.push(PathBuf::from(appdata).join("npm").join("claude.ps1"));
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

fn claude_candidates_for_prefix(prefix: &PathBuf) -> Vec<PathBuf> {
    vec![prefix.join("claude.cmd"), prefix.join("claude.exe")]
}

fn remove_managed_powershell_launcher(prefix: &PathBuf, log: &mut Vec<String>) {
    let path = prefix.join("claude.ps1");
    if !path.exists() {
        return;
    }

    match fs::remove_file(&path) {
        Ok(()) => log.push(format!(
            "已删除本软件管理目录中的 PowerShell 启动脚本，避免执行策略阻止：{}",
            path.display()
        )),
        Err(error) => log.push(format!(
            "删除本软件管理目录中的 PowerShell 启动脚本失败：{} ({})",
            path.display(),
            error
        )),
    }
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
            generate_diagnostic_report,
            configure_deepseek,
            update_api_key,
            one_click_setup,
            install_latest_claude,
            rollback_stable_claude,
            verify_claude,
            one_click_uninstall
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
