use assert_cmd::{cargo::cargo_bin_cmd, Command};
use serde_json::{json, Value};
use std::net::TcpListener;
use std::path::Path;
use std::time::{Duration, UNIX_EPOCH};

fn fixture() -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    let sessions = root.path().join(".pi/agent/sessions");
    std::fs::create_dir_all(&sessions).unwrap();
    std::fs::write(
        sessions.join("a.jsonl"),
        format!(
            "{}\n{}\n",
            json!({"type":"session","id":"a"}),
            json!({"type":"message","timestamp":"2026-10-25T00:00:00Z","message":{
            "role":"assistant","model":"gpt-5","provider":"openai",
            "usage":{"input":100,"output":2,"cacheRead":0,"cacheWrite":0,"totalTokens":102}}})
        ),
    )
    .unwrap();
    root
}

fn seed_prices(root: &Path) {
    let cache = root.join("config/cache");
    std::fs::create_dir_all(&cache).unwrap();
    for source in ["litellm", "openrouter", "models-dev"] {
        let data = if source == "litellm" {
            json!({"gpt-5":{"input_cost_per_token":0.000001,"output_cost_per_token":0.000002}})
        } else {
            json!({})
        };
        std::fs::write(
            cache.join(format!("pricing-{source}.json")),
            json!({"version":tokenx_engine::pricing::cache::CACHE_FORMAT_VERSION,"data":data})
                .to_string(),
        )
        .unwrap();
    }
}

fn command(root: &Path, proxy: &str) -> Command {
    let mut command = cargo_bin_cmd!("tokenx");
    for key in [
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
        "http_proxy",
        "https_proxy",
        "all_proxy",
        "NO_PROXY",
        "no_proxy",
    ] {
        command.env_remove(key);
    }
    command
        .env("HTTP_PROXY", proxy)
        .env("HTTPS_PROXY", proxy)
        .env("ALL_PROXY", proxy)
        .env("TOKENX_CONFIG_DIR", root.join("config"))
        .env("HOME", root)
        .args(["--language", "en"])
        .timeout(Duration::from_secs(10));
    command
}

#[test]
fn offline_reports_and_lookups_use_stale_prices_without_network_requests() {
    let root = fixture();
    seed_prices(root.path());
    for source in ["litellm", "openrouter", "models-dev"] {
        std::fs::File::options()
            .write(true)
            .open(
                root.path()
                    .join(format!("config/cache/pricing-{source}.json")),
            )
            .unwrap()
            .set_modified(UNIX_EPOCH + Duration::from_secs(1))
            .unwrap();
    }
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let proxy = format!("http://{}", listener.local_addr().unwrap());
    let report = command(root.path(), &proxy)
        .args([
            "--offline",
            "models",
            "--client",
            "pi",
            "--json",
            "--no-spinner",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let report: Value = serde_json::from_slice(&report).unwrap();
    assert_eq!(report["data"]["totals"]["tokens"], 102);
    assert!((report["data"]["totals"]["cost"].as_f64().unwrap() - 0.000104).abs() < 1e-12);
    command(root.path(), &proxy)
        .args([
            "pricing",
            "lookup",
            "gpt-5",
            "--offline",
            "--json",
            "--no-spinner",
        ])
        .assert()
        .success();
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
}

#[test]
fn offline_missing_prices_preserves_usage_and_explicit_diagnostics() {
    let root = fixture();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let proxy = format!("http://{}", listener.local_addr().unwrap());
    let output = command(root.path(), &proxy)
        .args([
            "models",
            "--offline",
            "--client",
            "pi",
            "--json",
            "--no-spinner",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let report: Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(report["data"]["totals"]["tokens"], 102);
    assert_eq!(report["metadata"]["pricingStatus"], "unavailable");
    assert!(!report["metadata"]["pricingDiagnostics"]
        .as_array()
        .unwrap()
        .is_empty());
    command(root.path(), &proxy)
        .args(["--offline", "pricing", "refresh", "--no-spinner"])
        .assert()
        .code(2);
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
}

#[test]
fn explicit_refresh_attempts_fresh_sources_and_preserves_files_on_failure() {
    let root = fixture();
    seed_prices(root.path());
    let cache = root.path().join("config/cache");
    let paths: Vec<_> = ["litellm", "openrouter", "models-dev"]
        .map(|source| cache.join(format!("pricing-{source}.json")))
        .into();
    let before: Vec<_> = paths
        .iter()
        .map(|path| std::fs::read(path).unwrap())
        .collect();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let proxy = format!("http://{}", listener.local_addr().unwrap());
    drop(listener);
    command(root.path(), &proxy)
        .args(["pricing", "refresh", "--no-spinner"])
        .assert()
        .code(1);
    let after: Vec<_> = paths
        .iter()
        .map(|path| std::fs::read(path).unwrap())
        .collect();
    assert_eq!(after, before);
}
