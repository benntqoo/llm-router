//! Windows **系统通知的 AUMID 注册**（T19；02 §7.5）。
//!
//! **为什么需要**：Windows 10 1607+ 的 toast 要求**调用进程拥有该 AUMID**
//! （`System.AppUserModel.ID`），否则系统会**静默丢弃**这条通知——
//! API 照样返回成功（真机踩过：日志写着「系统通知：已发送」，屏幕上什么都没有）。
//!
//! unpackaged 应用拿到 AUMID 的**官方做法** = 在「开始菜单\程序」下建一个
//! **快捷方式**并写上 `System.AppUserModel.ID` 属性（Tauri 通知插件也是这么做的）。
//! 注册一次就够（快捷方式常驻），重复注册会覆盖。
//!
//! **非 Windows 平台**：macOS / Linux 不需要这步（通知由各自的系统 API 直接发）。

/// 注册用的 AUMID（**与 toast 里报的名字一致**）。
pub const AUMID: &str = "LLM Router";

/// **纯函数**：全局通知开关关着时该报什么（`None` = 照常发送）。
///
/// 真机踩过（2026-11-27）：Windows 的**全局通知总开关**关着时，**所有** toast 都会被
/// **静默丢弃**——`notify-rust` 照样返回成功，日志写着「已发送」，屏幕上什么都没有。
/// 事件日志 `Microsoft-Windows-PushNotification-Platform/Operational` 里记着
/// `Id=3150 … PolicyReason [GlobalSettingDisabled]`。
/// → 发送前先查开关，**如实报告**，别让用户以为已经提醒过了。
pub fn global_block_reason(global_enabled: Option<bool>) -> Option<&'static str> {
    match global_enabled {
        Some(false) => Some("Windows 全局通知已关闭（设置 → 系统 → 通知）——请打开后再试"),
        _ => None,
    }
}

/// Windows 的**全局通知总开关**（`HKCU\…\CurrentVersion\PushNotifications\ToastEnabled`）。
///
/// - `Some(true)` = 开着；`Some(false)` = **全局关着**（toast 会被静默丢弃）；
/// - `None` = **读不到**（键不存在 / 非 Windows）→ 按「开着」处理，不拦。
pub fn global_toasts_enabled() -> Option<bool> {
    #[cfg(target_os = "windows")]
    {
        imp::global_toasts_enabled()
    }
    #[cfg(not(target_os = "windows"))]
    {
        None
    }
}

/// 确保当前 exe 拥有这个 AUMID（**Windows 专有**；其他平台直接返回 `Ok`）。
///
/// 返回**快捷方式路径**（便于日志与排查）。
pub fn ensure_aumid(app_id: &str) -> Result<Option<String>, String> {
    #[cfg(target_os = "windows")]
    {
        imp::ensure(app_id).map(Some)
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = app_id;
        Ok(None)
    }
}

#[cfg(target_os = "windows")]
mod imp {
    use windows::core::{Interface, HSTRING, PCWSTR};
    use windows::Win32::Foundation::{ERROR_SUCCESS, PROPERTYKEY};
    use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoUninitialize, IPersistFile, CLSCTX_INPROC_SERVER,
        COINIT_APARTMENTTHREADED,
    };
    use windows::Win32::System::Registry::{
        RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD,
    };
    use windows::Win32::UI::Shell::PropertiesSystem::IPropertyStore;
    use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};

    /// `PKEY_AppUserModel_ID` = `{9F4C2855-9F79-4B39-A8D0-E1D42DE1D5F3}`, pid 5。
    const PKEY_APP_USER_MODEL_ID: PROPERTYKEY = PROPERTYKEY {
        fmtid: windows::core::GUID::from_u128(0x9F4C2855_9F79_4B39_A8D0_E1D42DE1D5F3_u128),
        pid: 5,
    };

    /// 开始菜单「程序」目录（当前用户）。
    fn start_menu_programs() -> Result<std::path::PathBuf, String> {
        let base = std::env::var_os("APPDATA")
            .ok_or_else(|| "取不到 APPDATA（当前用户目录）".to_string())?;
        Ok(std::path::PathBuf::from(base)
            .join("Microsoft")
            .join("Windows")
            .join("Start Menu")
            .join("Programs"))
    }

    /// 建快捷方式 + 写 AUMID 属性。
    pub fn ensure(app_id: &str) -> Result<String, String> {
        let exe = std::env::current_exe().map_err(|err| format!("取不到 exe 路径：{err}"))?;
        let dir = start_menu_programs()?;
        std::fs::create_dir_all(&dir).map_err(|err| format!("建开始菜单目录失败：{err}"))?;
        let lnk = dir.join(format!("{app_id}.lnk"));

        unsafe {
            // COM 初始化：进程可能已经在 STA 里 → `RPC_E_CHANGED_MODE` 可忽略
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let outcome = (|| -> Result<(), String> {
                let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)
                    .map_err(|err| format!("创建快捷方式对象失败：{err}"))?;
                let exe_wide = HSTRING::from(exe.to_string_lossy().as_ref());
                link.SetPath(PCWSTR::from_raw(exe_wide.as_ptr()))
                    .map_err(|err| format!("设置快捷方式目标失败：{err}"))?;

                // 写 `System.AppUserModel.ID`
                let store: IPropertyStore = link
                    .cast()
                    .map_err(|err| format!("取属性存储失败：{err}"))?;
                let propvar = PROPVARIANT::from(app_id);
                store
                    .SetValue(&PKEY_APP_USER_MODEL_ID, &propvar)
                    .map_err(|err| format!("写 AppUserModel.ID 失败：{err}"))?;
                store
                    .Commit()
                    .map_err(|err| format!("提交属性失败：{err}"))?;

                // 落盘
                let persist: IPersistFile = link
                    .cast()
                    .map_err(|err| format!("取持久化接口失败：{err}"))?;
                let lnk_wide = HSTRING::from(lnk.to_string_lossy().as_ref());
                persist
                    .Save(&lnk_wide, true)
                    .map_err(|err| format!("保存快捷方式失败：{err}"))?;
                Ok(())
            })();
            CoUninitialize();
            outcome?;
        }
        Ok(lnk.to_string_lossy().to_string())
    }

    /// 读 `HKCU\Software\Microsoft\Windows\CurrentVersion\PushNotifications\ToastEnabled`。
    ///
    /// **这个开关是「全局」的**——关着时**所有**应用的 toast 都被 Windows 静默丢弃
    /// （事件日志 `Id=3150` / `PolicyReason [GlobalSettingDisabled]`），
    /// 而 `notify-rust` 照样返回成功。
    ///
    /// **注意**：改完这个开关**必须重启 `WpnUserService_<sid>`**（或注销重登）才生效——
    /// 通知平台服务会把策略**缓存**在内存里（真机踩过：连改两次注册表都没变化）。
    pub fn global_toasts_enabled() -> Option<bool> {
        let subkey = HSTRING::from("Software\\Microsoft\\Windows\\CurrentVersion\\PushNotifications");
        let value = HSTRING::from("ToastEnabled");
        let mut data: u32 = 0;
        let mut size: u32 = std::mem::size_of::<u32>() as u32;
        let status = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                PCWSTR::from_raw(subkey.as_ptr()),
                PCWSTR::from_raw(value.as_ptr()),
                RRF_RT_REG_DWORD,
                None,
                Some(&mut data as *mut u32 as *mut std::ffi::c_void),
                Some(&mut size),
            )
        };
        if status != ERROR_SUCCESS {
            return None;
        }
        Some(data != 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_disabled_global_switch_is_reported_instead_of_a_false_success() {
        assert_eq!(
            global_block_reason(Some(false)),
            Some("Windows 全局通知已关闭（设置 → 系统 → 通知）——请打开后再试")
        );
    }

    #[test]
    fn an_enabled_or_unknown_global_switch_does_not_block() {
        assert_eq!(global_block_reason(Some(true)), None);
        assert_eq!(global_block_reason(None), None);
    }
}
