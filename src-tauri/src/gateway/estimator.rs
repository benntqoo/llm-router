//! 本地 token 估算器（T12；01 §5.5「tokenizer 选型」、02 §8.1 估算入口）。
//!
//! 用 **tiktoken-rs** 做本地分词：`cl100k_base`（默认）/ `o200k_base`。
//! 编码表由 crate 内嵌（`assets/*.tiktoken`），**运行期不联网**。
//!
//! 两条硬约束（01 §5.5、02 §8.1）：
//!
//! 1. **绝不用估算值冒充精确值**——上游给了的 usage 一律不被估算覆盖；
//! 2. **国产模型只能近似**——tiktoken 的编码与国产模型真实分词不一致，
//!    估算值必须带 `estimated=1`（由调用方打标）。

use tiktoken_rs::{cl100k_base_singleton, o200k_base_singleton, CoreBPE};

use crate::gateway::usage::{TokenCounts, TokenEstimator};

/// 使用的 tiktoken 编码。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Encoding {
    /// `cl100k_base`：GPT-4 及更早；**所有国产模型也用它**（对国产模型只是近似）。
    Cl100kBase,
    /// `o200k_base`：GPT-4o / GPT-4.1 / GPT-5 / o 系列。
    O200kBase,
}

/// 按上游模型名选编码。
///
/// `o200k_base` 家族按 tiktoken-rs 的官方说明划分：gpt-5 / gpt-4.1 / gpt-4o /
/// o1 / o3 / o4 / gpt-oss。其余（含全部国产模型）一律 `cl100k_base`。
pub fn encoding_for(model: &str) -> Encoding {
    let m = model.trim().to_ascii_lowercase();
    let is_o200k = m.starts_with("gpt-4o")
        || m.starts_with("gpt-4.1")
        || m.starts_with("gpt-5")
        || m.starts_with("gpt-oss")
        || m == "o1"
        || m.starts_with("o1-")
        || m == "o3"
        || m.starts_with("o3-")
        || m == "o4"
        || m.starts_with("o4-");
    if is_o200k {
        Encoding::O200kBase
    } else {
        Encoding::Cl100kBase
    }
}

/// 该模型是否**只能近似**估算。
///
/// 只有 OpenAI 家族（其真实分词就是 tiktoken 的 cl100k/o200k）才接近精确；
/// **国产模型与自定义兼容渠道一律视为近似**（01 §5.5 规则 3）。
///
/// 用途：T13 的成本换算与 T14 的 UI 展示据此提示"这是估算值"，
/// 并保证估算值不参与预算告警（R7）。
pub fn is_approximation(model: &str) -> bool {
    let m = model.trim().to_ascii_lowercase();
    let known_openai = m.starts_with("gpt-")
        || m.starts_with("gpt-oss")
        || m == "o1"
        || m.starts_with("o1-")
        || m == "o3"
        || m.starts_with("o3-")
        || m == "o4"
        || m.starts_with("o4-");
    !known_openai
}

/// 取该模型对应的编码表（懒加载单例，首次调用时解析 BPE 表）。
fn bpe_for(model: &str) -> &'static CoreBPE {
    match encoding_for(model) {
        Encoding::Cl100kBase => cl100k_base_singleton(),
        Encoding::O200kBase => o200k_base_singleton(),
    }
}

/// tiktoken-rs 估算器（T12 的产线上默认估算器）。
///
/// 计数口径：`count_with_special_tokens`——把 `<|im_start|>` 之类的特殊标记
/// 也算进去，与 OpenAI 侧的计数口径一致。
pub struct TiktokenEstimator;

impl std::fmt::Debug for TiktokenEstimator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TiktokenEstimator").finish_non_exhaustive()
    }
}

impl TokenEstimator for TiktokenEstimator {
    fn estimate(&self, model: &str, prompt: &str, completion: &str) -> Option<TokenCounts> {
        let bpe = bpe_for(model);
        Some(TokenCounts {
            prompt_tokens: bpe.count_with_special_tokens(prompt) as i64,
            completion_tokens: bpe.count_with_special_tokens(completion) as i64,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::usage::CharRatioEstimator;

    #[test]
    fn o200k_family_uses_o200k_encoding() {
        for model in [
            "gpt-4o",
            "gpt-4o-mini",
            "gpt-4.1",
            "gpt-5",
            "o1",
            "o1-preview",
            "o3-mini",
            "o4-mini",
            "gpt-oss-20b",
        ] {
            assert_eq!(
                encoding_for(model),
                Encoding::O200kBase,
                "{model} 应走 o200k_base"
            );
        }
    }

    #[test]
    fn other_models_use_cl100k_encoding() {
        for model in [
            "gpt-4",
            "gpt-4-turbo",
            "gpt-3.5-turbo",
            "deepseek-chat",
            "deepseek-reasoner",
            "glm-4",
            "moonshot-v1-8k",
            "MiniMax-Text-01",
            "qwen-plus",
        ] {
            assert_eq!(
                encoding_for(model),
                Encoding::Cl100kBase,
                "{model} 应走 cl100k_base"
            );
        }
    }

    #[test]
    fn domestic_models_are_approximations() {
        for model in [
            "deepseek-chat",
            "deepseek-reasoner",
            "glm-4-plus",
            "moonshot-v1-32k",
            "MiniMax-Text-01",
            "qwen-max",
            "my-local-model",
        ] {
            assert!(
                is_approximation(model),
                "{model} 只能近似估算（01 §5.5 规则 3）"
            );
        }
        assert!(!is_approximation("gpt-4o"));
        assert!(!is_approximation("gpt-4"));
    }

    #[test]
    fn estimates_non_empty_text() {
        let est = TiktokenEstimator;
        let counts = est.estimate("deepseek-chat", "你好，世界", "hi").unwrap();
        assert!(counts.prompt_tokens > 0, "中文提示应估出正数");
        assert!(counts.completion_tokens > 0, "输出应估出正数");
    }

    #[test]
    fn empty_text_is_zero_tokens() {
        let est = TiktokenEstimator;
        let counts = est.estimate("deepseek-chat", "", "").unwrap();
        assert_eq!(counts.prompt_tokens, 0);
        assert_eq!(counts.completion_tokens, 0);
    }

    #[test]
    fn differs_from_char_ratio_heuristic() {
        // 选一段两种算法结论明显不同的文本（emoji 在 char 启发式里算 1 个 token，
        // 在 cl100k 里是多个），保证"换了实现"这件事测得出来。
        let text = "🌈🌈🌈🌈🌈🌈";
        let tik = TiktokenEstimator
            .estimate("deepseek-chat", text, "")
            .unwrap();
        let chr = CharRatioEstimator
            .estimate("deepseek-chat", text, "")
            .unwrap();
        assert_ne!(
            tik.prompt_tokens, chr.prompt_tokens,
            "用例必须能区分 tiktoken 与字符启发式"
        );
    }

    #[test]
    fn same_text_counts_stably() {
        let est = TiktokenEstimator;
        let a = est.estimate("deepseek-chat", "重复性检查", "重复性检查").unwrap();
        let b = est.estimate("deepseek-chat", "重复性检查", "重复性检查").unwrap();
        assert_eq!(a, b, "同一段文本的估算必须稳定");
    }
}
