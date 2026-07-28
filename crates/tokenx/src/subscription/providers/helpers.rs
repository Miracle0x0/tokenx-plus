use anyhow::Result;
use chrono::{DateTime, Datelike, Duration, Utc};

use crate::tui::date::{format_clock_time, format_month_day, weekday_name};

pub fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => s.to_string(),
    }
}

pub fn read_keychain(service: &str) -> Result<String> {
    if cfg!(not(target_os = "macos")) {
        anyhow::bail!(rust_i18n::t!("subscription.error.keychain_macos_only"));
    }
    let out = std::process::Command::new("security")
        .args(["find-generic-password", "-s", service, "-w"])
        .output()?;
    if !out.status.success() {
        anyhow::bail!(rust_i18n::t!(
            "subscription.error.keychain_lookup_failed",
            service = service
        ));
    }
    Ok(String::from_utf8(out.stdout)?.trim_end().to_string())
}

pub fn read_env(key: &str) -> Option<String> {
    let value = std::env::var(key).ok()?;
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

pub fn format_reset_time(resets_at: &str) -> String {
    let dt = match DateTime::parse_from_rfc3339(resets_at) {
        Ok(d) => d.with_timezone(&Utc),
        Err(_) => return resets_at.into(),
    };
    let diff = dt - Utc::now();
    if diff <= Duration::zero() {
        return rust_i18n::t!("subscription.reset.now").to_string();
    }
    let total_mins = diff.num_minutes();
    if total_mins < 60 {
        rust_i18n::t!("subscription.reset.in_minutes", mins = total_mins).to_string()
    } else if total_mins < 24 * 60 {
        let h = diff.num_hours();
        let m = (diff - Duration::hours(h)).num_minutes();
        if m > 0 {
            rust_i18n::t!("subscription.reset.in_hours_minutes", hours = h, mins = m).to_string()
        } else {
            rust_i18n::t!("subscription.reset.in_hours", hours = h).to_string()
        }
    } else if diff.num_days() < 7 {
        let datetime = format!(
            "{} {}",
            weekday_name(dt.weekday()),
            format_clock_time(dt.naive_utc())
        );
        rust_i18n::t!("subscription.reset.at", datetime = datetime).to_string()
    } else {
        let datetime = format_month_day(dt.date_naive());
        rust_i18n::t!("subscription.reset.at", datetime = datetime).to_string()
    }
}

pub fn render_ascii_bar(remaining_percent: f64, width: usize) -> String {
    let filled = (remaining_percent.clamp(0.0, 100.0) / 100.0 * width as f64).round() as usize;
    format!("[{}{}]", "=".repeat(filled), "-".repeat(width - filled))
}
