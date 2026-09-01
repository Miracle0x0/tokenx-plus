use super::litellm::{ModelPricing, TimePeriodPrice};
use super::{cache, emit_warning, PricingDiagnosticSink, PricingDiagnostics};
use crate::model_aliases;
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::Semaphore;

const CACHE_FILENAME: &str = "pricing-openrouter.json";
const MODELS_URL: &str = "https://openrouter.ai/api/v1/models";
const MAX_RETRIES: u32 = 3;
const INITIAL_BACKOFF_MS: u64 = 200;
const MAX_CONCURRENT_REQUESTS: usize = 10;

/// Structs for `/api/v1/models` endpoint (list all models).

#[derive(Deserialize)]
struct PricingOverride {
    #[serde(default)]
    utc_days: Option<Vec<String>>,
    #[serde(default)]
    utc_start: Option<u32>,
    #[serde(default)]
    utc_end: Option<u32>,
    #[serde(default)]
    min_prompt_tokens: Option<u64>,
    #[serde(default)]
    prompt: Option<String>,
    #[serde(default)]
    completion: Option<String>,
    #[serde(default)]
    input_cache_read: Option<String>,
    #[serde(default)]
    input_cache_write: Option<String>,
    #[serde(flatten)]
    unrecognized_fields: HashMap<String, Value>,
}

#[derive(Deserialize)]
struct OpenRouterPricing {
    prompt: String,
    completion: String,
    #[serde(default)]
    input_cache_read: Option<String>,
    #[serde(default)]
    input_cache_write: Option<String>,
    #[serde(default)]
    overrides: Vec<PricingOverride>,
}

#[derive(Deserialize)]
struct ModelListItem {
    id: String,
    pricing: Option<OpenRouterPricing>,
}

#[derive(Deserialize)]
struct ModelsListResponse {
    data: Vec<ModelListItem>,
}

/// Structs for `/api/v1/models/{id}/endpoints` endpoint (author pricing).

#[derive(Deserialize)]
struct Endpoint {
    provider_name: String,
    pricing: OpenRouterPricing,
}

#[derive(Deserialize)]
struct EndpointData {
    #[allow(dead_code)]
    id: String,
    endpoints: Vec<Endpoint>,
}

#[derive(Deserialize)]
struct EndpointsResponse {
    data: EndpointData,
}

/// Model ID prefix to provider name mapping.
///
/// Translates model ID prefixes like `z-ai` to their corresponding
/// provider names in the endpoints API, such as `Z.AI`.
fn get_author_provider_name(model_id: &str) -> Option<&'static str> {
    let prefix = model_id.split('/').next()?;

    match prefix.to_lowercase().as_str() {
        "z-ai" => Some("Z.AI"),
        "x-ai" => Some("xAI"),
        "anthropic" => Some("Anthropic"),
        "openai" => Some("OpenAI"),
        "google" => Some("Google"),
        "meta-llama" => Some("Meta"),
        "mistralai" => Some("Mistral"),
        "deepseek" => Some("DeepSeek"),
        "qwen" => Some("Alibaba"),
        "cohere" => Some("Cohere"),
        "perplexity" => Some("Perplexity"),
        "moonshotai" => Some("Moonshot AI"),
        "xiaomi" => Some("Xiaomi"),
        _ => None,
    }
}

pub fn load_cached(cache_dir: &Path) -> Option<HashMap<String, ModelPricing>> {
    cache::load_cache(cache_dir, CACHE_FILENAME)
}

pub fn load_cached_any_age(cache_dir: &Path) -> Option<HashMap<String, ModelPricing>> {
    cache::load_cache_any_age(cache_dir, CACHE_FILENAME)
}

fn parse_price(s: &str) -> Option<f64> {
    s.trim()
        .parse::<f64>()
        .ok()
        .filter(|v| v.is_finite() && *v >= 0.0)
}

fn parse_optional_price(value: Option<&str>, field: &str) -> Result<Option<f64>, String> {
    value
        .map(|value| parse_price(value).ok_or_else(|| format!("invalid {field} price")))
        .transpose()
}

fn valid_hhmm(value: u32) -> bool {
    value <= 2359 && value % 100 < 60
}

fn normalize_utc_days(days: Vec<String>) -> Result<Vec<String>, String> {
    if days.is_empty() {
        return Err("utc_days must not be empty".to_string());
    }

    let mut normalized = Vec::with_capacity(days.len());
    for day in days {
        let day = day.trim().to_ascii_lowercase();
        if !matches!(
            day.as_str(),
            "monday" | "tuesday" | "wednesday" | "thursday" | "friday" | "saturday" | "sunday"
        ) {
            return Err(format!("invalid UTC weekday `{day}`"));
        }
        if normalized.iter().any(|existing| existing == &day) {
            return Err(format!("duplicate UTC weekday `{day}`"));
        }
        normalized.push(day);
    }
    Ok(normalized)
}

fn parse_time_period_prices(
    model_id: &str,
    overrides: Vec<PricingOverride>,
) -> Result<Option<Vec<TimePeriodPrice>>, String> {
    if !model_aliases::is_deepseek_v4_model(model_id) {
        return Ok(None);
    }

    let mut periods = Vec::new();
    for (index, pricing_override) in overrides.into_iter().enumerate() {
        let has_time_condition = pricing_override.utc_days.is_some()
            || pricing_override.utc_start.is_some()
            || pricing_override.utc_end.is_some();
        if !has_time_condition {
            continue;
        }
        if !pricing_override.unrecognized_fields.is_empty() {
            let mut fields: Vec<_> = pricing_override
                .unrecognized_fields
                .keys()
                .cloned()
                .collect();
            fields.sort();
            return Err(format!(
                "override {index} contains unsupported fields: {}",
                fields.join(", ")
            ));
        }
        if pricing_override.min_prompt_tokens.is_some() {
            return Err(format!(
                "override {index} combines time pricing with unsupported min_prompt_tokens"
            ));
        }

        let utc_days = pricing_override
            .utc_days
            .map(normalize_utc_days)
            .transpose()
            .map_err(|error| format!("override {index}: {error}"))?;
        let (utc_start, utc_end) = match (pricing_override.utc_start, pricing_override.utc_end) {
            (Some(start), Some(end)) => {
                if !valid_hhmm(start) || !valid_hhmm(end) || start == end {
                    return Err(format!(
                        "override {index} has invalid UTC HHMM window {start}-{end}"
                    ));
                }
                (Some(start), Some(end))
            }
            (None, None) => (None, None),
            _ => {
                return Err(format!(
                    "override {index} must define utc_start and utc_end together"
                ));
            }
        };

        let input_cost_per_token =
            parse_optional_price(pricing_override.prompt.as_deref(), "override prompt")
                .map_err(|error| format!("override {index}: {error}"))?;
        let output_cost_per_token = parse_optional_price(
            pricing_override.completion.as_deref(),
            "override completion",
        )
        .map_err(|error| format!("override {index}: {error}"))?;
        let cache_read_input_token_cost = parse_optional_price(
            pricing_override.input_cache_read.as_deref(),
            "override input_cache_read",
        )
        .map_err(|error| format!("override {index}: {error}"))?;
        let cache_creation_input_token_cost = parse_optional_price(
            pricing_override.input_cache_write.as_deref(),
            "override input_cache_write",
        )
        .map_err(|error| format!("override {index}: {error}"))?;

        if input_cost_per_token.is_none()
            && output_cost_per_token.is_none()
            && cache_read_input_token_cost.is_none()
            && cache_creation_input_token_cost.is_none()
        {
            continue;
        }

        periods.push(TimePeriodPrice {
            utc_days,
            utc_start,
            utc_end,
            input_cost_per_token,
            output_cost_per_token,
            cache_read_input_token_cost,
            cache_creation_input_token_cost,
        });
    }

    Ok((!periods.is_empty()).then_some(periods))
}

fn parse_openrouter_pricing(
    model_id: &str,
    pricing: OpenRouterPricing,
) -> Result<ModelPricing, String> {
    let input = parse_price(&pricing.prompt).ok_or_else(|| "invalid prompt price".to_string())?;
    let output =
        parse_price(&pricing.completion).ok_or_else(|| "invalid completion price".to_string())?;
    let cache_read_input_token_cost =
        parse_optional_price(pricing.input_cache_read.as_deref(), "input_cache_read")?;
    let cache_creation_input_token_cost =
        parse_optional_price(pricing.input_cache_write.as_deref(), "input_cache_write")?;
    let time_period_prices = parse_time_period_prices(model_id, pricing.overrides)?;

    Ok(ModelPricing {
        input_cost_per_token: Some(input),
        output_cost_per_token: Some(output),
        cache_read_input_token_cost,
        cache_creation_input_token_cost,
        time_period_prices,
        ..Default::default()
    })
}

async fn fetch_author_pricing(
    client: Arc<reqwest::Client>,
    model_id: String,
    semaphore: Arc<Semaphore>,
    author_name: &'static str,
) -> Result<(String, Option<ModelPricing>), String> {
    let _permit = semaphore
        .acquire()
        .await
        .expect("OpenRouter pricing semaphore should not be closed");

    let url = format!("https://openrouter.ai/api/v1/models/{}/endpoints", model_id);

    let response = client
        .get(&url)
        .header("Content-Type", "application/json")
        .send()
        .await
        .map_err(|err| format!("{model_id}: endpoints request failed: {err}"))?;

    let status = response.status();
    if !status.is_success() {
        return Err(format!("{model_id}: endpoints API returned {status}"));
    }

    let data: EndpointsResponse = response
        .json()
        .await
        .map_err(|err| format!("{model_id}: endpoints JSON parse failed: {err}"))?;

    // Find the endpoint from the author provider
    let author_endpoint = match data
        .data
        .endpoints
        .into_iter()
        .find(|e| e.provider_name == author_name)
    {
        Some(ep) => ep,
        None => return Ok((model_id, None)),
    };

    let pricing = parse_openrouter_pricing(&model_id, author_endpoint.pricing)
        .map_err(|error| format!("{model_id}: author endpoint {error}"))?;

    Ok((model_id, Some(pricing)))
}

fn select_models_for_author_pricing(model_ids: Vec<String>) -> Vec<(String, &'static str)> {
    model_ids
        .into_iter()
        .filter_map(|id| get_author_provider_name(&id).map(|author| (id, author)))
        .collect()
}

/// Fetch all models and get author pricing for each
pub async fn fetch_all_models(cache_dir: &Path) -> HashMap<String, ModelPricing> {
    let mut diagnostics = None;
    fetch_all_models_with_sink(cache_dir, MODELS_URL, true, &mut diagnostics)
        .await
        .unwrap_or_default()
}

pub(crate) async fn fetch_all_models_with_diagnostics(
    cache_dir: &Path,
    diagnostics: &mut PricingDiagnostics,
) -> HashMap<String, ModelPricing> {
    let mut diagnostics = Some(diagnostics);
    fetch_all_models_with_sink(cache_dir, MODELS_URL, true, &mut diagnostics)
        .await
        .unwrap_or_default()
}

pub(crate) async fn refresh_with_diagnostics(
    cache_dir: &Path,
    diagnostics: &mut PricingDiagnostics,
) -> Result<HashMap<String, ModelPricing>, String> {
    let mut diagnostics = Some(diagnostics);
    fetch_all_models_with_sink(cache_dir, MODELS_URL, false, &mut diagnostics).await
}

async fn fetch_all_models_with_sink(
    cache_dir: &Path,
    models_url: &str,
    use_cache: bool,
    diagnostics: &mut PricingDiagnosticSink<'_>,
) -> Result<HashMap<String, ModelPricing>, String> {
    if use_cache {
        if let Some(cached) = load_cached(cache_dir) {
            return Ok(cached);
        }
    }

    let client = Arc::new(
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .connect_timeout(std::time::Duration::from_secs(10))
            .build()
            .expect("valid OpenRouter HTTP client configuration"),
    );

    let mut last_error: Option<String> = None;

    let model_items = 'retry: {
        for attempt in 0..MAX_RETRIES {
            let response = match client
                .get(models_url)
                .header("Content-Type", "application/json")
                .send()
                .await
            {
                Ok(r) => r,
                Err(e) => {
                    last_error = Some(format!("network error: {}", e));
                    if attempt < MAX_RETRIES - 1 {
                        tokio::time::sleep(std::time::Duration::from_millis(
                            INITIAL_BACKOFF_MS * (1 << attempt),
                        ))
                        .await;
                    }
                    continue;
                }
            };

            let status = response.status();
            if status.is_server_error() || status == reqwest::StatusCode::TOO_MANY_REQUESTS {
                last_error = Some(format!("HTTP {}", status));
                let _ = response.bytes().await;
                if attempt < MAX_RETRIES - 1 {
                    tokio::time::sleep(std::time::Duration::from_millis(
                        INITIAL_BACKOFF_MS * (1 << attempt),
                    ))
                    .await;
                }
                continue;
            }

            if !status.is_success() {
                let error = format!("models API returned {status}");
                emit_warning(diagnostics, format!("[tokenx] OpenRouter {error}"));
                break 'retry Err(error);
            }

            let data: ModelsListResponse = match response.json().await {
                Ok(d) => d,
                Err(e) => {
                    let error = format!("models JSON parse failed: {e}");
                    emit_warning(diagnostics, format!("[tokenx] OpenRouter {error}"));
                    break 'retry Err(error);
                }
            };

            break 'retry Ok(data.data);
        }

        let error = last_error.expect("OpenRouter retries end only after a request error");
        emit_warning(
            diagnostics,
            format!(
                "[tokenx] OpenRouter fetch failed after {} retries: {}",
                MAX_RETRIES, error
            ),
        );
        Err(error)
    }?;

    if model_items.is_empty() {
        return Ok(HashMap::new());
    }

    let mut result = HashMap::new();
    let mut model_ids = Vec::with_capacity(model_items.len());
    for model in model_items {
        if let Some(pricing) = model.pricing {
            match parse_openrouter_pricing(&model.id, pricing) {
                Ok(pricing) => {
                    result.insert(model.id.clone(), pricing);
                }
                Err(error) => emit_warning(
                    diagnostics,
                    format!("[tokenx] OpenRouter {} pricing skipped: {error}", model.id),
                ),
            }
        }
        model_ids.push(model.id);
    }

    let models_for_author_pricing = select_models_for_author_pricing(model_ids);

    let semaphore = Arc::new(Semaphore::new(MAX_CONCURRENT_REQUESTS));

    let mut handles = Vec::with_capacity(models_for_author_pricing.len());

    for (model_id, author_name) in models_for_author_pricing {
        let client = Arc::clone(&client);
        let sem = Arc::clone(&semaphore);

        let handle =
            tokio::spawn(
                async move { fetch_author_pricing(client, model_id, sem, author_name).await },
            );

        handles.push(handle);
    }

    for handle in handles {
        match handle.await {
            Ok(Ok((model_id, Some(pricing)))) => {
                result.insert(model_id, pricing);
            }
            Ok(Ok((_model_id, None))) => {}
            Ok(Err(err)) => emit_warning(
                diagnostics,
                format!("[tokenx] OpenRouter author pricing skipped: {err}"),
            ),
            Err(err) => emit_warning(
                diagnostics,
                format!("[tokenx] OpenRouter author pricing task failed: {err}"),
            ),
        }
    }

    if !result.is_empty() {
        if let Err(e) = cache::save_cache(cache_dir, CACHE_FILENAME, &result) {
            let cache_path = cache::get_cache_path(cache_dir, CACHE_FILENAME)
                .display()
                .to_string();
            emit_warning(
                diagnostics,
                format!(
                    "[tokenx] Warning: Failed to cache OpenRouter pricing at {}: {}",
                    cache_path, e
                ),
            );
        }
    }

    Ok(result)
}

pub async fn fetch_all_mapped(cache_dir: &Path) -> HashMap<String, ModelPricing> {
    fetch_all_models(cache_dir).await
}

pub(crate) async fn fetch_all_mapped_with_diagnostics(
    cache_dir: &Path,
    diagnostics: &mut PricingDiagnostics,
) -> HashMap<String, ModelPricing> {
    fetch_all_models_with_diagnostics(cache_dir, diagnostics).await
}

#[cfg(test)]
mod tests {
    use super::{
        fetch_all_models_with_sink, get_author_provider_name, load_cached,
        parse_openrouter_pricing, select_models_for_author_pricing, ModelsListResponse,
        OpenRouterPricing, CACHE_FILENAME, MAX_RETRIES,
    };
    use std::collections::HashMap;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    fn retryable_status_server() -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        thread::spawn(move || {
            for _ in 0..MAX_RETRIES {
                let Ok((mut stream, _)) = listener.accept() else {
                    return;
                };
                let mut buffer = [0; 1024];
                let _ = stream.read(&mut buffer);
                let response =
                    "HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                let _ = stream.write_all(response.as_bytes());
            }
        });
        url
    }

    #[tokio::test]
    async fn refresh_propagates_models_list_failure() {
        let url = retryable_status_server();
        let cache_dir = tempfile::TempDir::new().unwrap();
        let mut diagnostics = Vec::new();
        let mut sink = Some(&mut diagnostics);

        let result = fetch_all_models_with_sink(cache_dir.path(), &url, false, &mut sink).await;

        assert!(result.is_err());
        assert!(diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message().contains("OpenRouter fetch failed")));
    }

    #[test]
    fn maps_xiaomi_models_to_openrouter_author_provider() {
        assert_eq!(
            get_author_provider_name("xiaomi/mimo-v2.5-pro"),
            Some("Xiaomi")
        );
    }

    #[test]
    fn selects_only_models_with_author_provider_for_endpoint_enrichment() {
        let selected = select_models_for_author_pricing(vec![
            "relace/relace-apply-3".to_string(),
            "unknown/no-price".to_string(),
            "openai/gpt-5".to_string(),
        ]);

        let selected_ids: Vec<&str> = selected.iter().map(|(id, _)| id.as_str()).collect();

        assert!(selected_ids.contains(&"openai/gpt-5"));
        assert!(!selected_ids.contains(&"relace/relace-apply-3"));
        assert!(!selected_ids.contains(&"unknown/no-price"));
    }

    #[test]
    fn parses_model_list_pricing_as_baseline_pricing() {
        let pricing = parse_openrouter_pricing(
            "openai/gpt-5",
            OpenRouterPricing {
                prompt: "0.00000085".to_string(),
                completion: "0.00000125".to_string(),
                input_cache_read: None,
                input_cache_write: None,
                overrides: Vec::new(),
            },
        )
        .expect("valid model list pricing");

        assert_eq!(pricing.input_cost_per_token, Some(0.00000085));
        assert_eq!(pricing.output_cost_per_token, Some(0.00000125));
    }

    #[test]
    fn rejects_invalid_deepseek_v4_time_window() {
        let pricing: OpenRouterPricing = serde_json::from_value(serde_json::json!({
            "prompt": "0.00000085",
            "completion": "0.00000125",
            "overrides": [{
                "utc_start": 1260,
                "utc_end": 1400,
                "prompt": "0.00000042"
            }]
        }))
        .unwrap();

        let error = parse_openrouter_pricing("deepseek/deepseek-v4-flash", pricing).unwrap_err();

        assert!(error.contains("invalid UTC HHMM window"));
    }

    #[test]
    fn rejects_unknown_conditions_in_deepseek_v4_time_override() {
        let pricing: OpenRouterPricing = serde_json::from_value(serde_json::json!({
            "prompt": "0.00000085",
            "completion": "0.00000125",
            "overrides": [{
                "utc_start": 100,
                "utc_end": 400,
                "future_condition": true,
                "prompt": "0.00000042"
            }]
        }))
        .unwrap();

        let error = parse_openrouter_pricing("deepseek/deepseek-v4-flash", pricing).unwrap_err();

        assert!(error.contains("unsupported fields: future_condition"));
    }

    #[test]
    fn parses_deepseek_v4_time_period_pricing_fixture() {
        let response: ModelsListResponse = serde_json::from_str(include_str!(
            "../../tests/fixtures/openrouter_deepseek_v4_pricing.json"
        ))
        .unwrap();
        let model = response.data.into_iter().next().unwrap();
        let pricing = parse_openrouter_pricing(&model.id, model.pricing.unwrap()).unwrap();
        let periods = pricing.time_period_prices.as_ref().unwrap();

        assert_eq!(pricing.cache_read_input_token_cost, Some(0.000000014));
        assert_eq!(periods.len(), 6);
        assert_eq!(periods[0].utc_days.as_ref().unwrap()[0], "saturday");
        assert_eq!(periods[1].utc_start, Some(0));
        assert_eq!(periods[1].utc_end, Some(100));
        assert_eq!(periods[5].utc_start, Some(1000));
        assert_eq!(periods[5].utc_end, Some(0));
        assert_eq!(periods[5].input_cost_per_token, Some(0.00000022));
    }

    #[test]
    fn openrouter_cache_round_trip_preserves_time_period_prices() {
        let response: ModelsListResponse = serde_json::from_str(include_str!(
            "../../tests/fixtures/openrouter_deepseek_v4_pricing.json"
        ))
        .unwrap();
        let model = response.data.into_iter().next().unwrap();
        let model_id = model.id.clone();
        let pricing = parse_openrouter_pricing(&model.id, model.pricing.unwrap()).unwrap();
        let cache_dir = tempfile::tempdir().unwrap();

        super::cache::save_cache(
            cache_dir.path(),
            CACHE_FILENAME,
            &HashMap::from([(model_id.clone(), pricing)]),
        )
        .unwrap();
        let cached = load_cached(cache_dir.path()).unwrap();

        assert_eq!(
            cached[&model_id].time_period_prices.as_ref().unwrap().len(),
            6
        );
    }
}
