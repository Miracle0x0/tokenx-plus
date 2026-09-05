use super::litellm::{ModelPricing, TimePeriodPrice};
use super::{CatalogSource, SourceOrder};
use crate::{model_aliases, provider_identity, TokenBreakdown};
use chrono::{Datelike, Timelike, Utc, Weekday};
use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

const MAX_LOOKUP_CACHE_ENTRIES: usize = 512;
const TIERED_PRICING_THRESHOLD_128K_TOKENS: f64 = 128_000.0;
const TIERED_PRICING_THRESHOLD_200K_TOKENS: f64 = 200_000.0;
const TIERED_PRICING_THRESHOLD_256K_TOKENS: f64 = 256_000.0;
const TIERED_PRICING_THRESHOLD_272K_TOKENS: f64 = 272_000.0;

#[derive(Clone)]
struct CachedResult {
    pricing: ModelPricing,
    pricing_source: String,
    matched_key: String,
}

pub struct PricingLookup {
    litellm: Arc<HashMap<String, ModelPricing>>,
    openrouter: Arc<HashMap<String, ModelPricing>>,
    models_dev: Arc<HashMap<String, ModelPricing>>,
    litellm_keys: Arc<[String]>,
    openrouter_keys: Arc<[String]>,
    models_dev_keys: Arc<[String]>,
    order: SourceOrder,
    lookup_cache: RwLock<HashMap<String, Option<CachedResult>>>,
}

pub struct LookupResult {
    pub pricing: ModelPricing,
    pub pricing_source: String,
    pub matched_key: String,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PricingComputationError {
    #[error("output and reasoning token counts exceed i64::MAX")]
    OutputReasoningTokenOverflow,
    #[error("invalid pricing timestamp in milliseconds: {timestamp_ms}")]
    InvalidTimestamp { timestamp_ms: i64 },
    #[error("pricing produced a non-finite {component} cost")]
    NonFiniteCost { component: &'static str },
}

impl PricingLookup {
    pub fn new(
        litellm: HashMap<String, ModelPricing>,
        openrouter: HashMap<String, ModelPricing>,
    ) -> Self {
        Self::new_with_models_dev(litellm, openrouter, HashMap::new())
    }

    pub fn new_with_models_dev(
        litellm: HashMap<String, ModelPricing>,
        openrouter: HashMap<String, ModelPricing>,
        models_dev: HashMap<String, ModelPricing>,
    ) -> Self {
        Self::new_with_models_dev_and_order(litellm, openrouter, models_dev, SourceOrder::default())
    }

    pub fn new_with_models_dev_and_order(
        litellm: HashMap<String, ModelPricing>,
        openrouter: HashMap<String, ModelPricing>,
        models_dev: HashMap<String, ModelPricing>,
        order: SourceOrder,
    ) -> Self {
        let litellm_keys = sorted_catalog_keys(&litellm);
        let openrouter_keys = sorted_catalog_keys(&openrouter);
        let models_dev_keys = sorted_catalog_keys(&models_dev);

        Self {
            litellm: Arc::new(litellm),
            openrouter: Arc::new(openrouter),
            models_dev: Arc::new(models_dev),
            litellm_keys: litellm_keys.into(),
            openrouter_keys: openrouter_keys.into(),
            models_dev_keys: models_dev_keys.into(),
            order,
            lookup_cache: RwLock::new(HashMap::with_capacity(64)),
        }
    }

    pub fn lookup(&self, model_id: &str) -> Option<LookupResult> {
        self.lookup_with_provider(model_id, None)
    }

    pub(super) fn with_order(&self, order: SourceOrder) -> Self {
        Self {
            litellm: Arc::clone(&self.litellm),
            openrouter: Arc::clone(&self.openrouter),
            models_dev: Arc::clone(&self.models_dev),
            litellm_keys: Arc::clone(&self.litellm_keys),
            openrouter_keys: Arc::clone(&self.openrouter_keys),
            models_dev_keys: Arc::clone(&self.models_dev_keys),
            order,
            lookup_cache: RwLock::new(HashMap::with_capacity(64)),
        }
    }

    pub fn lookup_with_provider(
        &self,
        model_id: &str,
        provider_id: Option<&str>,
    ) -> Option<LookupResult> {
        let canonical_model_id = model_aliases::canonicalize_model_id(model_id);
        let provider_scope = resolved_provider_scope(provider_id, &canonical_model_id);
        let cache_key = build_lookup_cache_key(&canonical_model_id, provider_scope);

        if let Some(cached) = self
            .lookup_cache
            .read()
            .ok()
            .and_then(|cache| cache.get(&cache_key).cloned())
        {
            return cached.map(|cached| LookupResult {
                pricing: cached.pricing,
                pricing_source: cached.pricing_source,
                matched_key: cached.matched_key,
            });
        }

        let result = self.lookup_canonical(&canonical_model_id, None, provider_scope);

        if let Ok(mut cache) = self.lookup_cache.write() {
            if cache.len() >= MAX_LOOKUP_CACHE_ENTRIES {
                let evict_count = cache.len() / 4;
                let keys_to_remove: Vec<String> = cache.keys().take(evict_count).cloned().collect();
                for key in keys_to_remove {
                    cache.remove(&key);
                }
            }
            cache.insert(
                cache_key,
                result.as_ref().map(|result| CachedResult {
                    pricing: result.pricing.clone(),
                    pricing_source: result.pricing_source.clone(),
                    matched_key: result.matched_key.clone(),
                }),
            );
        }

        result
    }

    pub fn lookup_with_pricing_source(
        &self,
        model_id: &str,
        forced_pricing_source: Option<&str>,
    ) -> Option<LookupResult> {
        self.lookup_with_pricing_source_and_provider(model_id, forced_pricing_source, None)
    }

    pub fn lookup_with_pricing_source_and_provider(
        &self,
        model_id: &str,
        forced_pricing_source: Option<&str>,
        provider_id: Option<&str>,
    ) -> Option<LookupResult> {
        let canonical_model_id = model_aliases::canonicalize_model_id(model_id);
        let provider_scope = resolved_provider_scope(provider_id, &canonical_model_id);
        self.lookup_canonical(&canonical_model_id, forced_pricing_source, provider_scope)
    }

    fn lookup_canonical(
        &self,
        model_id: &str,
        forced_pricing_source: Option<&str>,
        provider_scope: Option<&str>,
    ) -> Option<LookupResult> {
        match forced_pricing_source {
            None => self.lookup_auto(model_id, provider_scope),
            Some(source) if source.eq_ignore_ascii_case("litellm") => lookup_catalog(
                &self.litellm,
                &self.litellm_keys,
                "LiteLLM",
                model_id,
                provider_scope,
            ),
            Some(source) if source.eq_ignore_ascii_case("openrouter") => lookup_catalog(
                &self.openrouter,
                &self.openrouter_keys,
                "OpenRouter",
                model_id,
                provider_scope,
            ),
            Some(source) if source.eq_ignore_ascii_case("models.dev") => lookup_catalog(
                &self.models_dev,
                &self.models_dev_keys,
                "Models.dev",
                model_id,
                provider_scope,
            ),
            Some(_) => None,
        }
    }

    fn lookup_auto(&self, model_id: &str, provider_scope: Option<&str>) -> Option<LookupResult> {
        if model_aliases::is_deepseek_v4_model(model_id) {
            let time_priced = lookup_catalog(
                &self.openrouter,
                &self.openrouter_keys,
                "OpenRouter",
                model_id,
                provider_scope,
            )
            .filter(|result| has_time_period_pricing(&result.pricing));
            if time_priced.is_some() {
                return time_priced;
            }
        }

        if let Some(provider_scope) = provider_scope {
            for source in self.order.sources() {
                let (dataset, keys, label) = self.catalog(*source);
                if let Some(result) =
                    lookup_provider_scoped_exact(dataset, keys, label, model_id, provider_scope)
                {
                    return Some(result);
                }
            }
        }

        for source in self.order.sources() {
            let (dataset, keys, label) = self.catalog(*source);
            if let Some(result) = lookup_unscoped_exact(dataset, keys, label, model_id) {
                return Some(result);
            }
        }

        None
    }

    fn catalog(
        &self,
        source: CatalogSource,
    ) -> (&HashMap<String, ModelPricing>, &[String], &'static str) {
        match source {
            CatalogSource::Litellm => (&self.litellm, &self.litellm_keys, "LiteLLM"),
            CatalogSource::Openrouter => (&self.openrouter, &self.openrouter_keys, "OpenRouter"),
            CatalogSource::ModelsDev => (&self.models_dev, &self.models_dev_keys, "Models.dev"),
        }
    }

    pub fn calculate_cost(
        &self,
        model_id: &str,
        input: i64,
        output: i64,
        cache_read: i64,
        cache_write: i64,
        reasoning: i64,
    ) -> Result<f64, PricingComputationError> {
        let usage = TokenBreakdown {
            input,
            output,
            cache_read,
            cache_write,
            reasoning,
        };
        self.calculate_cost_with_provider(model_id, None, &usage)
    }

    pub fn calculate_cost_with_provider(
        &self,
        model_id: &str,
        provider_id: Option<&str>,
        usage: &TokenBreakdown,
    ) -> Result<f64, PricingComputationError> {
        self.calculate_cost_with_provider_and_time(model_id, provider_id, usage, None)
    }

    pub fn calculate_cost_with_provider_and_time(
        &self,
        model_id: &str,
        provider_id: Option<&str>,
        usage: &TokenBreakdown,
        timestamp_ms: Option<i64>,
    ) -> Result<f64, PricingComputationError> {
        let Some(result) = self.lookup_with_provider(model_id, provider_id) else {
            return Ok(0.0);
        };

        let pricing = if model_aliases::is_deepseek_v4_model(model_id) {
            effective_pricing_at(&result.pricing, timestamp_ms)?
        } else {
            Cow::Borrowed(&result.pricing)
        };

        compute_cost(
            &pricing,
            usage.input,
            usage.output,
            usage.cache_read,
            usage.cache_write,
            usage.reasoning,
        )
    }
}

fn sorted_catalog_keys(dataset: &HashMap<String, ModelPricing>) -> Vec<String> {
    let mut keys: Vec<String> = dataset.keys().cloned().collect();
    keys.sort_by(|left, right| {
        left.to_ascii_lowercase()
            .cmp(&right.to_ascii_lowercase())
            .then_with(|| left.cmp(right))
    });
    keys
}

fn resolved_provider_scope<'a>(provider_id: Option<&'a str>, model_id: &str) -> Option<&'a str> {
    if let Some(observed) = provider_id
        .map(str::trim)
        .filter(|provider| !provider.is_empty() && !provider.eq_ignore_ascii_case("unknown"))
    {
        return Some(observed);
    }

    provider_identity::inferred_provider_from_model(model_id)
}

fn lookup_catalog(
    dataset: &HashMap<String, ModelPricing>,
    keys: &[String],
    pricing_source: &str,
    model_id: &str,
    provider_scope: Option<&str>,
) -> Option<LookupResult> {
    provider_scope
        .and_then(|provider_scope| {
            lookup_provider_scoped_exact(dataset, keys, pricing_source, model_id, provider_scope)
        })
        .or_else(|| lookup_unscoped_exact(dataset, keys, pricing_source, model_id))
}

fn lookup_provider_scoped_exact(
    dataset: &HashMap<String, ModelPricing>,
    keys: &[String],
    pricing_source: &str,
    model_id: &str,
    provider_scope: &str,
) -> Option<LookupResult> {
    keys.iter()
        .filter(|key| {
            let Some((_, catalog_model_id)) = key.rsplit_once('/') else {
                return false;
            };
            catalog_model_id.eq_ignore_ascii_case(model_id)
                && provider_identity::matches_provider_hint(key, Some(provider_scope))
        })
        .find_map(|key| {
            dataset
                .get(key)
                .and_then(|pricing| lookup_result_if_usable(pricing, pricing_source, key))
        })
}

fn lookup_unscoped_exact(
    dataset: &HashMap<String, ModelPricing>,
    keys: &[String],
    pricing_source: &str,
    model_id: &str,
) -> Option<LookupResult> {
    keys.iter()
        .filter(|key| !key.contains('/') && key.eq_ignore_ascii_case(model_id))
        .find_map(|key| {
            dataset
                .get(key)
                .and_then(|pricing| lookup_result_if_usable(pricing, pricing_source, key))
        })
}

fn is_valid_price_value(value: f64) -> bool {
    value.is_finite() && value >= 0.0
}

fn has_any_usable_pricing(pricing: &ModelPricing) -> bool {
    [
        pricing.input_cost_per_token,
        pricing.output_cost_per_token,
        pricing.cache_read_input_token_cost,
        pricing.cache_creation_input_token_cost,
        pricing.input_cost_per_token_above_128k_tokens,
        pricing.input_cost_per_token_above_200k_tokens,
        pricing.input_cost_per_token_above_256k_tokens,
        pricing.input_cost_per_token_above_272k_tokens,
        pricing.output_cost_per_token_above_128k_tokens,
        pricing.output_cost_per_token_above_200k_tokens,
        pricing.output_cost_per_token_above_256k_tokens,
        pricing.output_cost_per_token_above_272k_tokens,
        pricing.cache_read_input_token_cost_above_200k_tokens,
        pricing.cache_read_input_token_cost_above_272k_tokens,
        pricing.cache_creation_input_token_cost_above_200k_tokens,
    ]
    .into_iter()
    .any(|price| price.is_some_and(is_valid_price_value))
        || pricing.time_period_prices.as_ref().is_some_and(|periods| {
            periods.iter().any(|period| {
                [
                    period.input_cost_per_token,
                    period.output_cost_per_token,
                    period.cache_read_input_token_cost,
                    period.cache_creation_input_token_cost,
                ]
                .into_iter()
                .any(|price| price.is_some_and(is_valid_price_value))
            })
        })
}

fn has_time_period_pricing(pricing: &ModelPricing) -> bool {
    pricing
        .time_period_prices
        .as_ref()
        .is_some_and(|periods| !periods.is_empty())
}

fn utc_weekday_name(weekday: Weekday) -> &'static str {
    match weekday {
        Weekday::Mon => "monday",
        Weekday::Tue => "tuesday",
        Weekday::Wed => "wednesday",
        Weekday::Thu => "thursday",
        Weekday::Fri => "friday",
        Weekday::Sat => "saturday",
        Weekday::Sun => "sunday",
    }
}

fn time_period_matches(period: &TimePeriodPrice, timestamp: &chrono::DateTime<Utc>) -> bool {
    if let Some(days) = period.utc_days.as_ref() {
        let weekday = utc_weekday_name(timestamp.weekday());
        if !days.iter().any(|day| day.eq_ignore_ascii_case(weekday)) {
            return false;
        }
    }

    match (period.utc_start, period.utc_end) {
        (None, None) => true,
        (Some(start), Some(end)) if start < end => {
            let hhmm = timestamp.hour() * 100 + timestamp.minute();
            hhmm >= start && hhmm < end
        }
        (Some(start), Some(end)) if start > end => {
            let hhmm = timestamp.hour() * 100 + timestamp.minute();
            hhmm >= start || hhmm < end
        }
        _ => false,
    }
}

fn effective_pricing_at(
    pricing: &ModelPricing,
    timestamp_ms: Option<i64>,
) -> Result<Cow<'_, ModelPricing>, PricingComputationError> {
    let Some(periods) = pricing.time_period_prices.as_ref() else {
        return Ok(Cow::Borrowed(pricing));
    };
    let Some(timestamp_ms) = timestamp_ms else {
        return Ok(Cow::Borrowed(pricing));
    };
    let timestamp = chrono::DateTime::<Utc>::from_timestamp_millis(timestamp_ms)
        .ok_or(PricingComputationError::InvalidTimestamp { timestamp_ms })?;

    let mut effective = None;
    for period in periods
        .iter()
        .filter(|period| time_period_matches(period, &timestamp))
    {
        let effective = effective.get_or_insert_with(|| pricing.clone());
        if let Some(price) = period.input_cost_per_token {
            effective.input_cost_per_token = Some(price);
        }
        if let Some(price) = period.output_cost_per_token {
            effective.output_cost_per_token = Some(price);
        }
        if let Some(price) = period.cache_read_input_token_cost {
            effective.cache_read_input_token_cost = Some(price);
        }
        if let Some(price) = period.cache_creation_input_token_cost {
            effective.cache_creation_input_token_cost = Some(price);
        }
    }

    Ok(match effective {
        Some(effective) => Cow::Owned(effective),
        None => Cow::Borrowed(pricing),
    })
}

fn lookup_result_if_usable(
    pricing: &ModelPricing,
    pricing_source: &str,
    matched_key: &str,
) -> Option<LookupResult> {
    has_any_usable_pricing(pricing).then(|| LookupResult {
        pricing: pricing.clone(),
        pricing_source: pricing_source.into(),
        matched_key: matched_key.into(),
    })
}

fn build_lookup_cache_key(model_id: &str, provider_scope: Option<&str>) -> String {
    match provider_scope {
        Some(provider_scope) => format!(
            "{}|{}",
            provider_scope.to_ascii_lowercase(),
            model_id.to_ascii_lowercase()
        ),
        None => model_id.to_ascii_lowercase(),
    }
}

pub fn compute_cost(
    pricing: &ModelPricing,
    input: i64,
    output: i64,
    cache_read: i64,
    cache_write: i64,
    reasoning: i64,
) -> Result<f64, PricingComputationError> {
    let safe_price = |price: Option<f64>| {
        price
            .filter(|value| is_valid_price_value(*value))
            .unwrap_or(0.0)
    };
    let tiered_cost = |tokens: f64,
                       base: Option<f64>,
                       tiers: &[(f64, Option<f64>)],
                       component: &'static str|
     -> Result<f64, PricingComputationError> {
        let mut cost = 0.0;
        let mut lower_bound = 0.0;
        let mut active_price = safe_price(base);

        for (threshold, tier_price) in tiers {
            let Some(tier_price) = tier_price.filter(|value| is_valid_price_value(*value)) else {
                continue;
            };

            if !threshold.is_finite() || *threshold <= lower_bound {
                continue;
            }

            if tokens <= *threshold {
                let cost = cost + (tokens - lower_bound).max(0.0) * active_price;
                return finite_cost(cost, component);
            }

            cost += (*threshold - lower_bound) * active_price;
            cost = finite_cost(cost, component)?;
            lower_bound = *threshold;
            active_price = tier_price;
        }

        finite_cost(
            cost + (tokens - lower_bound).max(0.0) * active_price,
            component,
        )
    };

    let input = input.max(0) as f64;
    let output = output
        .max(0)
        .checked_add(reasoning.max(0))
        .ok_or(PricingComputationError::OutputReasoningTokenOverflow)? as f64;
    let cache_read = cache_read.max(0) as f64;
    let cache_write = cache_write.max(0) as f64;

    let input_cost = tiered_cost(
        input,
        pricing.input_cost_per_token,
        &[
            (
                TIERED_PRICING_THRESHOLD_128K_TOKENS,
                pricing.input_cost_per_token_above_128k_tokens,
            ),
            (
                TIERED_PRICING_THRESHOLD_200K_TOKENS,
                pricing.input_cost_per_token_above_200k_tokens,
            ),
            (
                TIERED_PRICING_THRESHOLD_256K_TOKENS,
                pricing.input_cost_per_token_above_256k_tokens,
            ),
            (
                TIERED_PRICING_THRESHOLD_272K_TOKENS,
                pricing.input_cost_per_token_above_272k_tokens,
            ),
        ],
        "input",
    )?;
    let output_cost = tiered_cost(
        output,
        pricing.output_cost_per_token,
        &[
            (
                TIERED_PRICING_THRESHOLD_128K_TOKENS,
                pricing.output_cost_per_token_above_128k_tokens,
            ),
            (
                TIERED_PRICING_THRESHOLD_200K_TOKENS,
                pricing.output_cost_per_token_above_200k_tokens,
            ),
            (
                TIERED_PRICING_THRESHOLD_256K_TOKENS,
                pricing.output_cost_per_token_above_256k_tokens,
            ),
            (
                TIERED_PRICING_THRESHOLD_272K_TOKENS,
                pricing.output_cost_per_token_above_272k_tokens,
            ),
        ],
        "output",
    )?;
    let cache_read_cost = tiered_cost(
        cache_read,
        pricing.cache_read_input_token_cost,
        &[
            (
                TIERED_PRICING_THRESHOLD_200K_TOKENS,
                pricing.cache_read_input_token_cost_above_200k_tokens,
            ),
            (
                TIERED_PRICING_THRESHOLD_272K_TOKENS,
                pricing.cache_read_input_token_cost_above_272k_tokens,
            ),
        ],
        "cache-read",
    )?;
    let cache_write_cost = tiered_cost(
        cache_write,
        pricing.cache_creation_input_token_cost,
        &[(
            TIERED_PRICING_THRESHOLD_200K_TOKENS,
            pricing.cache_creation_input_token_cost_above_200k_tokens,
        )],
        "cache-write",
    )?;

    finite_cost(
        input_cost + output_cost + cache_read_cost + cache_write_cost,
        "total",
    )
}

fn finite_cost(cost: f64, component: &'static str) -> Result<f64, PricingComputationError> {
    if cost.is_finite() {
        Ok(cost)
    } else {
        Err(PricingComputationError::NonFiniteCost { component })
    }
}

#[cfg(test)]
#[path = "lookup_tests.rs"]
mod tests;
