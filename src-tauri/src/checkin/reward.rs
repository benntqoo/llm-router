//! 签到积分（T18；02 §7.4 §4）。
//!
//! **积分是「本次拿到了多少」的快照，不是账户余额真值**——账户总额由取数方
//! （额度探测）负责，签到侧只写自己那笔。
//!
//! 密文边界（AGENT.md §5.3）：`raw` **只保留数值与布尔字段**——响应体里可能
//! 带 csrf / token 一类密文，字符串一律丢掉，不进 `checkin_result`。

use serde::{Deserialize, Serialize};

/// 一次签到拿到的积分。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Reward {
    /// 数量。
    pub amount: i64,
    /// 单位（`magicube` = 魔粒；`point` = MiniMax 积分）。
    pub unit: String,
    /// 站点自己的原始字段（**已过滤**：只有数值与布尔）。
    pub raw: serde_json::Value,
}

impl Reward {
    /// 构造（`raw` 走一遍过滤）。
    pub fn new(amount: i64, unit: &str, raw: serde_json::Value) -> Self {
        Reward {
            amount,
            unit: unit.to_string(),
            raw: sanitize_raw(raw),
        }
    }

    /// 落库用的 JSON 文本（进 `checkin_result.reward_json`）。
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| "{}".to_string())
    }
}

/// 只保留**数值与布尔**字段（字符串可能是密文，一律丢掉）。
///
/// 数组按元素递归；对象按 key 递归（key 保留，值过滤）。
fn sanitize_raw(value: serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Number(_) | serde_json::Value::Bool(_) => value,
        serde_json::Value::Array(items) => serde_json::Value::Array(
            items.into_iter().map(sanitize_raw).collect(),
        ),
        serde_json::Value::Object(map) => {
            let mut out = serde_json::Map::new();
            for (key, item) in map {
                let filtered = sanitize_raw(item);
                // 只留「过滤后仍是个标量」的字段；对象/数组里被过滤空了就丢掉
                if !filtered.is_null() {
                    out.insert(key, filtered);
                }
            }
            serde_json::Value::Object(out)
        }
        serde_json::Value::String(_) | serde_json::Value::Null => serde_json::Value::Null,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn raw_keeps_numbers_and_drops_strings() {
        let reward = Reward::new(
            400,
            "point",
            json!({"points": 400, "day": 4, "csrf_token": "SECRET-VALUE"}),
        );
        assert_eq!(reward.amount, 400);
        assert_eq!(reward.unit, "point");
        assert_eq!(reward.raw["points"], json!(400));
        assert_eq!(reward.raw["day"], json!(4));
        assert!(
            !reward.raw.to_string().contains("SECRET-VALUE"),
            "字符串（可能是密文）不得进 reward_json"
        );
        assert!(reward.raw.get("csrf_token").is_none());
    }

    #[test]
    fn nested_objects_are_filtered_recursively() {
        let reward = Reward::new(
            1,
            "magicube",
            json!({"data": {"balance": 1200, "token": "SECRET"}, "list": [{"amount": 400}]}),
        );
        assert_eq!(reward.raw["data"]["balance"], json!(1200));
        assert!(reward.raw["data"].get("token").is_none());
        assert_eq!(reward.raw["list"][0]["amount"], json!(400));
    }

    #[test]
    fn json_text_is_stable() {
        let reward = Reward::new(400, "point", json!({"points": 400}));
        let text = reward.to_json();
        assert!(text.contains("\"amount\":400"), "{text}");
        assert!(text.contains("\"unit\":\"point\""), "{text}");
    }
}
