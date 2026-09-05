use serde::{Deserialize, Deserializer, Serialize};

/// Public catalogs. Custom overrides and DeepSeek V4 time-period prices have
/// fixed authority outside this configurable order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CatalogSource {
    #[serde(rename = "litellm")]
    Litellm,
    #[serde(rename = "openrouter")]
    Openrouter,
    #[serde(rename = "models.dev")]
    ModelsDev,
}

impl CatalogSource {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Litellm => "LiteLLM",
            Self::Openrouter => "OpenRouter",
            Self::ModelsDev => "models.dev",
        }
    }
}

/// A complete permutation of the supported public catalogs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct SourceOrder([CatalogSource; 3]);

impl Default for SourceOrder {
    fn default() -> Self {
        Self([
            CatalogSource::Litellm,
            CatalogSource::Openrouter,
            CatalogSource::ModelsDev,
        ])
    }
}

impl SourceOrder {
    pub fn sources(&self) -> &[CatalogSource; 3] {
        &self.0
    }

    pub fn swap(&mut self, first: usize, second: usize) {
        self.0.swap(first, second);
    }
}

impl<'de> Deserialize<'de> for SourceOrder {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let sources = <[CatalogSource; 3]>::deserialize(deserializer)?;
        if sources[0] == sources[1] || sources[0] == sources[2] || sources[1] == sources[2] {
            return Err(serde::de::Error::custom(
                "pricing source order must contain litellm, openrouter, and models.dev exactly once",
            ));
        }
        Ok(Self(sources))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_only_complete_permutations() {
        for invalid in [
            r#"[]"#,
            r#"["litellm","openrouter"]"#,
            r#"["litellm","litellm","models.dev"]"#,
            r#"["custom","openrouter","models.dev"]"#,
            r#"["LiteLLM","openrouter","models.dev"]"#,
            r#"["litellm","openrouter","models.dev","litellm"]"#,
        ] {
            assert!(
                serde_json::from_str::<SourceOrder>(invalid).is_err(),
                "{invalid}"
            );
        }
        let json = r#"["models.dev","openrouter","litellm"]"#;
        let order = serde_json::from_str::<SourceOrder>(json).unwrap();
        assert_eq!(serde_json::to_string(&order).unwrap(), json);
        assert_eq!(
            bincode::deserialize::<SourceOrder>(&bincode::serialize(&order).unwrap()).unwrap(),
            order
        );
    }
}
