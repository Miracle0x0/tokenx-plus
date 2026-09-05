use anyhow::Result;
use tokenx_engine::pricing::TimePeriodPrice;

fn format_hhmm(value: u32) -> String {
    format!("{:02}:{:02}", value / 100, value % 100)
}

fn localized_utc_day(day: &str) -> String {
    match day.to_ascii_lowercase().as_str() {
        "monday" => rust_i18n::t!("commands.pricing.day_monday").into_owned(),
        "tuesday" => rust_i18n::t!("commands.pricing.day_tuesday").into_owned(),
        "wednesday" => rust_i18n::t!("commands.pricing.day_wednesday").into_owned(),
        "thursday" => rust_i18n::t!("commands.pricing.day_thursday").into_owned(),
        "friday" => rust_i18n::t!("commands.pricing.day_friday").into_owned(),
        "saturday" => rust_i18n::t!("commands.pricing.day_saturday").into_owned(),
        "sunday" => rust_i18n::t!("commands.pricing.day_sunday").into_owned(),
        _ => day.to_string(),
    }
}

fn format_time_period_scope(period: &TimePeriodPrice) -> String {
    let days = period
        .utc_days
        .as_ref()
        .map(|days| {
            days.iter()
                .map(|day| localized_utc_day(day))
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_else(|| rust_i18n::t!("commands.pricing.every_day").into_owned());
    let window = match (period.utc_start, period.utc_end) {
        (Some(start), Some(end)) => format!("{}-{}", format_hhmm(start), format_hhmm(end)),
        (None, None) => rust_i18n::t!("commands.pricing.all_day").into_owned(),
        (start, end) => format!("{start:?}-{end:?}"),
    };
    format!("{days}  {window}")
}

pub(crate) async fn run_pricing_lookup(
    paths: &crate::product_paths::ProductPaths,
    model_id: &str,
    json: bool,
    pricing_source: Option<&str>,
    no_spinner: bool,
) -> Result<()> {
    use colored::Colorize;
    use indicatif::ProgressBar;
    use indicatif::ProgressStyle;
    use tokenx_engine::pricing::PricingService;
    let settings = crate::settings::Settings::load(paths)?;

    let pricing_source_normalized = pricing_source.map(|value| value.to_lowercase());

    let spinner = if no_spinner {
        None
    } else {
        let message = match pricing_source {
            Some(source) => {
                rust_i18n::t!("commands.pricing.fetching_from", source = source).into_owned()
            }
            None => rust_i18n::t!("commands.pricing.fetching").into_owned(),
        };
        let pb = ProgressBar::new_spinner();
        pb.set_style(ProgressStyle::default_spinner());
        pb.set_message(message);
        pb.enable_steady_tick(std::time::Duration::from_millis(100));
        Some(pb)
    };

    let result = match async {
        let svc = PricingService::fetch_current_with_order(
            &paths.custom_pricing_file(),
            &paths.cache_dir(),
            settings.pricing_source_order,
        )
        .await?;
        Ok::<_, String>(
            svc.lookup_with_pricing_source(model_id, pricing_source_normalized.as_deref()),
        )
    }
    .await
    {
        Ok(result) => result,
        Err(err) => {
            if let Some(pb) = spinner {
                pb.finish_and_clear();
            }
            return Err(anyhow::anyhow!(err));
        }
    };

    if let Some(pb) = spinner {
        pb.finish_and_clear();
    }

    if json {
        match result {
            Some(pricing) => {
                #[derive(serde::Serialize)]
                #[serde(rename_all = "camelCase")]
                struct PricingValues {
                    input_cost_per_token: f64,
                    output_cost_per_token: f64,
                    #[serde(skip_serializing_if = "Option::is_none")]
                    cache_read_input_token_cost: Option<f64>,
                    #[serde(skip_serializing_if = "Option::is_none")]
                    cache_creation_input_token_cost: Option<f64>,
                    #[serde(skip_serializing_if = "Option::is_none")]
                    time_period_prices: Option<Vec<TimePeriodPricingValues>>,
                }

                #[derive(serde::Serialize)]
                #[serde(rename_all = "camelCase")]
                struct TimePeriodPricingValues {
                    #[serde(skip_serializing_if = "Option::is_none")]
                    utc_days: Option<Vec<String>>,
                    #[serde(skip_serializing_if = "Option::is_none")]
                    utc_start: Option<u32>,
                    #[serde(skip_serializing_if = "Option::is_none")]
                    utc_end: Option<u32>,
                    #[serde(skip_serializing_if = "Option::is_none")]
                    input_cost_per_token: Option<f64>,
                    #[serde(skip_serializing_if = "Option::is_none")]
                    output_cost_per_token: Option<f64>,
                    #[serde(skip_serializing_if = "Option::is_none")]
                    cache_read_input_token_cost: Option<f64>,
                    #[serde(skip_serializing_if = "Option::is_none")]
                    cache_creation_input_token_cost: Option<f64>,
                }

                #[derive(serde::Serialize)]
                #[serde(rename_all = "camelCase")]
                struct PricingOutput {
                    model_id: String,
                    matched_key: String,
                    pricing_source: String,
                    pricing: PricingValues,
                }

                let output = PricingOutput {
                    model_id: model_id.to_string(),
                    matched_key: pricing.matched_key,
                    pricing_source: pricing.pricing_source,
                    pricing: PricingValues {
                        input_cost_per_token: pricing.pricing.input_cost_per_token.unwrap_or(0.0),
                        output_cost_per_token: pricing.pricing.output_cost_per_token.unwrap_or(0.0),
                        cache_read_input_token_cost: pricing.pricing.cache_read_input_token_cost,
                        cache_creation_input_token_cost: pricing
                            .pricing
                            .cache_creation_input_token_cost,
                        time_period_prices: pricing.pricing.time_period_prices.as_ref().map(
                            |periods| {
                                periods
                                    .iter()
                                    .map(|period| TimePeriodPricingValues {
                                        utc_days: period.utc_days.clone(),
                                        utc_start: period.utc_start,
                                        utc_end: period.utc_end,
                                        input_cost_per_token: period.input_cost_per_token,
                                        output_cost_per_token: period.output_cost_per_token,
                                        cache_read_input_token_cost: period
                                            .cache_read_input_token_cost,
                                        cache_creation_input_token_cost: period
                                            .cache_creation_input_token_cost,
                                    })
                                    .collect()
                            },
                        ),
                    },
                };

                println!("{}", serde_json::to_string_pretty(&output)?);
            }
            None => {
                return Err(anyhow::anyhow!(rust_i18n::t!(
                    "commands.pricing.model_not_found",
                    model_id = model_id
                )));
            }
        }
    } else {
        match result {
            Some(pricing) => {
                println!(
                    "\n  {}",
                    rust_i18n::t!("commands.pricing.lookup_for", model_id = model_id.bold())
                );
                println!(
                    "  {}",
                    rust_i18n::t!("commands.pricing.matched_key", key = pricing.matched_key)
                );
                let pricing_source_label = match pricing.pricing_source.to_lowercase().as_str() {
                    "custom" => rust_i18n::t!("commands.pricing.source_custom").into_owned(),
                    "litellm" => "LiteLLM".to_string(),
                    "openrouter" => "OpenRouter".to_string(),
                    "models.dev" => "Models.dev".to_string(),
                    _ => pricing.pricing_source.clone(),
                };
                println!(
                    "  {}",
                    rust_i18n::t!(
                        "commands.pricing.source_label",
                        source = pricing_source_label
                    )
                );
                println!();
                let input = pricing.pricing.input_cost_per_token.unwrap_or(0.0);
                let output = pricing.pricing.output_cost_per_token.unwrap_or(0.0);
                println!(
                    "  {}",
                    rust_i18n::t!(
                        "commands.pricing.rate_input",
                        price = format!("${:.2}", input * 1_000_000.0)
                    )
                );
                println!(
                    "  {}",
                    rust_i18n::t!(
                        "commands.pricing.rate_output",
                        price = format!("${:.2}", output * 1_000_000.0)
                    )
                );
                if let Some(cache_read) = pricing.pricing.cache_read_input_token_cost {
                    println!(
                        "  {}",
                        rust_i18n::t!(
                            "commands.pricing.rate_cache_read",
                            price = format!("${:.2}", cache_read * 1_000_000.0)
                        )
                    );
                }
                if let Some(cache_write) = pricing.pricing.cache_creation_input_token_cost {
                    println!(
                        "  {}",
                        rust_i18n::t!(
                            "commands.pricing.rate_cache_write",
                            price = format!("${:.2}", cache_write * 1_000_000.0)
                        )
                    );
                }
                if let Some(periods) = pricing.pricing.time_period_prices.as_ref() {
                    println!();
                    println!("  {}", rust_i18n::t!("commands.pricing.time_periods_title"));
                    for period in periods {
                        println!("    {}", format_time_period_scope(period));
                        if let Some(input) = period.input_cost_per_token {
                            println!(
                                "      {}",
                                rust_i18n::t!(
                                    "commands.pricing.rate_input",
                                    price = format!("${:.2}", input * 1_000_000.0)
                                )
                            );
                        }
                        if let Some(output) = period.output_cost_per_token {
                            println!(
                                "      {}",
                                rust_i18n::t!(
                                    "commands.pricing.rate_output",
                                    price = format!("${:.2}", output * 1_000_000.0)
                                )
                            );
                        }
                        if let Some(cache_read) = period.cache_read_input_token_cost {
                            println!(
                                "      {}",
                                rust_i18n::t!(
                                    "commands.pricing.rate_cache_read",
                                    price = format!("${:.2}", cache_read * 1_000_000.0)
                                )
                            );
                        }
                        if let Some(cache_write) = period.cache_creation_input_token_cost {
                            println!(
                                "      {}",
                                rust_i18n::t!(
                                    "commands.pricing.rate_cache_write",
                                    price = format!("${:.2}", cache_write * 1_000_000.0)
                                )
                            );
                        }
                    }
                }
                println!();
            }
            None => {
                return Err(anyhow::anyhow!(
                    "{}",
                    rust_i18n::t!("commands.pricing.model_not_found", model_id = model_id)
                        .into_owned()
                        .red()
                ));
            }
        }
    }

    Ok(())
}

pub(crate) fn run_pricing_list_overrides(
    paths: &crate::product_paths::ProductPaths,
    json: bool,
) -> Result<()> {
    use colored::Colorize;
    use tokenx_engine::pricing::custom::CustomPricing;
    use tokenx_engine::pricing::ModelPricing;

    fn per_million(value: Option<f64>) -> Option<f64> {
        value.map(|v| v * 1_000_000.0)
    }

    #[derive(serde::Serialize)]
    #[serde(rename_all = "camelCase")]
    struct OverrideEntry {
        model_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        input_cost_per_million_tokens: Option<f64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        output_cost_per_million_tokens: Option<f64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_read_input_token_cost_per_million_tokens: Option<f64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_creation_input_token_cost_per_million_tokens: Option<f64>,
    }

    fn entry(model_id: &str, pricing: &ModelPricing) -> OverrideEntry {
        OverrideEntry {
            model_id: model_id.to_string(),
            input_cost_per_million_tokens: per_million(pricing.input_cost_per_token),
            output_cost_per_million_tokens: per_million(pricing.output_cost_per_token),
            cache_read_input_token_cost_per_million_tokens: per_million(
                pricing.cache_read_input_token_cost,
            ),
            cache_creation_input_token_cost_per_million_tokens: per_million(
                pricing.cache_creation_input_token_cost,
            ),
        }
    }

    let path = paths.custom_pricing_file();
    let overrides = CustomPricing::load_from_path(&path);
    let mut entries: Vec<OverrideEntry> = overrides
        .entries()
        .map(|(model_id, pricing)| entry(model_id, pricing))
        .collect();
    entries.sort_by(|a, b| a.model_id.cmp(&b.model_id));

    if json {
        #[derive(serde::Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Output {
            path: String,
            count: usize,
            models: Vec<OverrideEntry>,
        }

        println!(
            "{}",
            serde_json::to_string_pretty(&Output {
                path: path.display().to_string(),
                count: entries.len(),
                models: entries,
            })?
        );
        return Ok(());
    }

    if entries.is_empty() {
        println!(
            "\n  {}\n  {}\n",
            rust_i18n::t!("commands.pricing.no_overrides")
                .into_owned()
                .yellow(),
            rust_i18n::t!("commands.pricing.tried_path", path = path.display())
        );
        return Ok(());
    }

    println!(
        "\n  {}",
        rust_i18n::t!("commands.pricing.overrides_title")
            .into_owned()
            .bold()
    );
    println!(
        "  {}",
        rust_i18n::t!("commands.pricing.overrides_path", path = path.display())
    );
    println!("  {}", rust_i18n::t!("commands.pricing.overrides_note"));
    println!();

    for entry in entries {
        println!("  {}", entry.model_id.bold());
        if let Some(input) = entry.input_cost_per_million_tokens {
            println!(
                "    {}",
                rust_i18n::t!(
                    "commands.pricing.rate_input",
                    price = format!("${:.2}", input)
                )
            );
        }
        if let Some(output) = entry.output_cost_per_million_tokens {
            println!(
                "    {}",
                rust_i18n::t!(
                    "commands.pricing.rate_output",
                    price = format!("${:.2}", output)
                )
            );
        }
        if let Some(cache_read) = entry.cache_read_input_token_cost_per_million_tokens {
            println!(
                "    {}",
                rust_i18n::t!(
                    "commands.pricing.rate_cache_read",
                    price = format!("${:.2}", cache_read)
                )
            );
        }
        if let Some(cache_write) = entry.cache_creation_input_token_cost_per_million_tokens {
            println!(
                "    {}",
                rust_i18n::t!(
                    "commands.pricing.rate_cache_write",
                    price = format!("${:.2}", cache_write)
                )
            );
        }
    }
    println!();

    Ok(())
}
