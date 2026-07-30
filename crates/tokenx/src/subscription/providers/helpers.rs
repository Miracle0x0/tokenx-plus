use anyhow::Result;

use super::{SubscriptionIssue, SubscriptionIssueCode};

pub fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => s.to_string(),
    }
}

pub fn read_keychain(service: &str) -> Result<String> {
    if cfg!(not(target_os = "macos")) {
        return Err(anyhow::Error::new(SubscriptionIssue::new(
            SubscriptionIssueCode::KeychainMacOnly,
            "Keychain lookup is only available on macOS",
        )));
    }
    let out = std::process::Command::new("security")
        .args(["find-generic-password", "-s", service, "-w"])
        .output()?;
    if !out.status.success() {
        return Err(anyhow::Error::new(
            SubscriptionIssue::new(
                SubscriptionIssueCode::KeychainLookupFailed,
                format!("Keychain lookup failed for service '{service}'"),
            )
            .with_field("service", service),
        ));
    }
    Ok(String::from_utf8(out.stdout)?.trim_end().to_string())
}

pub fn read_env(key: &str) -> Option<String> {
    let value = std::env::var(key).ok()?;
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}
