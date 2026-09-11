//! Configurable model identity at the grouping and pricing boundary.

use serde::{Deserialize, Serialize};
use std::sync::LazyLock;

pub use toml::de::Error as ModelMappingsParseError;

/// Ordered user rules, evaluated before the bundled defaults.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelMappings {
    #[serde(default = "include_defaults")]
    pub include_defaults: bool,
    #[serde(default)]
    pub rules: Vec<ModelMappingRule>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RuleInput")]
pub struct ModelMappingRule {
    pattern: String,
    model: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RuleInput {
    pattern: String,
    model: String,
}

impl TryFrom<RuleInput> for ModelMappingRule {
    type Error = String;

    fn try_from(input: RuleInput) -> Result<Self, Self::Error> {
        if input.pattern.trim().is_empty() || input.model.trim().is_empty() {
            return Err("model mapping rules require non-empty pattern and model values".into());
        }
        Ok(Self {
            pattern: input.pattern,
            model: input.model,
        })
    }
}

const fn include_defaults() -> bool {
    true
}

impl Default for ModelMappings {
    fn default() -> Self {
        Self {
            include_defaults: true,
            rules: Vec::new(),
        }
    }
}

static DEFAULT_RULES: LazyLock<Vec<ModelMappingRule>> = LazyLock::new(|| {
    toml::from_str::<ModelMappings>(include_str!("../model-mappings.toml"))
        .expect("bundled model mappings must be valid")
        .rules
});

impl ModelMappings {
    pub fn from_toml(content: &str) -> Result<Self, ModelMappingsParseError> {
        toml::from_str(content)
    }

    /// Freeze the complete ordered rule list into generation-cache identity.
    pub(crate) fn resolved(mut self) -> Self {
        if self.include_defaults {
            self.rules.extend(DEFAULT_RULES.iter().cloned());
            self.include_defaults = false;
        }
        self
    }

    /// A writable override document with the bundled defaults shown as comments.
    pub fn example_toml() -> String {
        let mut content = String::from(
            "# Model mappings for aggregation, display, and pricing.\n\
             # Rules match full names, ignoring ASCII case; * matches any sequence.\n\
             # User rules run in order before defaults. The first match wins.\n\
             # Targets are final and are not mapped again.\n\
             # Set include_defaults to false to disable the mappings listed below.\n\
             # Syntax cleanup (route prefixes, dates, reasoning tiers) still applies\n\
             # to names not matched by a user rule.\n\
             # Bundled default rules:\n",
        );
        for line in include_str!("../model-mappings.toml").lines() {
            content.push_str("# ");
            content.push_str(line);
            content.push('\n');
        }
        content.push_str("\ninclude_defaults = true\n\n# Add overrides here, for example:\n# [[rules]]\n# pattern = \"my-private-model\"\n# model = \"deepseek-v4.1-flash\"\n");
        content
    }

    /// Syntax cleanup is shared with decoders. A matched target is final: no
    /// recursive mapping or further default normalization is applied to it.
    pub fn canonicalize(&self, observed: &str) -> String {
        self.canonicalize_observation(observed, observed)
    }

    pub(crate) fn canonicalize_observation(&self, observed: &str, decoded: &str) -> String {
        let terminal = crate::model_aliases::normalized_terminal_model_id(observed);
        let normalized_raw = crate::model_aliases::normalize_model_syntax(observed);
        let normalized = if observed == decoded {
            normalized_raw.clone()
        } else {
            crate::model_aliases::normalize_model_syntax(decoded)
        };
        let display_slug = crate::model_aliases::normalized_human_display_model_slug(&terminal);
        let mut candidates = vec![observed.trim(), &terminal, &normalized_raw, &normalized];
        if let Some(slug) = display_slug.as_deref() {
            candidates.push(slug);
        }
        // Compare each rule against all observed/decoded spellings before
        // advancing to the next rule, so array order is the sole priority.
        if let Some(target) = match_rules(&self.rules, &candidates) {
            return target.to_owned();
        }
        if self.include_defaults {
            if let Some(target) = match_rules(&DEFAULT_RULES, &candidates) {
                return target.to_owned();
            }
        }
        normalized
    }
}

fn match_rules<'a>(rules: &'a [ModelMappingRule], candidates: &[&str]) -> Option<&'a str> {
    rules
        .iter()
        .find(|rule| {
            candidates
                .iter()
                .any(|value| wildcard_matches(&rule.pattern, value))
        })
        .map(|rule| rule.model.as_str())
}

/// Full-string, ASCII-case-insensitive matching; only `*` is special.
fn wildcard_matches(pattern: &str, value: &str) -> bool {
    let (pattern, value) = (pattern.as_bytes(), value.as_bytes());
    let (mut p, mut v) = (0, 0);
    let mut star = None;
    let mut consumed = 0;
    while v < value.len() {
        if p < pattern.len() && pattern[p] == b'*' {
            star = Some(p);
            p += 1;
            consumed = v;
        } else if p < pattern.len() && pattern[p].eq_ignore_ascii_case(&value[v]) {
            p += 1;
            v += 1;
        } else if let Some(index) = star {
            consumed += 1;
            v = consumed;
            p = index + 1;
        } else {
            return false;
        }
    }
    while p < pattern.len() && pattern[p] == b'*' {
        p += 1;
    }
    p == pattern.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_config_and_toml_comments_parse() {
        assert_eq!(
            ModelMappings::from_toml(&ModelMappings::example_toml()).unwrap(),
            ModelMappings::default()
        );
        let mappings = ModelMappings::from_toml(
            r#"
            # Override a routed name.
            [[rules]]
            pattern = 'route/*'
            model = 'target' # Inline comment
        "#,
        )
        .unwrap();
        assert_eq!(mappings.canonicalize("route/private"), "target");
        assert!(ModelMappings::from_toml("[[rules] invalid").is_err());
    }

    #[test]
    fn configured_defaults_preserve_human_display_aliases() {
        for (observed, expected) in [
            ("Kimi K2.5 Thinking", "kimi-k2-thinking"),
            ("Grok Composer 2.5 Fast", "composer-2.5-fast"),
            ("GLM 4.7 High", "glm-4.7"),
            ("LongCat Flash 3B All Quant 0203 Eagle3", "longcat-flash-3b"),
        ] {
            let mappings = ModelMappings::default();
            assert_eq!(mappings.canonicalize(observed), expected);
            assert_eq!(mappings.resolved().canonicalize(observed), expected);
        }
    }

    #[test]
    fn defaults_and_user_override_share_one_boundary() {
        let mappings = ModelMappings::default();
        for model in [
            "deepseek-v4.1-pro",
            "deepseek-v4.1-flash",
            "DeepSeek/deepseek-flash",
        ] {
            assert_eq!(mappings.canonicalize(model), "deepseek-v4.1-flash");
        }
        assert_eq!(mappings.canonicalize("deepseek-v4-pro"), "deepseek-v4-pro");
        let mappings: ModelMappings = serde_json::from_value(serde_json::json!({
            "rules": [
                {"pattern": "gpt-5.6", "model": "gpt-5.6"},
                {"pattern": "deepseek-v4.1-pro", "model": "my-pro"},
                {"pattern": "deepseek-*", "model": "my-deepseek"}
            ]
        }))
        .unwrap();
        assert_eq!(mappings.canonicalize("gpt-5.6"), "gpt-5.6");
        assert_eq!(mappings.canonicalize("gpt-5.6-high"), "gpt-5.6");
        assert_eq!(mappings.canonicalize("deepseek-v4.1-pro"), "my-pro");
        assert_eq!(mappings.canonicalize("deepseek-flash"), "my-deepseek");
    }

    #[test]
    fn defaults_can_be_disabled_and_targets_are_not_remapped() {
        let mappings: ModelMappings = serde_json::from_value(serde_json::json!({
            "include_defaults": false,
            "rules": [{"pattern": "private", "model": "gpt-5.6"}]
        }))
        .unwrap();
        assert_eq!(mappings.canonicalize("private"), "gpt-5.6");
        assert_eq!(mappings.canonicalize("k2p5"), "k2p5");
        assert_eq!(mappings.canonicalize("deepseek-flash"), "deepseek-flash");
        assert_eq!(mappings.canonicalize("gpt-5.6-high"), "gpt-5.6");
    }

    #[test]
    fn wildcard_is_anchored_and_other_metacharacters_are_literal() {
        assert!(wildcard_matches("*seek*v4.1-*", "DeepSeek-v4.1-flash"));
        assert!(wildcard_matches("deepseek-*", "deepseek-"));
        assert!(!wildcard_matches("deepseek-*", "my-deepseek-flash"));
        assert!(!wildcard_matches("deepseek-?", "deepseek-x"));
        assert!(wildcard_matches("模型*", "模型一"));
    }

    #[test]
    fn invalid_configuration_is_an_error() {
        for value in [
            serde_json::json!({"rules": [{"pattern": "", "model": "target"}]}),
            serde_json::json!({"rules": [{"pattern": "*", "model": " "}]}),
            serde_json::json!({"rules": [{"pattern": "*", "model": "target", "typo": true}]}),
            serde_json::json!({"includeDefault": false}),
        ] {
            assert!(serde_json::from_value::<ModelMappings>(value).is_err());
        }
    }
}
