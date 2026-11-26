//! 系统兜底任务（T17；02 §7.2.1 §4）。
//!
//! **Windows**：每个「启用且有 cron」的站点注册**一个**计划任务
//! （名字 `LLM Router Checkin - <site_id>`，动作 `"<exe>" --checkin-once --site <site_id>`）。
//! **macOS**：同口径的 launchd agent（`~/Library/LaunchAgents/com.llmrouter.checkin.<site>.plist`）。
//!
//! cron → 计划任务频率映射（**不认识就退回每天一次**：宁可多跑，不可漏跑）：
//! `*/N * * * *` → 每 N 分钟；`M * * * *` → 每小时第 M 分；`M H * * *` → 每天 H:M；
//! `M H * * D` → 每周；`M H D * *` → 每月；其它 → 每天一次。

/// 任务名前缀（Windows 计划任务名 / macOS launchd label 的统一口径）。
pub const TASK_PREFIX: &str = "LLM Router Checkin";

/// 当前平台是否支持系统兜底。
pub const SUPPORTED: bool = cfg!(target_os = "windows") || cfg!(target_os = "macos");

/// 任务名（按站点 id）。
pub fn task_name(site_id: &str) -> String {
    format!("{TASK_PREFIX} - {site_id}")
}

/// 系统兜底任务的现状。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskStatus {
    /// 任务名。
    pub name: String,
    /// 是否已注册。
    pub registered: bool,
    /// 计划任务频率描述（给 UI 看）。
    pub frequency: Option<String>,
}

/// 注册（或覆盖）一个站点的系统兜底任务。
pub fn register(site_id: &str, cron_expr: &str) -> Result<TaskStatus, String> {
    imp::register(site_id, cron_expr)
}

/// 注销一个站点的系统兜底任务（没注册也算成功）。
pub fn unregister(site_id: &str) -> Result<bool, String> {
    imp::unregister(site_id)
}

/// 查一个站点的系统兜底任务是否已注册。
pub fn query(site_id: &str) -> Result<TaskStatus, String> {
    imp::query(site_id)
}

/// 列出本应用注册的系统兜底任务名。
pub fn list() -> Result<Vec<String>, String> {
    imp::list()
}

/// 当前可执行文件路径（计划任务的动作）。
pub fn current_exe() -> Result<std::path::PathBuf, String> {
    std::env::current_exe().map_err(|err| format!("取不到可执行文件路径：{err}"))
}

// ────────────────────────────── Windows ──────────────────────────────

#[cfg(target_os = "windows")]
mod imp {
    use std::process::Command;

    use windows::Win32::Globalization::{
        MultiByteToWideChar, CP_OEMCP, MULTI_BYTE_TO_WIDE_CHAR_FLAGS,
    };

    use crate::checkin::cron::CronShape;

    /// 把系统命令的输出解成可读文本。
    ///
    /// schtasks 的输出**按调用方的代码页**给：控制台里通常是 UTF-8（英文文案），
    /// GUI 进程（无控制台）里可能是**系统 OEM 代码页**（中文 Windows = GBK/936）。
    /// 直接按 UTF-8 解会出乱码（实测踩过），所以：UTF-8 → UTF-16LE → OEM 代码页
    /// 依次尝试，最后才退回有损解码（保证不 panic）。
    fn decode_output(bytes: &[u8]) -> String {
        // UTF-8 BOM
        let bytes = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
        if let Ok(text) = std::str::from_utf8(bytes) {
            return text.trim().to_string();
        }
        // UTF-16LE BOM
        if bytes.starts_with(&[0xFF, 0xFE]) {
            let units: Vec<u16> = bytes[2..]
                .chunks_exact(2)
                .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
                .collect();
            return String::from_utf16_lossy(&units).trim().to_string();
        }
        // 非 UTF-8 → 按系统 OEM 代码页（中文 Windows = GBK/936）解
        let flags = MULTI_BYTE_TO_WIDE_CHAR_FLAGS(0);
        unsafe {
            let length = MultiByteToWideChar(CP_OEMCP, flags, bytes, None);
            if length > 0 {
                let mut wide = vec![0u16; length as usize];
                if MultiByteToWideChar(CP_OEMCP, flags, bytes, Some(&mut wide)) > 0 {
                    return String::from_utf16_lossy(&wide).trim().to_string();
                }
            }
        }
        String::from_utf8_lossy(bytes).trim().to_string()
    }

    /// 计划任务的动作：应用自身 + `--checkin-once --site <id>`。
    fn action(site_id: &str) -> Result<String, String> {
        super::action_for(site_id)
    }

    /// cron → 计划任务频率参数。
    fn frequency_args(shape: &CronShape) -> (Vec<String>, String) {
        match shape {
            CronShape::EveryMinutes(step) => (
                vec![
                    "/sc".to_string(),
                    "minute".to_string(),
                    "/mo".to_string(),
                    step.to_string(),
                ],
                format!("每 {step} 分钟"),
            ),
            CronShape::Hourly(minute) => (
                vec![
                    "/sc".to_string(),
                    "hourly".to_string(),
                    "/st".to_string(),
                    format!("{minute:02}:00"),
                ],
                format!("每小时第 {minute} 分钟"),
            ),
            CronShape::Daily { hour, minute } => (
                vec![
                    "/sc".to_string(),
                    "daily".to_string(),
                    "/st".to_string(),
                    format!("{hour:02}:{minute:02}"),
                ],
                format!("每天 {hour:02}:{minute:02}"),
            ),
            CronShape::Weekly {
                hour,
                minute,
                days,
            } => {
                let names: Vec<&str> = days.iter().map(|day| weekday_name(*day)).collect();
                (
                    vec![
                        "/sc".to_string(),
                        "weekly".to_string(),
                        "/d".to_string(),
                        names.join(","),
                        "/st".to_string(),
                        format!("{hour:02}:{minute:02}"),
                    ],
                    format!("每周 {} {hour:02}:{minute:02}", names.join(",")),
                )
            }
            CronShape::Monthly { hour, minute, day } => (
                vec![
                    "/sc".to_string(),
                    "monthly".to_string(),
                    "/d".to_string(),
                    day.to_string(),
                    "/st".to_string(),
                    format!("{hour:02}:{minute:02}"),
                ],
                format!("每月 {day} 日 {hour:02}:{minute:02}"),
            ),
            // 不认识的表达式 → 每天一次（宁可多跑，不可漏跑）
            CronShape::Other => (
                vec![
                    "/sc".to_string(),
                    "daily".to_string(),
                    "/st".to_string(),
                    "09:00".to_string(),
                ],
                "每天 09:00（表达式未识别，退回每天一次）".to_string(),
            ),
        }
    }

    fn weekday_name(day: u32) -> &'static str {
        match day {
            1 => "MON",
            2 => "TUE",
            3 => "WED",
            4 => "THU",
            5 => "FRI",
            6 => "SAT",
            _ => "SUN",
        }
    }

    pub fn register(site_id: &str, cron_expr: &str) -> Result<super::TaskStatus, String> {
        let name = super::task_name(site_id);
        let shape = crate::checkin::cron::classify(cron_expr);
        let (freq_args, frequency) = frequency_args(&shape);
        let action = action(site_id)?;
        let mut command = Command::new("schtasks");
        command
            .arg("/create")
            .arg("/tn")
            .arg(&name)
            .arg("/tr")
            .arg(&action)
            .arg("/f");
        for arg in freq_args {
            command.arg(arg);
        }
        let output = command
            .output()
            .map_err(|err| format!("调用 schtasks 失败：{err}"))?;
        if !output.status.success() {
            return Err(format!(
                "注册计划任务失败：{}",
                decode_output(&output.stderr)
            ));
        }
        Ok(super::TaskStatus {
            name,
            registered: true,
            frequency: Some(frequency),
        })
    }

    pub fn unregister(site_id: &str) -> Result<bool, String> {
        let name = super::task_name(site_id);
        // **先查在不在**：不在就直接算"已注销"。
        // 不能靠错误文案判断——schtasks 的文案随系统语言变（英文系统
        // "ERROR: The system cannot find the file specified."、中文系统
        // "错误: 系统找不到指定的文件。"），按字符串匹配必然漏判（实测踩过）。
        if !query(site_id)?.registered {
            return Ok(false);
        }
        let output = Command::new("schtasks")
            .arg("/delete")
            .arg("/tn")
            .arg(&name)
            .arg("/f")
            .output()
            .map_err(|err| format!("调用 schtasks 失败：{err}"))?;
        if output.status.success() {
            return Ok(true);
        }
        Err(format!(
            "注销计划任务失败：{}",
            decode_output(&output.stderr)
        ))
    }

    pub fn query(site_id: &str) -> Result<super::TaskStatus, String> {
        let name = super::task_name(site_id);
        let output = Command::new("schtasks")
            .arg("/query")
            .arg("/tn")
            .arg(&name)
            .output()
            .map_err(|err| format!("调用 schtasks 失败：{err}"))?;
        Ok(super::TaskStatus {
            name,
            registered: output.status.success(),
            frequency: None,
        })
    }

    pub fn list() -> Result<Vec<String>, String> {
        let output = Command::new("schtasks")
            .arg("/query")
            .arg("/fo")
            .arg("LIST")
            .output()
            .map_err(|err| format!("调用 schtasks 失败：{err}"))?;
        let text = String::from_utf8_lossy(&output.stdout);
        let mut names = Vec::new();
        for line in text.lines() {
            let line = line.trim();
            let Some(rest) = line.strip_prefix("TaskName:") else {
                continue;
            };
            // schtasks 输出根目录任务时带前导 `\`（如 `\LLM Router Checkin - x`），
            // 这里剥掉，保证与 `task_name()` 产出的名字口径一致。
            let task = rest.trim().trim_start_matches('\\').trim();
            if task.starts_with(super::TASK_PREFIX) {
                names.push(task.to_string());
            }
        }
        Ok(names)
    }
}

// ────────────────────────────── macOS ──────────────────────────────

#[cfg(target_os = "macos")]
mod imp {
    use std::process::Command;

    use crate::checkin::cron::CronShape;

    fn plist_path(site_id: &str) -> std::path::PathBuf {
        let home = std::env::var("HOME").unwrap_or_default();
        std::path::Path::new(&home)
            .join("Library")
            .join("LaunchAgents")
            .join(format!("com.llmrouter.checkin.{site_id}.plist"))
    }

    fn label(site_id: &str) -> String {
        format!("com.llmrouter.checkin.{site_id}")
    }

    pub fn register(site_id: &str, cron_expr: &str) -> Result<super::TaskStatus, String> {
        let exe = super::current_exe()?;
        let shape = crate::checkin::cron::classify(cron_expr);
        let (schedule_xml, frequency) = match shape {
            CronShape::EveryMinutes(step) => (
                format!("    <key>StartInterval</key>\n    <integer>{}</integer>\n", step * 60),
                format!("每 {step} 分钟"),
            ),
            CronShape::Hourly(minute) => (
                format!(
                    "    <key>StartCalendarInterval</key>\n    <dict>\n      <key>Minute</key>\n      <integer>{minute}</integer>\n    </dict>\n"
                ),
                format!("每小时第 {minute} 分钟"),
            ),
            CronShape::Daily { hour, minute } => (
                format!(
                    "    <key>StartCalendarInterval</key>\n    <dict>\n      <key>Hour</key>\n      <integer>{hour}</integer>\n      <key>Minute</key>\n      <integer>{minute}</integer>\n    </dict>\n"
                ),
                format!("每天 {hour:02}:{minute:02}"),
            ),
            CronShape::Weekly {
                hour,
                minute,
                days,
            } => {
                // launchd 的 Weekday：0=周日 … 6=周六
                let entries: String = days
                    .iter()
                    .map(|day| {
                        format!(
                            "    <dict>\n      <key>Weekday</key>\n      <integer>{}</integer>\n      <key>Hour</key>\n      <integer>{hour}</integer>\n      <key>Minute</key>\n      <integer>{minute}</integer>\n    </dict>\n",
                            day % 7
                        )
                    })
                    .collect();
                (
                    format!(
                        "    <key>StartCalendarInterval</key>\n    <array>\n{entries}    </array>\n"
                    ),
                    format!("每周 {} {hour:02}:{minute:02}", days.join(",")),
                )
            }
            CronShape::Monthly { hour, minute, day } => (
                format!(
                    "    <key>StartCalendarInterval</key>\n    <dict>\n      <key>Day</key>\n      <integer>{day}</integer>\n      <key>Hour</key>\n      <integer>{hour}</integer>\n      <key>Minute</key>\n      <integer>{minute}</integer>\n    </dict>\n"
                ),
                format!("每月 {day} 日 {hour:02}:{minute:02}"),
            ),
            CronShape::Other => (
                format!(
                    "    <key>StartCalendarInterval</key>\n    <dict>\n      <key>Hour</key>\n      <integer>9</integer>\n      <key>Minute</key>\n      <integer>0</integer>\n    </dict>\n"
                ),
                "每天 09:00（表达式未识别，退回每天一次）".to_string(),
            ),
        };

        let plist = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<plist version=\"1.0\">\n<dict>\n  <key>Label</key>\n  <string>{label}</string>\n  <key>ProgramArguments</key>\n  <array>\n    <string>{}</string>\n    <string>--checkin-once</string>\n    <string>--site</string>\n    <string>{site_id}</string>\n  </array>\n  <key>RunAtLoad</key>\n  <false/>\n{schedule_xml}</dict>\n</plist>\n",
            exe.display()
        );
        let path = plist_path(site_id);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|err| format!("建目录失败：{err}"))?;
        }
        std::fs::write(&path, plist).map_err(|err| format!("写 plist 失败：{err}"))?;
        let _ = Command::new("launchctl")
            .arg("unload")
            .arg("-w")
            .arg(&path)
            .output();
        let output = Command::new("launchctl")
            .arg("load")
            .arg("-w")
            .arg(&path)
            .output()
            .map_err(|err| format!("调用 launchctl 失败：{err}"))?;
        if !output.status.success() {
            return Err(format!(
                "注册 launchd agent 失败：{}",
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        Ok(super::TaskStatus {
            name: label(site_id),
            registered: true,
            frequency: Some(frequency),
        })
    }

    pub fn unregister(site_id: &str) -> Result<bool, String> {
        let path = plist_path(site_id);
        if !path.exists() {
            return Ok(false);
        }
        let _ = Command::new("launchctl")
            .arg("unload")
            .arg("-w")
            .arg(&path)
            .output();
        std::fs::remove_file(&path).map_err(|err| format!("删 plist 失败：{err}"))?;
        Ok(true)
    }

    pub fn query(site_id: &str) -> Result<super::TaskStatus, String> {
        Ok(super::TaskStatus {
            name: label(site_id),
            registered: plist_path(site_id).exists(),
            frequency: None,
        })
    }

    pub fn list() -> Result<Vec<String>, String> {
        let home = std::env::var("HOME").unwrap_or_default();
        let dir = std::path::Path::new(&home).join("Library").join("LaunchAgents");
        let mut names = Vec::new();
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if name.starts_with("com.llmrouter.checkin.") {
                    names.push(name);
                }
            }
        }
        Ok(names)
    }
}

// ────────────────────────────── 其它平台 ──────────────────────────────

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
mod imp {
    pub fn register(_site_id: &str, _cron_expr: &str) -> Result<super::TaskStatus, String> {
        Err("当前平台没有系统兜底实现（只支持 Windows 计划任务 / macOS launchd）".to_string())
    }

    pub fn unregister(_site_id: &str) -> Result<bool, String> {
        Err("当前平台没有系统兜底实现".to_string())
    }

    pub fn query(_site_id: &str) -> Result<super::TaskStatus, String> {
        Ok(super::TaskStatus {
            name: String::new(),
            registered: false,
            frequency: None,
        })
    }

    pub fn list() -> Result<Vec<String>, String> {
        Ok(Vec::new())
    }
}

/// 便于测试：拼出计划任务的动作串（`"<exe>" --checkin-once --site <id>`）。
pub fn action_for(site_id: &str) -> Result<String, String> {
    let exe = current_exe()?;
    Ok(format!(
        "\"{}\" --checkin-once --site {site_id}",
        exe.display()
    ))
}
