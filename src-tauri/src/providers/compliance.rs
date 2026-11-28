//! 合规确认与渠道启用（02 §6.8；T24 Copilot 半边）。
//!
//! AGENT.md §5.5：**Copilot / WorkBuddy 反代默认关闭，首次启用强制 ToS 确认弹窗**。
//!
//! 本模块是**纯数据 + 纯函数**（无 IO、可单测）：
//! - **受限 provider 清单**（本期只有 `copilot`）；
//! - **ToS 声明原文**（**必须逐字进 UI 弹窗**）；
//! - **启用前校验**（没确认过 = 不许启用）。
//!
//! **合规不靠前端**：后端每一个「把渠道置为启用」的入口都要过 [`ensure_can_enable`] ——
//! 否则通用渠道表单里的一个勾选框就能绕过弹窗。

use crate::providers::copilot::PROVIDER as COPILOT_PROVIDER;

/// 受限 provider（反代额度型：**默认关闭 + 首次启用要过 ToS 确认**）。
///
/// WorkBuddy 半边**后置**（T23 延后到付费后，02 §6.3.1 §5.1）→ 届时在此加一行 + 一段声明。
pub const RESTRICTED_PROVIDERS: &[&str] = &[COPILOT_PROVIDER];

/// Copilot 的 ToS 声明原文（**必须逐字进弹窗**；02 §6.2 / §6.8.2）。
pub const COPILOT_TOS_STATEMENT: &str = "将 Copilot 额度用于第三方客户端违反 GitHub Copilot ToS，账号可能受限。用户显式确认后才启用。";

/// Copilot 声明的版本号（**改文案就要升版本** → 旧确认失效 → 要重新确认）。
pub const COPILOT_TOS_VERSION: &str = "copilot-tos-2026-11-28";

/// 该 provider 是不是**受限**（默认关闭 + 启用前要过 ToS）。
pub fn is_restricted(provider: &str) -> bool {
    let provider = provider.trim();
    RESTRICTED_PROVIDERS
        .iter()
        .any(|p| p.eq_ignore_ascii_case(provider))
}

/// 该 provider 的 ToS 声明原文（**非受限 provider 返回 `None`**）。
pub fn statement_for(provider: &str) -> Option<&'static str> {
    let provider = provider.trim();
    if provider.eq_ignore_ascii_case(COPILOT_PROVIDER) {
        Some(COPILOT_TOS_STATEMENT)
    } else {
        None
    }
}

/// 该 provider 的声明版本（**非受限 provider 返回 `None`**）。
pub fn version_for(provider: &str) -> Option<&'static str> {
    let provider = provider.trim();
    if provider.eq_ignore_ascii_case(COPILOT_PROVIDER) {
        Some(COPILOT_TOS_VERSION)
    } else {
        None
    }
}

/// 用户**该去哪里**确认 ToS（错误信息必须指路，不能只说「失败」）。
pub fn panel_hint(provider: &str) -> &'static str {
    let provider = provider.trim();
    if provider.eq_ignore_ascii_case(COPILOT_PROVIDER) {
        "请先在「GitHub Copilot 授权」面板确认 ToS 后再启用"
    } else {
        "请先确认该渠道的服务条款后再启用"
    }
}

/// 已有的确认记录**是否算数**。
///
/// - `accepted_at` 为 `None` → **没确认过**；
/// - 声明版本**对不上** → **没确认过**（文案改过 → 旧确认作废）。
pub fn acceptance_is_valid(
    provider: &str,
    accepted_at: Option<i64>,
    accepted_version: Option<&str>,
) -> bool {
    let Some(current) = version_for(provider) else {
        // 非受限 provider：没有 ToS 这回事 → 不拦
        return true;
    };
    if accepted_at.is_none() {
        return false;
    }
    match accepted_version {
        Some(version) if version.trim() == current => true,
        _ => false,
    }
}

/// **启用前的合规闸门**：不通过就返回一条**指路**的错误。
///
/// - 非受限 provider → 一律放行；
/// - 受限 provider 且**没有有效确认** → 拒绝，并告诉用户去哪里确认。
///
/// `accepted_at` / `accepted_version` 取自**库里的**渠道行（不是调用方传的），
/// 这样前端**伪造不了**「已确认」。
pub fn ensure_can_enable(
    provider: &str,
    accepted_at: Option<i64>,
    accepted_version: Option<&str>,
) -> Result<(), String> {
    if acceptance_is_valid(provider, accepted_at, accepted_version) {
        return Ok(());
    }
    let provider = provider.trim();
    let reason = if accepted_at.is_some() {
        "ToS 声明已更新，需要重新确认"
    } else {
        "尚未确认 ToS"
    };
    Err(format!(
        "该渠道（provider = {provider}）{reason}——{}。",
        panel_hint(provider)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copilot_is_restricted_and_custom_is_not() {
        assert!(is_restricted("copilot"));
        assert!(is_restricted("  Copilot "));
        assert!(!is_restricted("custom"));
        assert!(!is_restricted("deepseek"));
        assert!(!is_restricted(""));
    }

    #[test]
    fn statement_and_version_are_available_for_copilot_only() {
        assert_eq!(statement_for("copilot"), Some(COPILOT_TOS_STATEMENT));
        assert_eq!(version_for("copilot"), Some(COPILOT_TOS_VERSION));
        assert_eq!(statement_for("custom"), None);
        assert_eq!(version_for("custom"), None);
    }

    #[test]
    fn the_statement_text_is_the_one_the_design_doc_mandates() {
        // 02 §6.2 / §6.8.2：声明必须逐字进弹窗 —— 改文案要同步升版本
        assert!(COPILOT_TOS_STATEMENT.contains("违反 GitHub Copilot ToS"));
        assert!(COPILOT_TOS_STATEMENT.contains("账号可能受限"));
    }

    #[test]
    fn a_restricted_provider_cannot_be_enabled_without_acceptance() {
        let err = ensure_can_enable("copilot", None, None).expect_err("未确认不该放行");
        assert!(err.contains("尚未确认 ToS"), "{err}");
        assert!(err.contains("GitHub Copilot 授权"), "错误要指路：{err}");
    }

    #[test]
    fn a_valid_acceptance_lets_the_provider_be_enabled() {
        ensure_can_enable("copilot", Some(1_700_000_000), Some(COPILOT_TOS_VERSION))
            .expect("确认过就该放行");
    }

    #[test]
    fn a_stale_statement_version_is_treated_as_no_acceptance() {
        let err = ensure_can_enable("copilot", Some(1_700_000_000), Some("copilot-tos-2020-01-01"))
            .expect_err("旧版本的确认不该算数");
        assert!(err.contains("已更新"), "{err}");
    }

    #[test]
    fn unrestricted_providers_are_never_blocked() {
        ensure_can_enable("custom", None, None).expect("自定义渠道不受合规限制");
        ensure_can_enable("deepseek", None, None).expect("预设渠道不受合规限制");
    }

    #[test]
    fn acceptance_is_valid_matches_the_ensure_rule() {
        assert!(acceptance_is_valid("copilot", None, None) == false);
        assert!(acceptance_is_valid("copilot", Some(1), None) == false);
        assert!(acceptance_is_valid("copilot", Some(1), Some(COPILOT_TOS_VERSION)));
        assert!(acceptance_is_valid("custom", None, None));
    }
}
