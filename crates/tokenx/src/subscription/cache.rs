use anyhow::{Context, Result};

use super::SubscriptionOutput;

const CACHE_SCHEMA: &str = "tokenx.subscription-usage";
const CACHE_VERSION: u32 = 2;
const CACHE_MAX_AGE_SECS: u64 = 300;

#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct CacheEnvelope {
    schema: String,
    version: u32,
    timestamp: u64,
    #[serde(default)]
    locale: Option<String>,
    data: Vec<SubscriptionOutput>,
}

fn current_locale() -> String {
    rust_i18n::locale().to_string()
}

fn current_unix_timestamp() -> Result<u64> {
    Ok(std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .context(rust_i18n::t!("subscription.error.cache_clock_before_epoch"))?
        .as_secs())
}

pub(crate) fn save(path: &std::path::Path, data: &[SubscriptionOutput]) -> Result<()> {
    let locale = current_locale();
    save_at(path, data, current_unix_timestamp()?, &locale)
}

fn save_at(
    path: &std::path::Path,
    data: &[SubscriptionOutput],
    timestamp: u64,
    locale: &str,
) -> Result<()> {
    let envelope = CacheEnvelope {
        schema: CACHE_SCHEMA.to_string(),
        version: CACHE_VERSION,
        timestamp,
        locale: Some(locale.to_string()),
        data: data.to_vec(),
    };
    let bytes = serde_json::to_vec(&envelope)
        .with_context(|| rust_i18n::t!("subscription.error.cache_serialize", locale = locale))?;
    tokenx_engine::fs_atomic::write_atomic(path, &bytes).with_context(|| {
        rust_i18n::t!(
            "subscription.error.cache_persist",
            locale = locale,
            path = path.display()
        )
    })
}

#[cfg_attr(test, allow(dead_code))]
pub(crate) fn load(path: &std::path::Path) -> Result<Option<Vec<SubscriptionOutput>>> {
    let locale = current_locale();
    load_at(path, current_unix_timestamp()?, &locale)
}

fn load_at(
    path: &std::path::Path,
    now: u64,
    locale: &str,
) -> Result<Option<Vec<SubscriptionOutput>>> {
    let content = match std::fs::read(path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| {
                rust_i18n::t!(
                    "subscription.error.cache_read",
                    locale = locale,
                    path = path.display()
                )
            })
        }
    };
    let envelope: CacheEnvelope = serde_json::from_slice(&content).with_context(|| {
        rust_i18n::t!(
            "subscription.error.cache_malformed",
            locale = locale,
            path = path.display()
        )
    })?;
    if envelope.schema != CACHE_SCHEMA {
        anyhow::bail!(rust_i18n::t!(
            "subscription.error.cache_unsupported_schema",
            locale = locale,
            path = path.display(),
            schema = envelope.schema
        ));
    }
    if envelope.version != CACHE_VERSION {
        anyhow::bail!(rust_i18n::t!(
            "subscription.error.cache_unsupported_version",
            locale = locale,
            path = path.display(),
            version = envelope.version
        ));
    }
    let cache_locale = envelope.locale.ok_or_else(|| {
        anyhow::anyhow!(rust_i18n::t!(
            "subscription.error.cache_missing_locale",
            locale = locale,
            path = path.display()
        ))
    })?;
    if cache_locale != locale {
        return Ok(None);
    }
    if envelope.timestamp > now {
        anyhow::bail!(rust_i18n::t!(
            "subscription.error.cache_future_timestamp",
            locale = locale,
            path = path.display(),
            timestamp = envelope.timestamp,
            now = now
        ));
    }
    if now.saturating_sub(envelope.timestamp) > CACHE_MAX_AGE_SECS {
        return Ok(None);
    }
    Ok(Some(envelope.data))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::subscription::{ProviderId, UsageAccount, UsageMetric};

    fn output() -> SubscriptionOutput {
        SubscriptionOutput {
            provider: ProviderId::Codex,
            stale: false,
            account: Some(UsageAccount {
                id: "account-1".to_string(),
                label: Some("Work".to_string()),
                is_active: true,
            }),
            plan: Some("Pro".to_string()),
            email: Some("work@example.com".to_string()),
            metrics: vec![UsageMetric {
                label: "Weekly".to_string(),
                used_percent: 20.0,
                remaining_percent: 80.0,
                remaining_label: Some("80% left".to_string()),
                resets_at: Some("2026-07-30T00:00:00Z".to_string()),
            }],
        }
    }

    #[test]
    fn future_cache_timestamp_is_rejected_instead_of_treated_as_fresh() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let path = temp.path().join("subscription-usage-cache.json");
        save_at(&path, &[output()], 1_001, "en")?;

        let error = load_at(&path, 1_000, "en").unwrap_err();
        assert!(error.to_string().contains("future timestamp"));
        Ok(())
    }

    #[test]
    fn round_trip_records_locale_and_uses_typed_provider_identity() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let path = temp.path().join("subscription-usage-cache.json");
        let output = output();
        save_at(&path, std::slice::from_ref(&output), 1_000, "en")?;

        let value: serde_json::Value = serde_json::from_slice(&std::fs::read(&path)?)?;
        assert_eq!(value["schema"], CACHE_SCHEMA);
        assert_eq!(value["version"], CACHE_VERSION);
        assert_eq!(value["locale"], "en");
        assert_eq!(value["data"][0]["provider"], "codex");
        assert_eq!(load_at(&path, 1_300, "en")?, Some(vec![output]));
        assert_eq!(load_at(&path, 1_301, "en")?, None);
        Ok(())
    }

    #[test]
    fn cache_from_another_locale_is_invalidated_without_returning_data() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let path = temp.path().join("subscription-usage-cache.json");
        save_at(&path, &[output()], 1_000, "en")?;

        assert_eq!(load_at(&path, 1_300, "zh-CN")?, None);
        Ok(())
    }

    #[test]
    fn legacy_v1_cache_is_rejected_as_unsupported() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let path = temp.path().join("subscription-usage-cache.json");
        let envelope = serde_json::json!({
            "schema": CACHE_SCHEMA,
            "version": 1,
            "timestamp": 1_000,
            "data": [{
                "provider": "codex",
                "plan": null,
                "email": null,
                "metrics": []
            }]
        });
        std::fs::write(&path, serde_json::to_vec(&envelope)?)?;

        let error = load_at(&path, 1_000, "en").unwrap_err();
        assert!(error.to_string().contains("unsupported version 1"));
        Ok(())
    }

    #[test]
    fn current_cache_without_locale_is_rejected() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let path = temp.path().join("subscription-usage-cache.json");
        let envelope = serde_json::json!({
            "schema": CACHE_SCHEMA,
            "version": CACHE_VERSION,
            "timestamp": 1_000,
            "data": [{
                "provider": "codex",
                "plan": null,
                "email": null,
                "metrics": []
            }]
        });
        std::fs::write(&path, serde_json::to_vec(&envelope)?)?;

        let error = load_at(&path, 1_000, "en").unwrap_err();
        assert!(error.to_string().contains("missing its locale marker"));
        Ok(())
    }

    #[test]
    fn unsupported_cache_version_is_rejected() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let path = temp.path().join("subscription-usage-cache.json");
        let unsupported_version = CACHE_VERSION + 1;
        let envelope = serde_json::json!({
            "schema": CACHE_SCHEMA,
            "version": unsupported_version,
            "timestamp": 1_000,
            "locale": "en",
            "data": [{
                "provider": "codex",
                "plan": null,
                "email": null,
                "metrics": []
            }]
        });
        std::fs::write(&path, serde_json::to_vec(&envelope)?)?;

        let error = load_at(&path, 1_000, "en").unwrap_err();
        assert!(error
            .to_string()
            .contains(&format!("unsupported version {unsupported_version}")));
        Ok(())
    }
}
