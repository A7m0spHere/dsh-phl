//! Validation rules for provider identifiers, environment names and model capabilities.

use super::types::{ModelRef, ReasoningEfforts};

impl ModelRef {
    pub(crate) fn validate_capabilities(&self) -> Result<(), String> {
        if self.context_window == Some(0) || self.max_tokens == Some(0) {
            return Err(format!("模型 {} 的上下文和最大输出必须是正整数", self.id));
        }
        if let Some(ReasoningEfforts::Levels(levels)) = &self.reasoning_efforts {
            let supported = ["off", "minimal", "low", "medium", "high", "xhigh", "max"];
            if !levels.keys().any(|k| k != "off")
                || levels.iter().any(|(key, wire)| {
                    !supported.contains(&key.as_str())
                        || match wire {
                            None => key != "off",
                            Some(value) => value.trim().is_empty(),
                        }
                })
            {
                return Err(format!("模型 {} 的思考档位无效：至少声明一个思考档位，仅 off 可使用 null；或设为 false / 留空", self.id));
            }
        }
        Ok(())
    }
}

/* ------------------------------ validation ----------------------------- */

/// Env-var-name grammar. DSH does `process.env[apiKeyEnv]`, so a lowercase or
/// space-laden name would silently read as undefined; reject it at the UI
/// boundary instead.
pub(crate) fn valid_env_name(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first.is_ascii_alphabetic() || first == '_')
        && name.len() <= 64
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// The provider name is pasted straight into a YAML key; keep it to the
/// characters DSH's own providers use.
pub(crate) fn valid_provider_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}
