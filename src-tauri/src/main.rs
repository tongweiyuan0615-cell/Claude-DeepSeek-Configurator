#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use serde::Serialize;
use std::path::PathBuf;
use std::process::Command;

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

fn command_output(program: &str, args: &[&str]) -> Result<String, String> {
    let output = Command::new(program)
        .args(args)
        .output()
        .map_err(|error| format!("无法执行 {program}: {error}"))?;

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
        Ok(combined)
    } else {
        Err(if combined.is_empty() {
            format!("{program} exited with status {}", output.status)
        } else {
            combined
        })
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

fn check_node() -> ToolCheck {
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

fn check_npm() -> ToolCheck {
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
        Ok(version) => ToolCheck {
            installed: true,
            version: Some(version.clone()),
            meets_requirement: None,
            message: format!("已安装 {version}"),
        },
        Err(path_error) => match command_output_from_candidates(&claude_candidates(), &["--version"]) {
            Ok(version) => ToolCheck {
                installed: true,
                version: Some(version.clone()),
                meets_requirement: None,
                message: format!("已安装 {version}"),
            },
            Err(candidate_error) => ToolCheck {
                installed: false,
                version: None,
                meets_requirement: None,
                message: format!("{path_error}\n{candidate_error}"),
            },
        },
    }
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
fn check_environment() -> EnvironmentStatus {
    let (deepseek_configured, missing_env_vars) = deepseek_config_status();

    EnvironmentStatus {
        node: check_node(),
        npm: check_npm(),
        claude: check_claude(),
        deepseek_configured,
        missing_env_vars,
    }
}

#[tauri::command]
fn install_claude() -> CommandResult {
    install_claude_native()
}

fn install_claude_native() -> CommandResult {
    if let Err(error) = ensure_windows() {
        return error;
    }

    let mut log = Vec::new();
    let native_result = command_output(
        "powershell.exe",
        &[
            "-NoProfile",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            "& ([scriptblock]::Create((Invoke-RestMethod 'https://claude.ai/install.ps1')))",
        ],
    );

    match native_result {
        Ok(output) => {
            if !output.trim().is_empty() {
                log.push(bounded_output(output));
            }

            refresh_process_path_from_registry();
            let check = check_claude();
            if check.installed {
                return CommandResult {
                    success: true,
                    message: "Claude Code 安装完成".to_string(),
                    output: Some(log.join("\n\n")).filter(|value| !value.trim().is_empty()),
                };
            }

            log.push(format!("官方安装器执行完成，但当前进程还未找到 claude：{}", check.message));
        }
        Err(error) => {
            log.push(format!("官方 Windows 安装器失败：{}", bounded_output(error)));
        }
    }

    if !check_npm().installed {
        return CommandResult {
            success: false,
            message: "Claude Code 安装失败".to_string(),
            output: Some(format!(
                "{}\n\n官方安装器不可用，且本机没有可用的 npm 兜底安装路径。",
                log.join("\n\n")
            )),
        };
    }

    match command_output("npm.cmd", &["install", "-g", "@anthropic-ai/claude-code"]) {
        Ok(output) => {
            if !output.trim().is_empty() {
                log.push(format!("npm 兜底安装输出：\n{}", bounded_output(output)));
            }

            refresh_process_path_from_registry();
            let check = check_claude();
            if check.installed {
                CommandResult {
                    success: true,
                    message: "Claude Code 安装完成".to_string(),
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
fn one_click_setup(api_key: String) -> CommandResult {
    if api_key.trim().is_empty() {
        return CommandResult {
            success: false,
            message: "请输入 DeepSeek API Key".to_string(),
            output: None,
        };
    }

    let mut log = Vec::new();

    if !check_claude().installed {
        let install_result = install_claude_native();
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
        log.push("Claude Code 已安装，跳过安装步骤".to_string());
    }

    let configure_result = configure_deepseek(api_key);
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
        Ok(output) => CommandResult {
            success: true,
            message: "Claude Code 可执行".to_string(),
            output: Some(output),
        },
        Err(path_error) => match command_output_from_candidates(&claude_candidates(), &["--version"]) {
            Ok(output) => CommandResult {
                success: true,
                message: "Claude Code 可执行".to_string(),
                output: Some(output),
            },
            Err(candidate_error) => CommandResult {
                success: false,
                message: "Claude Code 验证失败".to_string(),
                output: Some(format!("{path_error}\n{candidate_error}")),
            },
        },
    }
}

#[tauri::command]
fn clear_deepseek_config() -> CommandResult {
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

    if let Ok(appdata) = std::env::var("APPDATA") {
        candidates.push(PathBuf::from(&appdata).join("npm").join("claude.cmd"));
        candidates.push(PathBuf::from(appdata).join("npm").join("claude.exe"));
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

fn command_output_from_candidates(paths: &[PathBuf], args: &[&str]) -> Result<String, String> {
    let mut checked = Vec::new();

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
                    return Ok(combined);
                }

                checked.push(format!("{}: {}", path.display(), combined));
            }
            Err(error) => checked.push(format!("{}: {}", path.display(), error)),
        }
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
            install_claude,
            configure_deepseek,
            one_click_setup,
            verify_claude,
            clear_deepseek_config
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
