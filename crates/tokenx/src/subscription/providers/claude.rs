use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::{Path, PathBuf};

use super::helpers::capitalize;
use super::{SubscriptionIssue, SubscriptionIssueCode, SubscriptionPayload, UsageMetric};

const BETA_HEADER: &str = "oauth-2025-04-20";

#[derive(Debug, Deserialize)]
struct Credentials {
    #[serde(rename = "claudeAiOauth")]
    claude_ai_oauth: Option<Oauth>,
}

#[derive(Debug, Deserialize)]
struct Oauth {
    #[serde(rename = "accessToken")]
    access_token: Option<String>,
    #[serde(rename = "expiresAt")]
    expires_at: Option<i64>,
    #[serde(rename = "subscriptionType")]
    subscription_type: Option<String>,
    #[serde(rename = "rateLimitTier")]
    rate_limit_tier: Option<String>,
}

#[derive(Debug, Deserialize)]
struct UsageResponse {
    five_hour: Option<Window>,
    seven_day: Option<Window>,
    seven_day_opus: Option<Window>,
}

#[derive(Debug, Deserialize)]
struct Window {
    utilization: f64,
    resets_at: Option<String>,
}

fn read_keychain() -> Result<Option<String>> {
    super::helpers::read_keychain("Claude Code-credentials")
}

fn credentials_path_for_home(home: Option<&Path>) -> Option<PathBuf> {
    home.map(|home| home.join(".claude").join(".credentials.json"))
}

fn credentials_path() -> Option<PathBuf> {
    credentials_path_for_home(dirs::home_dir().as_deref())
}

fn no_home_or_keychain_issue(error: &anyhow::Error) -> SubscriptionIssue {
    SubscriptionIssue::new(
        SubscriptionIssueCode::ClaudeNoHomeOrKeychain,
        format!(
            "No Claude credentials found: the home directory is unavailable and the keychain lookup failed: {error}"
        ),
    )
    .with_cause(SubscriptionIssue::from_anyhow(error))
}

fn read_credentials() -> Result<Credentials> {
    let path = credentials_path();
    read_credentials_from_sources(
        cfg!(target_os = "macos"),
        chrono::Utc::now().timestamp_millis(),
        || {
            let content = read_keychain().map_err(|error| {
                if path.is_none() {
                    anyhow::Error::new(no_home_or_keychain_issue(&error))
                } else {
                    error
                }
            })?;
            content
                .map(|text| {
                    serde_json::from_str(&text).with_context(|| {
                        SubscriptionIssue::new(SubscriptionIssueCode::ClaudeParseCredentials,
                    "Failed to parse Claude credentials from Keychain:Claude Code-credentials")
                    .with_field("path", "Keychain:Claude Code-credentials")
                    })
                })
                .transpose()
        },
        || read_file_credentials(path.as_deref()),
    )
}

fn read_file_credentials(path: Option<&Path>) -> Result<Option<Credentials>> {
    let Some(path) = path else {
        return Ok(None);
    };
    let content = match std::fs::read_to_string(path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| {
                SubscriptionIssue::new(
                    SubscriptionIssueCode::ClaudeReadCredentials,
                    format!("Failed to read Claude credentials from {}", path.display()),
                )
                .with_field("path", path.display())
            })
        }
    };
    serde_json::from_str::<Credentials>(&content)
        .map(Some)
        .with_context(|| {
            SubscriptionIssue::new(
                SubscriptionIssueCode::ClaudeParseCredentials,
                format!("Failed to parse Claude credentials from {}", path.display()),
            )
            .with_field("path", path.display())
        })
}

fn validate_access_token(credentials: &Credentials) -> Result<&Oauth> {
    let oauth = credentials.claude_ai_oauth.as_ref().ok_or_else(|| {
        anyhow::Error::new(SubscriptionIssue::new(
            SubscriptionIssueCode::ClaudeNoOauth,
            "No Claude OAuth credentials. Run 'claude' to log in.",
        ))
    })?;
    if oauth
        .access_token
        .as_deref()
        .is_none_or(|token| token.trim().is_empty())
    {
        return Err(anyhow::Error::new(SubscriptionIssue::new(
            SubscriptionIssueCode::ClaudeNoAccessToken,
            "No Claude access token.",
        )));
    }
    Ok(oauth)
}

fn read_credentials_from_sources(
    macos: bool,
    now_ms: i64,
    keychain: impl FnOnce() -> Result<Option<Credentials>>,
    file: impl FnOnce() -> Result<Option<Credentials>>,
) -> Result<Credentials> {
    let mut expired_keychain = false;
    if macos {
        if let Some(credentials) = keychain()? {
            let oauth = validate_access_token(&credentials)?;
            if oauth.expires_at.is_none_or(|expiry| expiry > now_ms) {
                return Ok(credentials);
            }
            expired_keychain = true;
        }
    }
    let credentials = file()?;
    if let Some(credentials) = credentials {
        let oauth = validate_access_token(&credentials)?;
        if oauth.expires_at.is_none_or(|expiry| expiry > now_ms) {
            return Ok(credentials);
        }
    } else if !expired_keychain {
        return Err(anyhow::Error::new(SubscriptionIssue::new(
            SubscriptionIssueCode::ClaudeNoCredentials,
            "No Claude credentials found in the current provider locations",
        )));
    }
    Err(anyhow::Error::new(SubscriptionIssue::new(SubscriptionIssueCode::ClaudeCredentialExpired,
        "Claude credentials have expired. Run `claude` to refresh the provider-owned authentication.")))
}

async fn fetch_usage(client: &reqwest::Client, token: &str) -> Result<UsageResponse> {
    let resp = client
        .get("https://api.anthropic.com/api/oauth/usage")
        .header("Authorization", format!("Bearer {token}"))
        .header("Accept", "application/json")
        .header("Content-Type", "application/json")
        .header("anthropic-beta", BETA_HEADER)
        .send()
        .await?;
    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        return Err(anyhow::Error::new(SubscriptionIssue::new(
            SubscriptionIssueCode::ClaudeRejected,
            "Claude credentials were rejected. Run `claude` to refresh the provider-owned authentication.",
        )));
    }
    if !status.is_success() {
        return Err(anyhow::Error::new(
            SubscriptionIssue::new(
                SubscriptionIssueCode::ClaudeRequestFailed,
                format!("Claude usage request failed (HTTP {status})"),
            )
            .with_field("status", status),
        ));
    }
    Ok(resp.json().await?)
}

fn window_metric(label: &str, w: &Window) -> UsageMetric {
    let used = w.utilization.clamp(0.0, 100.0);
    UsageMetric {
        label: label.into(),
        used_percent: used,
        remaining_percent: 100.0 - used,
        remaining_label: None,
        resets_at: w.resets_at.clone(),
    }
}

pub async fn fetch(client: &reqwest::Client) -> Result<SubscriptionPayload> {
    let creds = read_credentials()?;
    let oauth = validate_access_token(&creds)?;
    let access_token = oauth
        .access_token
        .as_deref()
        .expect("validated Claude access token");
    let plan = oauth.subscription_type.as_ref().map(|s| {
        let tier = oauth
            .rate_limit_tier
            .as_deref()
            .and_then(|t| t.rsplit('_').next());
        match tier {
            Some(mult) => format!("{} {}", capitalize(s), mult),
            None => capitalize(s),
        }
    });

    let resp = fetch_usage(client, access_token).await?;

    let mut metrics = Vec::new();
    if let Some(ref w) = resp.five_hour {
        metrics.push(window_metric("Session", w));
    }
    if let Some(ref w) = resp.seven_day {
        metrics.push(window_metric("Weekly", w));
    }
    if let Some(ref w) = resp.seven_day_opus {
        metrics.push(window_metric("Opus", w));
    }

    Ok(SubscriptionPayload {
        account: None,
        plan,
        email: None,
        metrics,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credentials_path_uses_the_current_claude_location() {
        assert_eq!(
            credentials_path_for_home(Some(Path::new("/home/tester"))),
            Some(PathBuf::from("/home/tester/.claude/.credentials.json"))
        );
    }

    #[test]
    fn missing_home_does_not_create_a_relative_credentials_path() {
        assert_eq!(credentials_path_for_home(None), None);
    }

    #[test]
    fn missing_home_preserves_the_typed_keychain_failure() {
        let keychain_issue = SubscriptionIssue::new(
            SubscriptionIssueCode::KeychainLookupFailed,
            "Keychain lookup failed for service 'Claude Code-credentials'",
        )
        .with_field("service", "Claude Code-credentials");
        let error = anyhow::Error::new(keychain_issue);

        let issue = no_home_or_keychain_issue(&error);

        assert_eq!(issue.code(), SubscriptionIssueCode::ClaudeNoHomeOrKeychain);
        let cause = issue.cause().expect("typed keychain cause");
        assert_eq!(cause.code(), SubscriptionIssueCode::KeychainLookupFailed);
        assert_eq!(cause.field("service"), Some("Claude Code-credentials"));
    }
    fn credential(token: &str, expiry: Option<i64>) -> Credentials {
        serde_json::from_value(
            serde_json::json!({"claudeAiOauth": {"accessToken":token,"expiresAt":expiry}}),
        )
        .unwrap()
    }

    #[test]
    fn durable_keychain_login_wins_without_reading_a_longer_lived_file() {
        for expiry in [Some(2000), None] {
            let selected = read_credentials_from_sources(
                true,
                1000,
                || Ok(Some(credential("keychain", expiry))),
                || panic!("file must not override the current Keychain account"),
            )
            .unwrap();
            assert_eq!(
                validate_access_token(&selected)
                    .unwrap()
                    .access_token
                    .as_deref(),
                Some("keychain")
            );
        }
    }

    #[test]
    fn file_is_selected_only_for_absent_or_expired_keychain() {
        for keychain in [None, Some(credential("expired", Some(1000)))] {
            let selected = read_credentials_from_sources(
                true,
                1000,
                || Ok(keychain),
                || Ok(Some(credential("file", Some(2000)))),
            )
            .unwrap();
            assert_eq!(
                validate_access_token(&selected)
                    .unwrap()
                    .access_token
                    .as_deref(),
                Some("file")
            );
        }
        let selected = read_credentials_from_sources(
            false,
            1000,
            || panic!("Keychain is macOS-only"),
            || Ok(Some(credential("linux", None))),
        )
        .unwrap();
        assert_eq!(
            validate_access_token(&selected)
                .unwrap()
                .access_token
                .as_deref(),
            Some("linux")
        );
    }

    #[test]
    fn keychain_failures_and_blank_tokens_do_not_silently_select_the_file() {
        let error = read_credentials_from_sources(
            true,
            1000,
            || Err(anyhow::anyhow!("keychain denied")),
            || panic!("must preserve keychain failure"),
        )
        .unwrap_err();
        assert_eq!(error.to_string(), "keychain denied");
        let error = read_credentials_from_sources(
            true,
            1000,
            || Ok(Some(credential("  ", None))),
            || panic!("must report invalid credentials"),
        )
        .unwrap_err();
        assert_eq!(
            SubscriptionIssue::from_anyhow(&error).code(),
            SubscriptionIssueCode::ClaudeNoAccessToken
        );
    }

    #[test]
    fn expired_and_missing_credentials_remain_distinct_errors() {
        for file in [None, Some(credential("expired-file", Some(999)))] {
            let error = read_credentials_from_sources(
                true,
                1000,
                || Ok(Some(credential("expired-keychain", Some(999)))),
                || Ok(file),
            )
            .unwrap_err();
            assert_eq!(
                SubscriptionIssue::from_anyhow(&error).code(),
                SubscriptionIssueCode::ClaudeCredentialExpired
            );
        }
        let error =
            read_credentials_from_sources(true, 1000, || Ok(None), || Ok(None)).unwrap_err();
        assert_eq!(
            SubscriptionIssue::from_anyhow(&error).code(),
            SubscriptionIssueCode::ClaudeNoCredentials
        );
    }

    #[test]
    fn file_read_errors_and_parse_errors_are_explicit_and_read_only() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("credentials.json");
        assert!(read_file_credentials(Some(&path)).unwrap().is_none());
        assert!(read_file_credentials(Some(dir.path())).is_err());
        std::fs::write(&path, "{invalid").unwrap();
        let error = read_file_credentials(Some(&path)).unwrap_err();
        assert_eq!(
            SubscriptionIssue::from_anyhow(&error).code(),
            SubscriptionIssueCode::ClaudeParseCredentials
        );
        assert_eq!(std::fs::read_to_string(path).unwrap(), "{invalid");
    }
}
