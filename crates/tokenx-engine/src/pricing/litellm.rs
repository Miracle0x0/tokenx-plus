use super::{cache, emit_warning, http, PricingDiagnosticSink, PricingDiagnostics};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

const CACHE_FILENAME: &str = "pricing-litellm.json";
const PRICING_URL: &str =
    "https://raw.githubusercontent.com/BerriAI/litellm/main/model_prices_and_context_window.json";

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TimePeriodPrice {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub utc_days: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub utc_start: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub utc_end: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_cost_per_token: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_cost_per_token: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read_input_token_cost: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_creation_input_token_cost: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ModelPricing {
    pub input_cost_per_token_above_100k_tokens: Option<f64>,
    pub output_cost_per_token_above_100k_tokens: Option<f64>,
    pub cache_read_input_token_cost_above_100k_tokens: Option<f64>,
    pub cache_creation_input_token_cost_above_100k_tokens: Option<f64>,
    pub cache_creation_input_token_cost_above_1hr_above_100k_tokens: Option<f64>,

    /// Source-declared provider for native LiteLLM rows without a route prefix.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub litellm_provider: Option<String>,
    #[serde(flatten)]
    pub service_tiers: super::service_tier::ServiceTierPricing,
    pub input_cost_per_token: Option<f64>,
    pub input_cost_per_token_above_128k_tokens: Option<f64>,
    pub input_cost_per_token_above_200k_tokens: Option<f64>,
    pub input_cost_per_token_above_256k_tokens: Option<f64>,
    pub input_cost_per_token_above_272k_tokens: Option<f64>,
    pub output_cost_per_token: Option<f64>,
    pub output_cost_per_token_above_128k_tokens: Option<f64>,
    pub output_cost_per_token_above_200k_tokens: Option<f64>,
    pub output_cost_per_token_above_256k_tokens: Option<f64>,
    pub output_cost_per_token_above_272k_tokens: Option<f64>,
    pub cache_creation_input_token_cost: Option<f64>,
    pub cache_creation_input_token_cost_above_200k_tokens: Option<f64>,
    pub cache_creation_input_token_cost_above_1hr: Option<f64>,
    pub cache_creation_input_token_cost_above_1hr_above_200k_tokens: Option<f64>,
    pub cache_creation_input_token_cost_above_272k_tokens: Option<f64>,
    pub cache_read_input_token_cost: Option<f64>,
    pub cache_read_input_token_cost_above_200k_tokens: Option<f64>,
    pub cache_read_input_token_cost_above_272k_tokens: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_period_prices: Option<Vec<TimePeriodPrice>>,
}

pub type PricingDataset = HashMap<String, ModelPricing>;

pub fn load_cached(cache_dir: &Path) -> Option<PricingDataset> {
    cache::load_cache(cache_dir, CACHE_FILENAME)
}

pub fn load_cached_any_age(cache_dir: &Path) -> Option<PricingDataset> {
    cache::load_cache_any_age(cache_dir, CACHE_FILENAME)
}

pub async fn fetch(cache_dir: &Path) -> Result<PricingDataset, reqwest::Error> {
    let mut diagnostics = None;
    fetch_inner(
        reqwest::Client::builder(),
        cache_dir,
        PRICING_URL,
        true,
        &mut diagnostics,
    )
    .await
}

pub(crate) async fn refresh_with_diagnostics(
    cache_dir: &Path,
    diagnostics: &mut PricingDiagnostics,
) -> Result<PricingDataset, String> {
    let mut diagnostics = Some(diagnostics);
    fetch_inner(
        reqwest::Client::builder(),
        cache_dir,
        PRICING_URL,
        false,
        &mut diagnostics,
    )
    .await
    .map_err(|error| error.to_string())
}

async fn fetch_inner(
    client_builder: reqwest::ClientBuilder,
    cache_dir: &Path,
    url: &str,
    use_cache: bool,
    diagnostics: &mut PricingDiagnosticSink<'_>,
) -> Result<PricingDataset, reqwest::Error> {
    if use_cache {
        if let Some(cached) = load_cached(cache_dir) {
            return Ok(cached);
        }
    }

    let client = http::build_client(client_builder)?;
    for attempt in 0..http::MAX_ATTEMPTS {
        let Some(response) =
            http::request_attempt(&client, url, "LiteLLM", attempt, diagnostics).await?
        else {
            continue;
        };
        let data = match response.json::<PricingDataset>().await {
            Ok(data) => data,
            Err(error) => {
                emit_warning(
                    diagnostics,
                    format!("[tokenx] LiteLLM JSON parse failed: {error}"),
                );
                return Err(error);
            }
        };
        if let Err(error) = cache::save_cache(cache_dir, CACHE_FILENAME, &data) {
            let cache_path = cache::get_cache_path(cache_dir, CACHE_FILENAME);
            emit_warning(
                diagnostics,
                format!(
                    "[tokenx] Warning: Failed to cache LiteLLM pricing at {}: {error}",
                    cache_path.display(),
                ),
            );
        }
        return Ok(data);
    }
    unreachable!("the final HTTP attempt returns a response or an error")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, Read, Write};
    use std::net::TcpListener;
    use std::thread;

    #[tokio::test]
    async fn truncated_response_body_remains_a_terminal_json_error() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = std::io::BufReader::new(&mut stream);
            loop {
                let mut header = String::new();
                assert_ne!(request.read_line(&mut header).unwrap(), 0);
                if header == "\r\n" {
                    break;
                }
            }
            drop(request);
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\nConnection: close\r\n\r\n{")
                .unwrap();
        });
        let cache_dir = tempfile::TempDir::new().unwrap();
        let mut diagnostics = Vec::new();
        let result = fetch_inner(
            reqwest::Client::builder().no_proxy(),
            cache_dir.path(),
            &url,
            false,
            &mut Some(&mut diagnostics),
        )
        .await;
        server.join().unwrap();

        assert!(result.is_err());
        assert_eq!(diagnostics.len(), 1);
        assert!(diagnostics[0]
            .message()
            .contains("LiteLLM JSON parse failed"));
        assert!(!cache_dir.path().join(CACHE_FILENAME).exists());
    }

    fn retryable_status_server(status_line: &'static str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());

        thread::spawn(move || {
            for _ in 0..http::MAX_ATTEMPTS {
                let Ok((mut stream, _)) = listener.accept() else {
                    return;
                };
                let mut buffer = [0; 1024];
                let _ = stream.read(&mut buffer);
                let response =
                    format!("{status_line}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                let _ = stream.write_all(response.as_bytes());
            }
        });

        url
    }

    #[tokio::test]
    async fn fetch_returns_error_after_retryable_http_statuses() {
        let url = retryable_status_server("HTTP/1.1 503 Service Unavailable");
        let mut diagnostics = None;
        let cache_dir = tempfile::TempDir::new().unwrap();

        let result = fetch_inner(
            reqwest::Client::builder().no_proxy(),
            cache_dir.path(),
            &url,
            false,
            &mut diagnostics,
        )
        .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn fetch_with_diagnostics_collects_retryable_http_statuses() {
        let url = retryable_status_server("HTTP/1.1 429 Too Many Requests");
        let mut diagnostics = Vec::new();
        let mut sink = Some(&mut diagnostics);
        let cache_dir = tempfile::TempDir::new().unwrap();

        let result = fetch_inner(
            reqwest::Client::builder().no_proxy(),
            cache_dir.path(),
            &url,
            false,
            &mut sink,
        )
        .await;

        assert!(result.is_err());
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message().contains("LiteLLM HTTP 429")),
            "diagnostics missing retryable status: {diagnostics:?}"
        );
    }

    #[tokio::test]
    async fn successful_fetch_survives_cache_write_failure() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        thread::spawn(move || {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            let mut buffer = [0; 1024];
            let _ = stream.read(&mut buffer);
            let body = br#"{"snapshot-model":{"input_cost_per_token":0.000001,"output_cost_per_token":0.000002}}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes());
            let _ = stream.write_all(body);
        });
        let temp = tempfile::TempDir::new().unwrap();
        let cache_dir = temp.path().join("not-a-directory");
        std::fs::write(&cache_dir, b"blocks cache directory creation").unwrap();
        let mut diagnostics = Vec::new();
        let mut sink = Some(&mut diagnostics);

        let result = fetch_inner(
            reqwest::Client::builder().no_proxy(),
            &cache_dir,
            &url,
            false,
            &mut sink,
        )
        .await
        .unwrap();

        assert!(result.contains_key("snapshot-model"));
        assert!(diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message().contains("Failed to cache LiteLLM")));
    }

    #[test]
    fn test_deserialize_model_pricing_with_above_200k_fields() {
        let pricing: ModelPricing = serde_json::from_str(
            r#"{
                "input_cost_per_token": 0.0000015,
                "input_cost_per_token_above_200k_tokens": 0.000003,
                "output_cost_per_token": 0.0000075,
                "output_cost_per_token_above_200k_tokens": 0.000015,
                "cache_creation_input_token_cost": 0.000001875,
                "cache_creation_input_token_cost_above_200k_tokens": 0.00000375,
                "cache_read_input_token_cost": 0.00000015,
                "cache_read_input_token_cost_above_200k_tokens": 0.0000003
            }"#,
        )
        .unwrap();

        assert_eq!(pricing.input_cost_per_token, Some(0.0000015));
        assert_eq!(
            pricing.input_cost_per_token_above_200k_tokens,
            Some(0.000003)
        );
        assert_eq!(pricing.output_cost_per_token, Some(0.0000075));
        assert_eq!(
            pricing.output_cost_per_token_above_200k_tokens,
            Some(0.000015)
        );
        assert_eq!(pricing.cache_creation_input_token_cost, Some(0.000001875));
        assert_eq!(
            pricing.cache_creation_input_token_cost_above_200k_tokens,
            Some(0.00000375)
        );
        assert_eq!(pricing.cache_read_input_token_cost, Some(0.00000015));
        assert_eq!(
            pricing.cache_read_input_token_cost_above_200k_tokens,
            Some(0.0000003)
        );
    }

    #[test]
    fn test_deserialize_model_pricing_without_above_200k_fields() {
        let pricing: ModelPricing = serde_json::from_str(
            r#"{
                "input_cost_per_token": 0.00000125,
                "output_cost_per_token": 0.00001,
                "cache_creation_input_token_cost": 0.00000125,
                "cache_read_input_token_cost": 0.000000125
            }"#,
        )
        .unwrap();

        assert_eq!(pricing.input_cost_per_token, Some(0.00000125));
        assert_eq!(pricing.input_cost_per_token_above_200k_tokens, None);
        assert_eq!(pricing.output_cost_per_token, Some(0.00001));
        assert_eq!(pricing.output_cost_per_token_above_200k_tokens, None);
        assert_eq!(pricing.cache_creation_input_token_cost, Some(0.00000125));
        assert_eq!(
            pricing.cache_creation_input_token_cost_above_200k_tokens,
            None
        );
        assert_eq!(pricing.cache_read_input_token_cost, Some(0.000000125));
        assert_eq!(pricing.cache_read_input_token_cost_above_200k_tokens, None);
    }

    #[test]
    fn test_deserialize_model_pricing_with_above_272k_fields() {
        let pricing: ModelPricing = serde_json::from_str(
            r#"{
                "input_cost_per_token": 0.000005,
                "input_cost_per_token_above_272k_tokens": 0.000010,
                "output_cost_per_token": 0.000030,
                "output_cost_per_token_above_272k_tokens": 0.000045,
                "cache_read_input_token_cost": 0.0000005,
                "cache_read_input_token_cost_above_272k_tokens": 0.000001
            }"#,
        )
        .unwrap();

        assert_eq!(pricing.input_cost_per_token, Some(0.000005));
        assert_eq!(
            pricing.input_cost_per_token_above_272k_tokens,
            Some(0.000010)
        );
        assert_eq!(pricing.output_cost_per_token, Some(0.000030));
        assert_eq!(
            pricing.output_cost_per_token_above_272k_tokens,
            Some(0.000045)
        );
        assert_eq!(pricing.cache_read_input_token_cost, Some(0.0000005));
        assert_eq!(
            pricing.cache_read_input_token_cost_above_272k_tokens,
            Some(0.000001)
        );
    }
}
