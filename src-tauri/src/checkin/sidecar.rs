//! 浏览器辅轨的 Rust 侧客户端（T18；02 §7.4 §3）。
//!
//! **CLI 协议**：每次调用辅轨就起一个 Node 进程跑一个动作，**stdout 吐 JSON**，
//! 用完即退——比本地 HTTP 少活动部件（无端口占用 / 无孤儿进程 / 无本地 socket 鉴权问题）。
//!
//! 辅轨本身是 `sidecar/`（Node + Playwright）：驱动**浏览器**跑签到，
//! **持久化 profile** 复用登录态。**签名由浏览器自己算**——应用不逆向、不伪造
//! （AGENT.md §5.7）。
//!
//! **找不到 Node 或 sidecar 脚本时一律返回 `None` / 报错**，不静默降级成"假装跑过了"。

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// `health` 动作的返回。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SidecarHealth {
    /// Node 运行时版本。
    pub node_version: String,
    /// 浏览器可用（系统 Chrome 或 Playwright 自带 chromium）。
    pub browser_available: bool,
    /// 用的哪个浏览器（`chrome` / `chromium`）。
    pub browser_channel: String,
    /// 持久化 profile 目录。
    pub profile_dir: String,
    /// 站点是否已有登录态（**只报"有/没有"，不报任何凭据内容**）。
    pub logged_in: Option<bool>,
}

/// 辅轨客户端（**只管"怎么把动作跑起来"**，不懂业务）。
#[derive(Debug, Clone)]
pub struct Sidecar {
    /// Node 可执行（`node`）。
    node: PathBuf,
    /// sidecar 脚本（`index.mjs`）。
    script: PathBuf,
    /// 持久化 profile 目录。
    profile_dir: PathBuf,
}

/// 环境变量：sidecar 脚本目录（覆盖自动探测）。
pub const ENV_SIDECAR_DIR: &str = "LLMR_SIDECAR_DIR";
/// 环境变量：浏览器 profile 目录。
pub const ENV_PROFILE_DIR: &str = "LLMR_BROWSER_PROFILE";
/// 环境变量：Node 可执行。
pub const ENV_NODE: &str = "LLMR_NODE";

impl Sidecar {
    /// 构造（一般用 [`Sidecar::default_instance`]）。
    pub fn new(node: PathBuf, script: PathBuf, profile_dir: PathBuf) -> Self {
        Sidecar {
            node,
            script,
            profile_dir,
        }
    }

    /// 默认实例：按约定路径找 sidecar 脚本 + Node；**找不到就返回 `None`**。
    ///
    /// 探测顺序：`LLMR_SIDECAR_DIR` → 可执行文件同级 `sidecar/` → 可执行文件上溯两级的
    /// `sidecar/`（开发时的 `target/debug/` → 仓库根）→ 当前工作目录的 `sidecar/`。
    pub fn default_instance() -> Option<Self> {
        let script_dir = find_sidecar_dir()?;
        let script = script_dir.join("index.mjs");
        if !script.exists() {
            return None;
        }
        let node = std::env::var_os(ENV_NODE)
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("node"));
        Some(Sidecar::new(node, script, default_profile_dir()))
    }

    /// 浏览器 profile 目录（默认在应用数据目录下；**不进 git**）。
    pub fn profile_dir(&self) -> &Path {
        &self.profile_dir
    }

    /// 跑一个辅轨动作，返回 stdout 的 JSON。
    ///
    /// `timeout` 是**动作级超时**（浏览器启动比 HTTP 慢得多，不共用重放引擎的 30s）。
    pub async fn run(
        &self,
        action: &str,
        args: &[String],
        timeout: Duration,
    ) -> Result<serde_json::Value, String> {
        let mut command = tokio::process::Command::new(&self.node);
        command
            .arg(&self.script)
            .arg(action)
            .env("LLMR_BROWSER_PROFILE", &self.profile_dir)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        for arg in args {
            command.arg(arg);
        }
        let child = command
            .spawn()
            .map_err(|err| format!("启动辅轨失败（{}）：{err}", self.node.display()))?;
        let output = match tokio::time::timeout(timeout, child.wait_with_output()).await {
            Ok(result) => result.map_err(|err| format!("等辅轨结束失败：{err}"))?,
            Err(_) => return Err(format!("辅轨动作 `{action}` 超时（{}s）", timeout.as_secs())),
        };
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        if stdout.is_empty() {
            return Err(if stderr.is_empty() {
                format!("辅轨动作 `{action}` 没有输出")
            } else {
                format!("辅轨动作 `{action}` 没有输出：{stderr}")
            });
        }
        serde_json::from_str(&stdout).map_err(|err| {
            format!("辅轨动作 `{action}` 的输出不是 JSON：{err}（输出前 200 字：{}）", clip(&stdout, 200))
        })
    }

    /// 健康检查。
    pub async fn health(&self) -> Result<SidecarHealth, String> {
        let value = self.run("health", &[], Duration::from_secs(30)).await?;
        serde_json::from_value(value).map_err(|err| format!("辅轨 health 输出解析失败：{err}"))
    }
}

/// 截断（给错误提示用）。
fn clip(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let head: String = text.chars().take(max).collect();
    format!("{head}…")
}

/// 找 sidecar 脚本目录。
fn find_sidecar_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os(ENV_SIDECAR_DIR) {
        let path = PathBuf::from(dir);
        if path.exists() {
            return Some(path);
        }
    }
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join("sidecar"));
            // 开发态：`<repo>/src-tauri/target/debug/llm-router.exe` → 上溯到仓库根
            for up in dir.ancestors().skip(1).take(3) {
                candidates.push(up.join("sidecar"));
            }
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd.join("sidecar"));
    }
    candidates.into_iter().find(|path| path.join("index.mjs").exists())
}

/// 默认的浏览器 profile 目录。
fn default_profile_dir() -> PathBuf {
    profile_dir_from(std::env::var_os(ENV_PROFILE_DIR))
}

/// 由环境变量值算 profile 目录（**纯函数**，单测直接打——edition 2024 下
/// `std::env::set_var` 是不安全的，多线程改环境变量不可靠）。
fn profile_dir_from(env_value: Option<std::ffi::OsString>) -> PathBuf {
    if let Some(dir) = env_value {
        return PathBuf::from(dir);
    }
    crate::store::app_data_dir().join("browser-profile")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clip_keeps_short_text_and_marks_long_text() {
        assert_eq!(clip("abc", 10), "abc");
        let long = clip("一二三四五", 3);
        assert_eq!(long, "一二三…");
    }

    #[test]
    fn profile_dir_prefers_the_env_value() {
        let dir = profile_dir_from(Some(std::ffi::OsString::from("D:\\tmp\\profile-probe")));
        assert_eq!(dir, PathBuf::from("D:\\tmp\\profile-probe"));
        let fallback = profile_dir_from(None);
        assert!(fallback.ends_with("browser-profile"), "实际 {fallback:?}");
    }
}
