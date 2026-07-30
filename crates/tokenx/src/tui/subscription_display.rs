//! Localized presentation adapter for locale-neutral Subscription data.

use chrono::{DateTime, Datelike, Duration, Utc};

use crate::subscription::{
    SubscriptionError, SubscriptionIssue, SubscriptionIssueCode, SubscriptionOutput,
};

use crate::date_display::{format_clock_time, format_month_day, weekday_name};

pub(crate) fn output_name(output: &SubscriptionOutput) -> String {
    output_name_for_locale(output, &rust_i18n::locale())
}

fn output_name_for_locale(output: &SubscriptionOutput, locale: &str) -> String {
    let account_name = output.account.as_ref().map(|account| {
        account
            .label_name()
            .map(str::to_string)
            .or_else(|| {
                output
                    .email
                    .as_deref()
                    .map(str::trim)
                    .filter(|email| !email.is_empty())
                    .map(str::to_string)
            })
            .or_else(|| {
                account.short_id().map(|id| {
                    rust_i18n::t!("subscription.display.account", locale = locale, id = id)
                        .into_owned()
                })
            })
            .unwrap_or_else(|| {
                rust_i18n::t!("subscription.provider.unknown", locale = locale).into_owned()
            })
    });

    let name = account_name.map_or_else(
        || output.provider.label().to_string(),
        |account| {
            rust_i18n::t!(
                "subscription.display.provider_account",
                locale = locale,
                provider = output.provider.label(),
                account = account
            )
            .into_owned()
        },
    );
    if output.stale {
        rust_i18n::t!("subscription.display.stale", locale = locale, name = name).into_owned()
    } else {
        name
    }
}

pub(crate) fn metric_label(label: &str) -> String {
    metric_label_for_locale(label, &rust_i18n::locale())
}

fn metric_label_for_locale(label: &str, locale: &str) -> String {
    let static_key = match label {
        "Session" => Some("subscription.metric.session"),
        "Weekly" => Some("subscription.metric.weekly"),
        "Opus" => Some("subscription.metric.opus"),
        "Monthly" => Some("subscription.metric.monthly"),
        "Credits" => Some("subscription.metric.credits"),
        "On demand" => Some("subscription.metric.on_demand"),
        "Web Search" => Some("subscription.metric.web_search"),
        "model" => Some("subscription.metric.model_fallback"),
        _ => None,
    };
    if let Some(key) = static_key {
        return rust_i18n::t!(key, locale = locale).into_owned();
    }
    if let Some(hours) = label.strip_suffix(" Hour") {
        if hours.parse::<i64>().is_ok() {
            return rust_i18n::t!(
                "subscription.metric.hour_window",
                locale = locale,
                hours = hours
            )
            .into_owned();
        }
    }
    if let Some(duration) = label.strip_suffix("m limit") {
        if duration.parse::<i64>().is_ok() {
            return rust_i18n::t!(
                "subscription.metric.minute_limit",
                locale = locale,
                duration = duration
            )
            .into_owned();
        }
    }
    if let Some(duration) = label.strip_suffix("d limit") {
        if duration.parse::<i64>().is_ok() {
            return rust_i18n::t!(
                "subscription.metric.day_limit",
                locale = locale,
                duration = duration
            )
            .into_owned();
        }
    }
    if let Some(duration) = label.strip_suffix("s limit") {
        if duration.parse::<i64>().is_ok() {
            return rust_i18n::t!(
                "subscription.metric.second_limit",
                locale = locale,
                duration = duration
            )
            .into_owned();
        }
    }
    if let Some(index) = label.strip_prefix("Limit ") {
        if index.parse::<usize>().is_ok() {
            return rust_i18n::t!(
                "subscription.metric.limit_n",
                locale = locale,
                index = index
            )
            .into_owned();
        }
    }
    if let Some(name) = label.strip_suffix("-wk") {
        if !name.is_empty() {
            return rust_i18n::t!(
                "subscription.metric.weekly_suffix",
                locale = locale,
                name = name
            )
            .into_owned();
        }
    }
    label.to_string()
}

pub(crate) fn remaining_label(label: &str) -> String {
    remaining_label_for_locale(label, &rust_i18n::locale())
}

fn remaining_label_for_locale(label: &str, locale: &str) -> String {
    label
        .strip_suffix(" left")
        .filter(|value| !value.is_empty())
        .map(|value| {
            rust_i18n::t!("subscription.metric.left", locale = locale, value = value).into_owned()
        })
        .unwrap_or_else(|| label.to_string())
}

pub(crate) fn error_provider(error: &SubscriptionError) -> String {
    match (error.provider_id, error.provider.as_str()) {
        (Some(provider), _) => provider.label().to_string(),
        (None, "Subscription cache") => rust_i18n::t!("subscription.provider.cache").into_owned(),
        (None, "unknown") => rust_i18n::t!("subscription.provider.unknown").into_owned(),
        (None, provider) => provider.to_string(),
    }
}

pub(crate) fn error_message(error: &SubscriptionError) -> String {
    issue_message_for_locale(&error.issue, &rust_i18n::locale())
}

fn issue_message_for_locale(issue: &SubscriptionIssue, locale: &str) -> String {
    let field = |name: &str| issue.field(name).unwrap_or("");
    match issue.code() {
        SubscriptionIssueCode::Unexpected => issue.message().to_string(),
        SubscriptionIssueCode::FetchPanicked => {
            rust_i18n::t!("subscription.error.fetch_panicked", locale = locale).into_owned()
        }
        SubscriptionIssueCode::FetchTimeout => rust_i18n::t!(
            "subscription.error.fetch_timeout",
            locale = locale,
            secs = field("secs")
        )
        .into_owned(),
        SubscriptionIssueCode::WorkerDisconnected => {
            rust_i18n::t!("subscription.error.worker_disconnected", locale = locale).into_owned()
        }
        SubscriptionIssueCode::KeychainMacOnly => {
            rust_i18n::t!("subscription.error.keychain_macos_only", locale = locale).into_owned()
        }
        SubscriptionIssueCode::KeychainLookupFailed => rust_i18n::t!(
            "subscription.error.keychain_lookup_failed",
            locale = locale,
            service = field("service")
        )
        .into_owned(),
        SubscriptionIssueCode::CacheClockBeforeEpoch => rust_i18n::t!(
            "subscription.error.cache_clock_before_epoch",
            locale = locale
        )
        .into_owned(),
        SubscriptionIssueCode::CacheSerialize => {
            rust_i18n::t!("subscription.error.cache_serialize", locale = locale).into_owned()
        }
        SubscriptionIssueCode::CachePersist => rust_i18n::t!(
            "subscription.error.cache_persist",
            locale = locale,
            path = field("path")
        )
        .into_owned(),
        SubscriptionIssueCode::CacheRead => rust_i18n::t!(
            "subscription.error.cache_read",
            locale = locale,
            path = field("path")
        )
        .into_owned(),
        SubscriptionIssueCode::CacheMalformed => rust_i18n::t!(
            "subscription.error.cache_malformed",
            locale = locale,
            path = field("path")
        )
        .into_owned(),
        SubscriptionIssueCode::CacheUnsupportedSchema => rust_i18n::t!(
            "subscription.error.cache_unsupported_schema",
            locale = locale,
            path = field("path"),
            schema = field("schema")
        )
        .into_owned(),
        SubscriptionIssueCode::CacheUnsupportedVersion => rust_i18n::t!(
            "subscription.error.cache_unsupported_version",
            locale = locale,
            path = field("path"),
            version = field("version")
        )
        .into_owned(),
        SubscriptionIssueCode::CacheFutureTimestamp => rust_i18n::t!(
            "subscription.error.cache_future_timestamp",
            locale = locale,
            path = field("path"),
            timestamp = field("timestamp"),
            now = field("now")
        )
        .into_owned(),
        SubscriptionIssueCode::ClaudeReadCredentials => rust_i18n::t!(
            "subscription.error.claude_read_credentials",
            locale = locale,
            path = field("path")
        )
        .into_owned(),
        SubscriptionIssueCode::ClaudeParseCredentials => rust_i18n::t!(
            "subscription.error.claude_parse_credentials",
            locale = locale,
            path = field("path")
        )
        .into_owned(),
        SubscriptionIssueCode::ClaudeNoHomeOrKeychain => {
            let error = issue
                .cause()
                .map(|cause| issue_message_for_locale(cause, locale))
                .unwrap_or_else(|| field("error").to_string());
            rust_i18n::t!(
                "subscription.error.claude_no_home_or_keychain",
                locale = locale,
                error = error
            )
            .into_owned()
        }
        SubscriptionIssueCode::ClaudeNoCredentials => {
            rust_i18n::t!("subscription.error.claude_no_credentials", locale = locale).into_owned()
        }
        SubscriptionIssueCode::ClaudeRejected => {
            rust_i18n::t!("subscription.error.claude_rejected", locale = locale).into_owned()
        }
        SubscriptionIssueCode::ClaudeRequestFailed => rust_i18n::t!(
            "subscription.error.claude_request_failed",
            locale = locale,
            status = field("status")
        )
        .into_owned(),
        SubscriptionIssueCode::ClaudeNoOauth => {
            rust_i18n::t!("subscription.error.claude_no_oauth", locale = locale).into_owned()
        }
        SubscriptionIssueCode::ClaudeNoAccessToken => {
            rust_i18n::t!("subscription.error.claude_no_access_token", locale = locale).into_owned()
        }
        SubscriptionIssueCode::CodexNoHome => {
            rust_i18n::t!("subscription.error.codex_no_home", locale = locale).into_owned()
        }
        SubscriptionIssueCode::CodexReadAuth => rust_i18n::t!(
            "subscription.error.codex_read_auth",
            locale = locale,
            path = field("path")
        )
        .into_owned(),
        SubscriptionIssueCode::CodexParseAuth => rust_i18n::t!(
            "subscription.error.codex_parse_auth",
            locale = locale,
            path = field("path")
        )
        .into_owned(),
        SubscriptionIssueCode::CodexNoCredentials => rust_i18n::t!(
            "subscription.error.codex_no_credentials",
            locale = locale,
            path = field("path")
        )
        .into_owned(),
        SubscriptionIssueCode::CodexNoUsableToken => rust_i18n::t!(
            "subscription.error.codex_no_usable_token",
            locale = locale,
            path = field("path")
        )
        .into_owned(),
        SubscriptionIssueCode::CodexRejected => {
            rust_i18n::t!("subscription.error.codex_rejected", locale = locale).into_owned()
        }
        SubscriptionIssueCode::CodexRequestFailed => rust_i18n::t!(
            "subscription.error.codex_request_failed",
            locale = locale,
            status = field("status")
        )
        .into_owned(),
        SubscriptionIssueCode::CodexAuthPage => {
            rust_i18n::t!("subscription.error.codex_auth_page", locale = locale).into_owned()
        }
        SubscriptionIssueCode::CodexNoTokens => {
            rust_i18n::t!("subscription.error.codex_no_tokens", locale = locale).into_owned()
        }
        SubscriptionIssueCode::CodexNoAccessToken => {
            rust_i18n::t!("subscription.error.codex_no_access_token", locale = locale).into_owned()
        }
        SubscriptionIssueCode::GrokNoHome => {
            rust_i18n::t!("subscription.error.grok_no_home", locale = locale).into_owned()
        }
        SubscriptionIssueCode::GrokReadCredentials => rust_i18n::t!(
            "subscription.error.grok_read_credentials",
            locale = locale,
            path = field("path")
        )
        .into_owned(),
        SubscriptionIssueCode::GrokParseCredentials => rust_i18n::t!(
            "subscription.error.grok_parse_credentials",
            locale = locale,
            path = field("path")
        )
        .into_owned(),
        SubscriptionIssueCode::GrokAuthNotObject => {
            rust_i18n::t!("subscription.error.grok_auth_not_object", locale = locale).into_owned()
        }
        SubscriptionIssueCode::GrokAuthEntriesNone => {
            rust_i18n::t!("subscription.error.grok_auth_entries_none", locale = locale).into_owned()
        }
        SubscriptionIssueCode::GrokAuthEntriesMultiple => rust_i18n::t!(
            "subscription.error.grok_auth_entries_multiple",
            locale = locale,
            count = field("count")
        )
        .into_owned(),
        SubscriptionIssueCode::GrokMissingUserId => {
            rust_i18n::t!("subscription.error.grok_missing_user_id", locale = locale).into_owned()
        }
        SubscriptionIssueCode::GrokMissingPrincipalId => rust_i18n::t!(
            "subscription.error.grok_missing_principal_id",
            locale = locale
        )
        .into_owned(),
        SubscriptionIssueCode::GrokFieldEmpty => rust_i18n::t!(
            "subscription.error.grok_field_empty",
            locale = locale,
            field = field("field")
        )
        .into_owned(),
        SubscriptionIssueCode::GrokRejected => {
            rust_i18n::t!("subscription.error.grok_rejected", locale = locale).into_owned()
        }
        SubscriptionIssueCode::GrokBillingError => rust_i18n::t!(
            "subscription.error.grok_billing_error",
            locale = locale,
            detail = field("detail")
        )
        .into_owned(),
        SubscriptionIssueCode::GrokParseBilling => {
            rust_i18n::t!("subscription.error.grok_parse_billing", locale = locale).into_owned()
        }
        SubscriptionIssueCode::GrokNoData => {
            rust_i18n::t!("subscription.error.grok_no_data", locale = locale).into_owned()
        }
        SubscriptionIssueCode::KimiNoHome => {
            rust_i18n::t!("subscription.error.kimi_no_home", locale = locale).into_owned()
        }
        SubscriptionIssueCode::KimiNoCredential => rust_i18n::t!(
            "subscription.error.kimi_no_credential",
            locale = locale,
            path = field("path")
        )
        .into_owned(),
        SubscriptionIssueCode::KimiRequestFailed => rust_i18n::t!(
            "subscription.error.kimi_request_failed",
            locale = locale,
            status = field("status")
        )
        .into_owned(),
        SubscriptionIssueCode::KimiAuthRejected => rust_i18n::t!(
            "subscription.error.kimi_auth_rejected",
            locale = locale,
            source = field("source"),
            status = field("status")
        )
        .into_owned(),
        SubscriptionIssueCode::KimiNoAccessToken => {
            rust_i18n::t!("subscription.error.kimi_no_access_token", locale = locale).into_owned()
        }
        SubscriptionIssueCode::KimiCredentialExpired => rust_i18n::t!(
            "subscription.error.kimi_credential_expired",
            locale = locale
        )
        .into_owned(),
        SubscriptionIssueCode::KimiEnvRequired => rust_i18n::t!(
            "subscription.error.kimi_env_required",
            locale = locale,
            env = field("env"),
            source = field("source")
        )
        .into_owned(),
        SubscriptionIssueCode::MiniMaxSessionExpired => rust_i18n::t!(
            "subscription.error.minimax_session_expired",
            locale = locale,
            site = field("site")
        )
        .into_owned(),
        SubscriptionIssueCode::MiniMaxRequestFailed => rust_i18n::t!(
            "subscription.error.minimax_request_failed",
            locale = locale,
            site = field("site"),
            status = field("status")
        )
        .into_owned(),
        SubscriptionIssueCode::MiniMaxApiError => {
            let message = if field("message") == "unknown error" {
                rust_i18n::t!("subscription.error.minimax_unknown_error", locale = locale)
                    .into_owned()
            } else {
                field("message").to_string()
            };
            rust_i18n::t!(
                "subscription.error.minimax_api_error",
                locale = locale,
                site = field("site"),
                message = message
            )
            .into_owned()
        }
        SubscriptionIssueCode::MiniMaxNoUsage => rust_i18n::t!(
            "subscription.error.minimax_no_usage",
            locale = locale,
            site = field("site")
        )
        .into_owned(),
        SubscriptionIssueCode::MiniMaxNoEnv => rust_i18n::t!(
            "subscription.error.minimax_no_env",
            locale = locale,
            env = field("env")
        )
        .into_owned(),
        SubscriptionIssueCode::ZaiQuotaFailed => rust_i18n::t!(
            "subscription.error.zai_quota_failed",
            locale = locale,
            status = field("status")
        )
        .into_owned(),
        SubscriptionIssueCode::ZaiSubscriptionFailed => rust_i18n::t!(
            "subscription.error.zai_subscription_failed",
            locale = locale,
            status = field("status")
        )
        .into_owned(),
        SubscriptionIssueCode::ZaiNoApiKey => {
            rust_i18n::t!("subscription.error.zai_no_api_key", locale = locale).into_owned()
        }
    }
}

pub(crate) fn format_reset_time(resets_at: &str) -> String {
    let dt = match DateTime::parse_from_rfc3339(resets_at) {
        Ok(date) => date.with_timezone(&Utc),
        Err(_) => return resets_at.to_string(),
    };
    let diff = dt - Utc::now();
    if diff <= Duration::zero() {
        return rust_i18n::t!("subscription.reset.now").into_owned();
    }
    let total_mins = diff.num_minutes();
    if total_mins < 60 {
        rust_i18n::t!("subscription.reset.in_minutes", mins = total_mins).into_owned()
    } else if total_mins < 24 * 60 {
        let hours = diff.num_hours();
        let mins = (diff - Duration::hours(hours)).num_minutes();
        if mins > 0 {
            rust_i18n::t!(
                "subscription.reset.in_hours_minutes",
                hours = hours,
                mins = mins
            )
            .into_owned()
        } else {
            rust_i18n::t!("subscription.reset.in_hours", hours = hours).into_owned()
        }
    } else if diff.num_days() < 7 {
        let datetime = format!(
            "{} {}",
            weekday_name(dt.weekday()),
            format_clock_time(dt.naive_utc())
        );
        rust_i18n::t!("subscription.reset.at", datetime = datetime).into_owned()
    } else {
        rust_i18n::t!(
            "subscription.reset.at",
            datetime = format_month_day(dt.date_naive())
        )
        .into_owned()
    }
}

pub(crate) fn render_ascii_bar(remaining_percent: f64, width: usize) -> String {
    let filled = (remaining_percent.clamp(0.0, 100.0) / 100.0 * width as f64).round() as usize;
    format!("[{}{}]", "=".repeat(filled), "-".repeat(width - filled))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::subscription::{
        ProviderId, SubscriptionIssue, SubscriptionIssueCode, UsageAccount, UsageMetric,
    };

    fn output() -> SubscriptionOutput {
        SubscriptionOutput {
            provider: ProviderId::Codex,
            stale: true,
            account: Some(UsageAccount {
                id: "account-123456789".to_string(),
                label: None,
                is_active: true,
            }),
            plan: None,
            email: None,
            metrics: vec![UsageMetric {
                label: "5 Hour".to_string(),
                used_percent: 20.0,
                remaining_percent: 80.0,
                remaining_label: Some("80 left".to_string()),
                resets_at: None,
            }],
        }
    }

    #[test]
    fn canonical_subscription_data_localizes_only_at_presentation() {
        let output = output();
        assert_eq!(
            output_name_for_locale(&output, "en"),
            "Codex (Account accoun...6789) (stale)"
        );
        assert_eq!(
            output_name_for_locale(&output, "zh-CN"),
            "Codex（账户 accoun...6789）（数据已过期）"
        );
        assert_eq!(metric_label_for_locale("5 Hour", "en"), "5 Hour");
        assert_eq!(metric_label_for_locale("5 Hour", "zh-CN"), "5 小时");
        assert_eq!(remaining_label_for_locale("80 left", "zh-CN"), "剩余 80");
    }

    #[test]
    fn structured_issue_localizes_without_parsing_english_text() {
        let issue = SubscriptionIssue::new(
            SubscriptionIssueCode::FetchTimeout,
            "provider fetch exceeded the 30s overall timeout",
        )
        .with_field("secs", 30);

        assert_eq!(
            issue_message_for_locale(&issue, "zh-CN"),
            "提供商用量获取超过 30 秒总超时"
        );
        assert_eq!(
            issue_message_for_locale(&issue, "en"),
            "provider fetch exceeded the 30s overall timeout"
        );
    }

    #[test]
    fn nested_keychain_issues_localize_at_every_level() {
        let mac_only = SubscriptionIssue::new(
            SubscriptionIssueCode::ClaudeNoHomeOrKeychain,
            "No Claude credentials found: the home directory is unavailable and the keychain lookup failed: Keychain lookup is only available on macOS",
        )
        .with_cause(SubscriptionIssue::new(
            SubscriptionIssueCode::KeychainMacOnly,
            "Keychain lookup is only available on macOS",
        ));

        assert_eq!(
            issue_message_for_locale(&mac_only, "zh-CN"),
            "未找到 Claude 凭据：主目录不可用且钥匙串查询失败：钥匙串查询仅在 macOS 上可用"
        );
        assert_eq!(
            issue_message_for_locale(&mac_only, "en"),
            "No Claude credentials found: the home directory is unavailable and the keychain lookup failed: Keychain lookup is only available on macOS"
        );

        let lookup_failed = SubscriptionIssue::new(
            SubscriptionIssueCode::ClaudeNoHomeOrKeychain,
            "No Claude credentials found: the home directory is unavailable and the keychain lookup failed: Keychain lookup failed for service 'Claude Code-credentials'",
        )
        .with_cause(
            SubscriptionIssue::new(
                SubscriptionIssueCode::KeychainLookupFailed,
                "Keychain lookup failed for service 'Claude Code-credentials'",
            )
            .with_field("service", "Claude Code-credentials"),
        );

        assert_eq!(
            issue_message_for_locale(&lookup_failed, "zh-CN"),
            "未找到 Claude 凭据：主目录不可用且钥匙串查询失败：服务 'Claude Code-credentials' 的钥匙串查询失败"
        );
    }
}
