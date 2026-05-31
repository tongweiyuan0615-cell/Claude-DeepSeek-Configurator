use serde::Serialize;
use std::collections::HashMap;
use std::fs;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::{Path, PathBuf};
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
const ENV_FILE_NAME: &str = ".claude-deepseek-env";
const PROFILE_START: &str = "# >>> Claude Code + DeepSeek Configurator >>>";
const PROFILE_END: &str = "# <<< Claude Code + DeepSeek Configurator <<<";

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
        let node = node_dir.join("bin").join("node");
        if node.exists() {
            return match command_output(&node.to_string_lossy(), &["--version"]) {
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
            message: format!("已随 macOS 安装包内置 v{NODE_RUNTIME_VERSION}，部署时自动启用"),
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
        let npm = node_dir.join("bin").join("npm");
        if npm.exists() && npm_cli_path(&node_dir).exists() {
            let _ = repair_node_npm_launchers(&node_dir);
            return match command_output(&npm.to_string_lossy(), &["--version"]) {
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
            message: "已随 macOS 安装包内置，部署时自动启用".to_string(),
        };
    }

    match command_output("npm", &["--version"]) {
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
    let path_entries = managed_tool_path_entries();
    prepend_process_path(&path_entries);

    match command_output_from_candidates(&claude_candidates(), &["--version"]) {
        Ok(version) => claude_tool_check(version),
        Err(candidate_error) => match command_output("/bin/zsh", &["-lc", "claude --version"]) {
            Ok(version) => claude_tool_check(version),
            Err(path_error) => ToolCheck {
                installed: false,
                version: None,
                meets_requirement: Some(false),
                message: format!("{candidate_error}\n{path_error}"),
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
    let vars = read_macos_env_file();
    let mut missing = Vec::new();

    for (name, expected) in DEEPSEEK_ENV_VARS {
        match vars.get(*name) {
            Some(value) if value == expected => {}
            _ => missing.push((*name).to_string()),
        }
    }

    match vars.get(AUTH_TOKEN_ENV_VAR) {
        Some(value) if !value.trim().is_empty() => {}
        _ => missing.push(AUTH_TOKEN_ENV_VAR.to_string()),
    }

    match vars.get(CLAUDE_AUTOUPDATER_ENV_VAR) {
        Some(value) if value == "1" => {}
        _ => missing.push(CLAUDE_AUTOUPDATER_ENV_VAR.to_string()),
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
    let vars = read_macos_env_file();
    let mut lines = Vec::new();

    lines.push("Claude Code + DeepSeek Configurator diagnostic report".to_string());
    lines.push("Platform: macOS".to_string());
    lines.push(format!("App version: {}", env!("CARGO_PKG_VERSION")));
    lines.push(format!("Claude compatible version: {CLAUDE_COMPAT_VERSION}"));
    lines.push(format!("Bundled Node.js version: v{NODE_RUNTIME_VERSION}"));
    lines.push(String::new());

    lines.push("Managed paths:".to_string());
    add_path_report(&mut lines, "Managed base", managed_base_dir());
    add_path_report(&mut lines, "Managed Node", managed_node_dir());
    add_path_report(&mut lines, "Managed Claude prefix", managed_claude_prefix_dir());
    add_path_report(&mut lines, "Managed env file", env_file_path());
    lines.push(String::new());

    lines.push("Shell profile links:".to_string());
    match shell_profile_paths() {
        Ok(profiles) => {
            for profile in profiles {
                let contains_block = fs::read_to_string(&profile)
                    .map(|content| content.contains(PROFILE_START) && content.contains(PROFILE_END))
                    .unwrap_or(false);
                lines.push(format!(
                    "{}: exists={}, managed_block={}",
                    profile.display(),
                    profile.exists(),
                    contains_block
                ));
            }
        }
        Err(error) => lines.push(format!("unavailable ({error})")),
    }
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
        if vars
            .get(AUTH_TOKEN_ENV_VAR)
            .map(|value| !value.trim().is_empty())
            .unwrap_or(false)
        {
            "configured (redacted)"
        } else {
            "missing"
        }
    ));
    lines.push(format_macos_env_state(&vars, CLAUDE_AUTOUPDATER_ENV_VAR, "1"));
    for (name, expected) in DEEPSEEK_ENV_VARS {
        lines.push(format_macos_env_state(&vars, name, expected));
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

fn format_macos_env_state(vars: &HashMap<String, String>, name: &str, expected: &str) -> String {
    let state = match vars.get(name) {
        Some(value) if value == expected => "ok",
        Some(_) => "configured but unexpected",
        None => "missing",
    };
    format!("{name}: {state}")
}

fn redact_sensitive_text(input: &str) -> String {
    let mut output = input.to_string();
    let vars = read_macos_env_file();
    if let Some(token) = vars.get(AUTH_TOKEN_ENV_VAR) {
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
    if let Err(error) = ensure_macos() {
        return error;
    }

    let mut log = Vec::new();
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

    let mut path_entries = vec![managed_claude_bin_dir()];
    if !node_dir.as_os_str().is_empty() {
        path_entries.push(node_dir.join("bin"));
    }
    prepend_process_path(&path_entries);

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

            let check = check_managed_claude();
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
            log.push(format!("npm 安装失败：{}", bounded_output(error)));
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
        result.message = "API Key 已更新。请重新打开 Terminal / iTerm / VS Code 终端。".to_string();
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

    if let Err(error) = ensure_macos() {
        return error;
    }

    if let Err(error) = write_macos_env_file(api_key.trim()) {
        return CommandResult {
            success: false,
            message: "写入 macOS 配置文件失败".to_string(),
            output: Some(error),
        };
    }

    if let Err(error) = install_shell_profile_link() {
        return CommandResult {
            success: false,
            message: "写入终端启动配置失败".to_string(),
            output: Some(error),
        };
    }

    apply_process_env(api_key.trim());

    CommandResult {
        success: true,
        message: "配置完成。请重新打开 Terminal / iTerm / VS Code 终端。".to_string(),
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
    }

    let configure_result = configure_deepseek_internal(api_key);
    log.push(configure_result.message.clone());

    if !configure_result.success {
        if let Some(output) = configure_result.output {
            log.push(output);
        }

        return CommandResult {
            success: false,
            message: "一键部署失败：DeepSeek 配置写入未完成".to_string(),
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
            message: "一键部署完成。请重新打开 Terminal / iTerm / VS Code 终端。".to_string(),
            output: Some(log.join("\n\n")),
        }
    } else {
        CommandResult {
            success: false,
            message: "配置已写入，但 Claude Code 验证失败".to_string(),
            output: Some(log.join("\n\n")),
        }
    }
}

#[tauri::command]
fn verify_claude() -> CommandResult {
    let path_entries = managed_tool_path_entries();
    prepend_process_path(&path_entries);

    match command_output_from_candidates(&claude_candidates(), &["--version"]) {
        Ok(output) => verify_claude_version_result(output),
        Err(candidate_error) => match command_output("/bin/zsh", &["-lc", "claude --version"]) {
            Ok(output) => verify_claude_version_result(output),
            Err(path_error) => CommandResult {
                success: false,
                message: "Claude Code 验证失败".to_string(),
                output: Some(format!("{candidate_error}\n{path_error}")),
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
    if let Err(error) = ensure_macos() {
        return error;
    }

    let mut errors = Vec::new();
    if let Ok(env_file) = env_file_path() {
        if env_file.exists() {
            if let Err(error) = fs::remove_file(&env_file) {
                errors.push(format!("删除 {} 失败：{}", env_file.display(), error));
            }
        }
    }

    if let Err(error) = remove_shell_profile_link() {
        errors.push(error);
    }

    std::env::remove_var(CLAUDE_AUTOUPDATER_ENV_VAR);
    std::env::remove_var(AUTH_TOKEN_ENV_VAR);
    for (name, _) in DEEPSEEK_ENV_VARS {
        std::env::remove_var(name);
    }

    if errors.is_empty() {
        CommandResult {
            success: true,
            message: "DeepSeek macOS 配置已清除。请重新打开 Terminal / iTerm / VS Code 终端。".to_string(),
            output: None,
        }
    } else {
        CommandResult {
            success: false,
            message: "部分 macOS 配置清除失败".to_string(),
            output: Some(errors.join("\n")),
        }
    }
}

#[tauri::command]
async fn one_click_uninstall() -> CommandResult {
    run_blocking(one_click_uninstall_internal).await
}

fn one_click_uninstall_internal() -> CommandResult {
    if let Err(error) = ensure_macos() {
        return error;
    }

    let mut log = Vec::new();
    let mut errors = Vec::new();
    let clear_result = clear_deepseek_config_internal();
    log.push(clear_result.message.clone());
    if let Some(output) = clear_result.output {
        log.push(output);
    }

    if let Ok(base_dir) = managed_base_dir() {
        if base_dir.exists() {
            match fs::remove_dir_all(&base_dir) {
                Ok(()) => log.push(format!("已删除本软件安装的内置 Node 与 Claude Code：{}", base_dir.display())),
                Err(error) => {
                    let message = format!("删除软件运行目录失败：{} ({})", base_dir.display(), error);
                    errors.push(message.clone());
                    log.push(message);
                }
            }
        } else {
            log.push("本软件安装目录不存在，无需重复删除。".to_string());
        }
    }

    remove_process_path_entries(&managed_tool_path_entries());

    match first_claude_from_login_shell() {
        Some(path) if is_managed_claude_path(&path) => {
            let message = format!("新终端仍会命中本软件 Claude 路径：{}。请检查 shell 配置是否还有旧 PATH。", path.display());
            errors.push(message.clone());
            log.push(message);
        }
        Some(path) => {
            log.push(format!(
                "提示：新终端仍检测到其他 Claude Code：{}。这通常是用户自己安装的外部版本，本软件不会删除它。",
                path.display()
            ));
        }
        None => log.push("新终端已不再检测到本软件安装的 Claude Code。".to_string()),
    }

    log.push("如果已经打开的 Terminal / iTerm / VS Code 终端里 claude 仍能运行，请完全关闭后重新打开；旧终端会保留卸载前的 PATH 或命令缓存。".to_string());

    let success = clear_result.success && errors.is_empty();
    CommandResult {
        success,
        message: if success {
            "一键卸载完成"
        } else {
            "一键卸载未完全完成"
        }
        .to_string(),
        output: Some(log.join("\n\n")).filter(|value| !value.trim().is_empty()),
    }
}

fn ensure_macos() -> Result<(), CommandResult> {
    if cfg!(target_os = "macos") {
        Ok(())
    } else {
        Err(CommandResult {
            success: false,
            message: "此版本仅支持 macOS".to_string(),
            output: None,
        })
    }
}

fn home_dir() -> Result<PathBuf, String> {
    std::env::var("HOME")
        .map(PathBuf::from)
        .map_err(|_| "无法读取 HOME，不能准备 macOS 配置".to_string())
}

fn managed_base_dir() -> Result<PathBuf, String> {
    Ok(home_dir()?
        .join("Library")
        .join("Application Support")
        .join(MANAGED_DIR_NAME))
}

fn managed_node_dir() -> Result<PathBuf, String> {
    Ok(managed_base_dir()?.join("runtime").join("node"))
}

fn managed_claude_prefix_dir() -> Result<PathBuf, String> {
    Ok(managed_base_dir()?.join("claude-code"))
}

fn managed_claude_bin_dir() -> PathBuf {
    managed_claude_prefix_dir()
        .unwrap_or_else(|_| PathBuf::from("/tmp/ClaudeDeepSeekConfigurator/claude-code"))
        .join("bin")
}

fn managed_tool_path_entries() -> Vec<PathBuf> {
    let mut entries = vec![managed_claude_bin_dir()];
    if let Ok(node_dir) = managed_node_dir() {
        entries.push(node_dir.join("bin"));
    }
    entries
}

fn env_file_path() -> Result<PathBuf, String> {
    Ok(home_dir()?.join(ENV_FILE_NAME))
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
        .find(|path| path.join("bin").join("node").exists() && npm_cli_path(path).exists())
}

fn ensure_node_runtime(app: &tauri::AppHandle, log: &mut Vec<String>) -> Result<PathBuf, String> {
    let target = managed_node_dir()?;
    if target.join("bin").join("node").exists() && npm_cli_path(&target).exists() {
        make_node_runtime_executable(&target)?;
        repair_node_npm_launchers(&target)?;
        return Ok(target);
    }

    let source = bundled_node_dir(app).ok_or_else(|| {
        "安装包中没有找到内置 Node runtime；请重新下载最新版 macOS 安装包。".to_string()
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
    make_node_runtime_executable(&target)?;
    repair_node_npm_launchers(&target)?;
    log.push(format!("已启用内置 Node.js v{NODE_RUNTIME_VERSION}。"));
    Ok(target)
}

fn copy_dir_all(source: &Path, target: &Path) -> Result<(), String> {
    fs::create_dir_all(target)
        .map_err(|error| format!("创建目录失败：{} ({error})", target.display()))?;

    for entry in fs::read_dir(source)
        .map_err(|error| format!("读取目录失败：{} ({error})", source.display()))?
    {
        let entry = entry.map_err(|error| error.to_string())?;
        let file_type = entry.file_type().map_err(|error| error.to_string())?;
        let from = entry.path();
        let to = target.join(entry.file_name());

        if file_type.is_symlink() {
            let link_target = fs::read_link(&from)
                .map_err(|error| format!("读取符号链接失败：{} ({error})", from.display()))?;
            symlink(&link_target, &to)
                .map_err(|error| format!("复制符号链接失败：{} -> {} ({error})", from.display(), to.display()))?;
        } else if file_type.is_dir() {
            copy_dir_all(&from, &to)?;
        } else {
            fs::copy(&from, &to)
                .map_err(|error| format!("复制文件失败：{} -> {} ({error})", from.display(), to.display()))?;
            if let Ok(metadata) = fs::metadata(&from) {
                let _ = fs::set_permissions(&to, metadata.permissions());
            }
        }
    }

    Ok(())
}

fn make_node_runtime_executable(node_dir: &Path) -> Result<(), String> {
    for name in ["node", "npm", "npx"] {
        let path = node_dir.join("bin").join(name);
        if !path.exists() {
            continue;
        }

        let mut permissions = fs::metadata(&path)
            .map_err(|error| format!("读取 {} 权限失败：{error}", path.display()))?
            .permissions();
        permissions.set_mode(permissions.mode() | 0o755);
        fs::set_permissions(&path, permissions)
            .map_err(|error| format!("设置 {} 可执行权限失败：{error}", path.display()))?;
    }

    Ok(())
}

fn npm_cli_path(node_dir: &Path) -> PathBuf {
    node_dir
        .join("lib")
        .join("node_modules")
        .join("npm")
        .join("bin")
        .join("npm-cli.js")
}

fn npx_cli_path(node_dir: &Path) -> PathBuf {
    node_dir
        .join("lib")
        .join("node_modules")
        .join("npm")
        .join("bin")
        .join("npx-cli.js")
}

fn repair_node_npm_launchers(node_dir: &Path) -> Result<(), String> {
    let bin_dir = node_dir.join("bin");
    let node = bin_dir.join("node");
    let npm_cli = npm_cli_path(node_dir);
    let npx_cli = npx_cli_path(node_dir);

    if !node.exists() {
        return Err(format!("内置 Node 缺少可执行文件：{}", node.display()));
    }
    if !npm_cli.exists() {
        return Err(format!("内置 npm 缺少入口文件：{}", npm_cli.display()));
    }

    write_node_launcher(
        &bin_dir.join("npm"),
        r#"../lib/node_modules/npm/bin/npm-cli.js"#,
    )?;

    if npx_cli.exists() {
        write_node_launcher(
            &bin_dir.join("npx"),
            r#"../lib/node_modules/npm/bin/npx-cli.js"#,
        )?;
    }

    Ok(())
}

fn write_node_launcher(path: &Path, cli_relative_path: &str) -> Result<(), String> {
    if fs::symlink_metadata(path).is_ok() {
        fs::remove_file(path)
            .map_err(|error| format!("重建 {} 失败：{error}", path.display()))?;
    }

    let script = format!(
        "#!/bin/sh\nDIR=\"$(CDPATH= cd \"$(dirname \"$0\")\" && pwd)\"\nexec \"$DIR/node\" \"$DIR/{cli_relative_path}\" \"$@\"\n"
    );

    fs::write(path, script).map_err(|error| format!("写入 {} 失败：{error}", path.display()))?;
    let mut permissions = fs::metadata(path)
        .map_err(|error| format!("读取 {} 权限失败：{error}", path.display()))?
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions)
        .map_err(|error| format!("设置 {} 可执行权限失败：{error}", path.display()))?;

    Ok(())
}

fn resolve_npm_cmd(app: &tauri::AppHandle, log: &mut Vec<String>) -> Result<(PathBuf, PathBuf), String> {
    match ensure_node_runtime(app, log) {
        Ok(node_dir) => {
            prepend_process_path(&[node_dir.join("bin")]);
            Ok((node_dir.join("bin").join("npm"), node_dir))
        }
        Err(runtime_error) => match command_output("npm", &["--version"]) {
            Ok(version) => {
                log.push(format!(
                    "内置 Node runtime 不可用，临时使用系统 npm {version}。原因：{runtime_error}"
                ));
                Ok((PathBuf::from("npm"), PathBuf::from("")))
            }
            Err(npm_error) => Err(format!("{runtime_error}\n\n系统 npm 也不可用：{npm_error}")),
        },
    }
}

fn prepend_process_path(paths: &[PathBuf]) {
    let mut parts: Vec<String> = paths
        .iter()
        .filter(|path| !path.as_os_str().is_empty())
        .map(|path| path.to_string_lossy().to_string())
        .collect();

    if let Ok(current) = std::env::var("PATH") {
        parts.push(current);
    }

    std::env::set_var("PATH", parts.join(":"));
}

fn remove_process_path_entries(paths: &[PathBuf]) {
    let Ok(current) = std::env::var("PATH") else {
        return;
    };

    let targets: Vec<String> = paths
        .iter()
        .map(|path| normalize_unix_path_text(&path.to_string_lossy()))
        .collect();

    let parts: Vec<String> = current
        .split(':')
        .filter_map(|part| {
            let trimmed = part.trim();
            if trimmed.is_empty()
                || targets
                    .iter()
                    .any(|target| normalize_unix_path_text(trimmed) == *target)
            {
                None
            } else {
                Some(trimmed.to_string())
            }
        })
        .collect();

    std::env::set_var("PATH", parts.join(":"));
}

fn read_macos_env_file() -> HashMap<String, String> {
    let mut vars = HashMap::new();
    let Ok(path) = env_file_path() else {
        return vars;
    };
    let Ok(content) = fs::read_to_string(path) else {
        return vars;
    };

    for line in content.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("export ") else {
            continue;
        };
        let Some((name, value)) = rest.split_once('=') else {
            continue;
        };
        vars.insert(name.trim().to_string(), unquote_shell_value(value.trim()));
    }

    vars
}

fn unquote_shell_value(value: &str) -> String {
    if value.starts_with('\'') && value.ends_with('\'') && value.len() >= 2 {
        value[1..value.len() - 1].replace("'\\''", "'")
    } else if value.starts_with('"') && value.ends_with('"') && value.len() >= 2 {
        value[1..value.len() - 1].to_string()
    } else {
        value.to_string()
    }
}

fn write_macos_env_file(api_key: &str) -> Result<(), String> {
    let env_file = env_file_path()?;
    let claude_bin = managed_claude_bin_dir();
    let node_bin = managed_node_dir()?.join("bin");

    let mut content = String::new();
    content.push_str("# Claude Code + DeepSeek V4 Configurator\n");
    content.push_str("# Managed by the desktop configurator. Do not paste this file online.\n\n");
    content.push_str("__claude_ds_path_add() {\n");
    content.push_str("  case \":$PATH:\" in\n");
    content.push_str("    *\":$1:\"*) ;;\n");
    content.push_str("    *) export PATH=\"$1:$PATH\" ;;\n");
    content.push_str("  esac\n");
    content.push_str("}\n");
    content.push_str(&format!("__claude_ds_path_add {}\n", shell_quote(&claude_bin.to_string_lossy())));
    content.push_str(&format!("__claude_ds_path_add {}\n", shell_quote(&node_bin.to_string_lossy())));
    content.push_str("unset -f __claude_ds_path_add\n\n");
    content.push_str(&format!(
        "export {}={}\n",
        CLAUDE_AUTOUPDATER_ENV_VAR,
        shell_quote("1")
    ));
    for (name, value) in DEEPSEEK_ENV_VARS {
        content.push_str(&format!("export {name}={}\n", shell_quote(value)));
    }
    content.push_str(&format!(
        "export {}={}\n",
        AUTH_TOKEN_ENV_VAR,
        shell_quote(api_key)
    ));

    fs::write(&env_file, content)
        .map_err(|error| format!("写入 {} 失败：{error}", env_file.display()))?;
    let mut permissions = fs::metadata(&env_file)
        .map_err(|error| format!("读取 {} 权限失败：{error}", env_file.display()))?
        .permissions();
    permissions.set_mode(0o600);
    fs::set_permissions(&env_file, permissions)
        .map_err(|error| format!("设置 {} 权限失败：{error}", env_file.display()))?;

    Ok(())
}

fn apply_process_env(api_key: &str) {
    std::env::set_var(CLAUDE_AUTOUPDATER_ENV_VAR, "1");
    for (name, value) in DEEPSEEK_ENV_VARS {
        std::env::set_var(name, value);
    }
    std::env::set_var(AUTH_TOKEN_ENV_VAR, api_key);

    if let Ok(node_dir) = managed_node_dir() {
        prepend_process_path(&[managed_claude_bin_dir(), node_dir.join("bin")]);
    }
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn check_claude_path_priority() -> ToolCheck {
    let Some(path) = first_claude_from_login_shell() else {
        return ToolCheck {
            installed: false,
            version: None,
            meets_requirement: Some(false),
            message: "新终端 PATH 还没有命中 claude；一键部署会自动写入本软件管理路径。".to_string(),
        };
    };

    let managed = is_managed_claude_path(&path);
    ToolCheck {
        installed: true,
        version: None,
        meets_requirement: Some(managed),
        message: if managed {
            format!("新终端会优先使用本软件管理的 Claude：{}", path.display())
        } else {
            format!(
                "新终端优先命中其他 Claude：{}；一键部署会尝试把本软件管理路径放到最前面。",
                path.display()
            )
        },
    }
}

fn ensure_managed_claude_priority() -> String {
    if let Err(error) = install_shell_profile_link() {
        return format!("Claude 命令优先级修复失败：{error}");
    }

    let path_entries = managed_tool_path_entries();
    prepend_process_path(&path_entries);

    let priority = check_claude_path_priority();
    if priority.meets_requirement == Some(true) {
        format!("Claude 命令优先级已确认：{}", priority.message)
    } else {
        format!(
            "Claude 命令优先级提醒：{} 如果客户 shell 配置里还有后续覆盖 PATH 的内容，可能需要手动清理旧 Claude 路径。",
            priority.message
        )
    }
}

fn first_claude_from_login_shell() -> Option<PathBuf> {
    let output = command_output(
        "/bin/zsh",
        &[
            "-lc",
            "printf '__CLAUDE_PATH__%s\\n' \"$(command -v claude 2>/dev/null)\"",
        ],
    )
    .ok()?;
    let path = output
        .lines()
        .find_map(|line| line.trim().strip_prefix("__CLAUDE_PATH__"))?
        .trim();

    if path.starts_with('/') {
        Some(PathBuf::from(path))
    } else {
        None
    }
}

fn is_managed_claude_path(path: &Path) -> bool {
    let Ok(prefix) = managed_claude_prefix_dir() else {
        return false;
    };

    let path_text = normalize_unix_path_text(&path.to_string_lossy());
    let prefix_text = normalize_unix_path_text(&prefix.to_string_lossy());
    path_text == prefix_text || path_text.starts_with(&(prefix_text + "/"))
}

fn normalize_unix_path_text(value: &str) -> String {
    value
        .trim()
        .trim_matches('"')
        .trim_end_matches('/')
        .to_string()
}

fn install_shell_profile_link() -> Result<(), String> {
    let source_line = format!(
        "{PROFILE_START}\n[ -f \"$HOME/{ENV_FILE_NAME}\" ] && . \"$HOME/{ENV_FILE_NAME}\"\n{PROFILE_END}\n"
    );

    for profile in shell_profile_paths()? {
        let existing = fs::read_to_string(&profile).unwrap_or_default();
        let cleaned = remove_profile_block(&existing);
        let next = if cleaned.trim().is_empty() {
            source_line.clone()
        } else {
            format!("{}\n{}", cleaned.trim_end(), source_line)
        };
        fs::write(&profile, next)
            .map_err(|error| format!("写入 {} 失败：{error}", profile.display()))?;
    }

    Ok(())
}

fn remove_shell_profile_link() -> Result<(), String> {
    for profile in shell_profile_paths()? {
        if !profile.exists() {
            continue;
        }
        let existing = fs::read_to_string(&profile)
            .map_err(|error| format!("读取 {} 失败：{error}", profile.display()))?;
        let cleaned = remove_profile_block(&existing);
        fs::write(&profile, cleaned)
            .map_err(|error| format!("写入 {} 失败：{error}", profile.display()))?;
    }

    Ok(())
}

fn shell_profile_paths() -> Result<Vec<PathBuf>, String> {
    let home = home_dir()?;
    Ok(vec![home.join(".zshrc"), home.join(".zprofile"), home.join(".bash_profile")])
}

fn remove_profile_block(content: &str) -> String {
    let mut output = content.to_string();
    loop {
        let Some(start) = output.find(PROFILE_START) else {
            break;
        };
        let Some(end_relative) = output[start..].find(PROFILE_END) else {
            break;
        };
        let end = start + end_relative + PROFILE_END.len();
        let remove_to = output[end..]
            .find('\n')
            .map(|offset| end + offset + 1)
            .unwrap_or(end);
        output.replace_range(start..remove_to, "");
    }
    output
}

fn claude_candidates() -> Vec<PathBuf> {
    let mut candidates = managed_claude_candidates();

    if let Ok(home) = home_dir() {
        candidates.push(home.join(".local").join("bin").join("claude"));
        candidates.push(home.join(".npm-global").join("bin").join("claude"));
    }

    candidates.push(PathBuf::from("/opt/homebrew/bin/claude"));
    candidates.push(PathBuf::from("/usr/local/bin/claude"));

    candidates
}

fn managed_claude_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();

    if let Ok(prefix) = managed_claude_prefix_dir() {
        candidates.push(prefix.join("bin").join("claude"));
    }

    candidates
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
                let combined = command_text_from_output(&output);
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
            verify_claude,
            one_click_uninstall
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
