import { useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

type ToolCheck = {
  installed: boolean;
  version: string | null;
  meets_requirement: boolean | null;
  message: string;
};

type EnvironmentStatus = {
  node: ToolCheck;
  npm: ToolCheck;
  claude: ToolCheck;
  deepseek_configured: boolean;
  missing_env_vars: string[];
};

type CommandResult = {
  success: boolean;
  message: string;
  output: string | null;
};

const emptyTool: ToolCheck = {
  installed: false,
  version: null,
  meets_requirement: null,
  message: "尚未检测",
};

const initialStatus: EnvironmentStatus = {
  node: emptyTool,
  npm: emptyTool,
  claude: emptyTool,
  deepseek_configured: false,
  missing_env_vars: [],
};

function StatusRow({
  label,
  check,
}: {
  label: string;
  check: ToolCheck | { installed: boolean; message: string };
}) {
  const ok = check.installed && (!("meets_requirement" in check) || check.meets_requirement !== false);

  return (
    <div className="status-row">
      <span className={`status-dot ${ok ? "ok" : "warn"}`} />
      <div>
        <div className="status-title">{label}</div>
        <div className="status-message">{check.message}</div>
      </div>
    </div>
  );
}

function App() {
  const [apiKey, setApiKey] = useState("");
  const [status, setStatus] = useState<EnvironmentStatus>(initialStatus);
  const [busy, setBusy] = useState<string | null>(null);
  const [notice, setNotice] = useState<CommandResult | null>(null);

  const canConfigure = useMemo(() => {
    return apiKey.trim().length > 0;
  }, [apiKey]);

  async function refreshStatus(clearNotice = true) {
    setBusy("check");
    if (clearNotice) {
      setNotice(null);
    }
    try {
      const next = await invoke<EnvironmentStatus>("check_environment");
      setStatus(next);
    } catch (error) {
      setNotice({
        success: false,
        message: error instanceof Error ? error.message : String(error),
        output: null,
      });
    } finally {
      setBusy(null);
    }
  }

  async function runAction(action: string, command: string, payload?: Record<string, unknown>) {
    setBusy(action);
    setNotice(null);
    try {
      const result = await invoke<CommandResult>(command, payload);
      setNotice(result);
      await refreshStatus(false);
    } catch (error) {
      setNotice({
        success: false,
        message: error instanceof Error ? error.message : String(error),
        output: null,
      });
    } finally {
      setBusy(null);
    }
  }

  async function uninstallAll() {
    const confirmed = window.confirm("这会卸载 Claude Code，并清除本机用户级 DeepSeek 配置。是否继续？");
    if (!confirmed) {
      return;
    }

    await runAction("uninstall", "uninstall_claude_and_deepseek");
  }

  useEffect(() => {
    refreshStatus();
  }, []);

  return (
    <main className="app-shell">
      <section className="workspace">
        <header className="header">
          <div>
            <h1>Claude Code + DeepSeek V4 配置器</h1>
            <p>输入 DeepSeek API Key 后，自动安装 Claude Code 并写入 Windows 用户级环境变量。</p>
          </div>
          <button className="secondary-button" onClick={() => refreshStatus()} disabled={busy !== null}>
            {busy === "check" ? "检测中" : "重新检测"}
          </button>
        </header>

        <section className="panel">
          <div className="section-heading">
            <h2>环境状态</h2>
            <span>{busy ? "正在处理" : "就绪"}</span>
          </div>
          <div className="status-grid">
            <StatusRow label="Node.js（可选）" check={status.node} />
            <StatusRow label="npm（可选）" check={status.npm} />
            <StatusRow label="Claude Code" check={status.claude} />
            <StatusRow
              label="DeepSeek 环境变量"
              check={{
                installed: status.deepseek_configured,
                message: status.deepseek_configured
                  ? "已配置"
                  : status.missing_env_vars.length > 0
                    ? `未完整配置：${status.missing_env_vars.join(", ")}`
                    : "未配置",
              }}
            />
          </div>
        </section>

        <section className="panel">
          <div className="section-heading">
            <h2>API Key</h2>
            <span>仅写入本机用户环境变量</span>
          </div>
          <label className="field">
            <span>DeepSeek API Key</span>
            <input
              value={apiKey}
              onChange={(event) => setApiKey(event.target.value)}
              type="password"
              autoComplete="off"
              spellCheck={false}
              placeholder="输入 DeepSeek API Key"
            />
          </label>
          <div className="actions">
            {(!status.claude.installed || status.claude.meets_requirement === false) && (
              <button
                className="secondary-button"
                onClick={() => runAction("install", "install_claude")}
                disabled={busy !== null}
              >
                {busy === "install" ? "安装中" : "安装兼容版 Claude Code"}
              </button>
            )}
            <button
              className="primary-button"
              onClick={() => runAction("configure", "one_click_setup", { apiKey })}
              disabled={busy !== null || !canConfigure}
            >
              {busy === "configure" ? "部署中" : "一键部署"}
            </button>
            <button
              className="secondary-button"
              onClick={() => runAction("verify", "verify_claude")}
              disabled={busy !== null || !status.claude.installed}
            >
              验证配置
            </button>
            <button
              className="danger-button"
              onClick={() => runAction("clear", "clear_deepseek_config")}
              disabled={busy !== null}
            >
              {busy === "clear" ? "清除中" : "清除 DeepSeek 配置"}
            </button>
            <button
              className="danger-button"
              onClick={uninstallAll}
              disabled={busy !== null}
            >
              {busy === "uninstall" ? "卸载中" : "卸载 Claude Code + DeepSeek"}
            </button>
          </div>
        </section>

        {notice && (
          <section className={`notice ${notice.success ? "success" : "error"}`}>
            <strong>{notice.message}</strong>
            {notice.output && <pre>{notice.output}</pre>}
          </section>
        )}
      </section>
    </main>
  );
}

export default App;
