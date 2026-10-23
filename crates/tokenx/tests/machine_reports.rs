use serde_json::{json, Value};
use std::{path::Path, process::Command};

fn write(path: &Path, value: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, value).unwrap();
}

fn report(home: &Path, config: &Path, grouping: &str, json_output: bool) -> String {
    let mut command = Command::new(assert_cmd::cargo::cargo_bin!("tokenx"));
    command
        .env("TOKENX_CONFIG_DIR", config)
        .args(["--language", "en", "models", "--home"])
        .arg(home)
        .args(["--client", "pi", "--group-by", grouping, "--no-spinner"]);
    if json_output {
        command.arg("--json");
    }
    let result = command.output().unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    String::from_utf8(result.stdout).unwrap()
}

#[test]
fn machine_grouping_and_price_origins_are_visible_in_cli_json_and_tables() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("home");
    let config = root.path().join("config");
    let remote = root.path().join("server");
    for (path, id, provider) in [
        (
            home.join(".pi/agent/sessions/local.jsonl"),
            "local",
            "gateway",
        ),
        (remote.join("remote.jsonl"), "server", "openai"),
    ] {
        write(
            &path,
            &format!(
                "{}\n{}\n",
                json!({"type":"session","id":id}),
                json!({"type":"message","timestamp":"2026-10-18T00:00:00Z",
            "message":{"role":"assistant","model":"gpt-5","provider":provider,
            "usage":{"input":100,"output":0,"cacheRead":0,"cacheWrite":0,"totalTokens":100}}})
            ),
        );
    }
    write(&config.join("settings.json"),&json!({"scanner":{"extraScanPaths":{"pi":[remote]},"machineLabels":{remote.to_str().unwrap():"gpu"},"localMachineLabel":"laptop"}}).to_string());
    write(&config.join("custom-pricing.json"),&json!({"provider_overrides":[{"provider_id":"gateway","model_id":"gpt-5","pricing":{"input_cost_per_token":0.0}}]}).to_string());
    for catalog in ["litellm", "openrouter", "models-dev"] {
        let data = if catalog == "litellm" {
            json!({"openai/gpt-5":{"input_cost_per_token":0.000001}})
        } else {
            json!({})
        };
        write(
            &config.join(format!("cache/pricing-{catalog}.json")),
            &json!({"version":4,"data":data}).to_string(),
        );
    }
    for _ in 0..2 {
        let parsed: Value =
            serde_json::from_str(&report(&home, &config, "machine,model", true)).unwrap();
        assert_eq!(parsed["data"]["groupBy"], "machine,model");
        assert_eq!(parsed["data"]["totals"]["tokens"], 200);
        assert_eq!(parsed["data"]["totals"]["costProvenance"]["complete"], true);
        assert_eq!(
            parsed["data"]["totals"]["costProvenance"]["sources"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        let rows = parsed["data"]["models"].as_array().unwrap();
        let local = rows.iter().find(|row| row["machine"] == "laptop").unwrap();
        assert_eq!(local["cost"], 0.0);
        assert_eq!(local["costProvenance"]["sources"][0]["source"], "Custom");
        assert_eq!(local["costProvenance"]["sources"][0]["matchedKey"], "gpt-5");
        assert_eq!(
            local["costProvenance"]["sources"][0]["billingProvider"],
            "gateway"
        );
        assert!(rows.iter().any(|row| row["machine"] == "gpu"));
    }
    let table = report(&home, &config, "machine,model", false);
    assert!(table.contains("Machine") && table.contains("laptop") && table.contains("gpu"));
    let flat: Value = serde_json::from_str(&report(&home, &config, "model", true)).unwrap();
    assert_eq!(flat["data"]["models"].as_array().unwrap().len(), 1);
    assert!(flat["data"]["models"][0].get("machine").is_none());
    assert_eq!(
        flat["data"]["models"][0]["costProvenance"]["sources"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
}
