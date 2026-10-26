use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::*;
use crate::acquisition::{acquisition_engine, build_generation};
use crate::tui::data::{AgentEntry, DailyUsage, HourlyUsage};
use crate::tui::data::{DailyClientInfo, DailyModelInfo, UsageModelEntry, UsageTokenBreakdown};
use chrono::NaiveDate;
use serial_test::serial;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

type ClientModelCosts<'a> = Vec<(&'a str, Vec<(&'a str, &'a str, f64)>)>;

fn config_with_theme(theme: Option<&str>) -> TuiConfig {
    TuiConfig {
        theme: theme.map(|value| value.parse().unwrap()),
        refresh: 0,
        no_refresh: false,
        client_universe: tokenx_engine::ClientUniverse::all(),
        initial_tab: None,
        effective_date: chrono::NaiveDate::from_ymd_opt(2026, 7, 26).unwrap(),
    }
}

#[test]
fn offline_subscription_entry_and_manual_fetch_never_queue_network_work() {
    let mut config = config_with_theme(None);
    config.initial_tab = Some(Tab::Subscription);
    let mut settings = Settings::default();
    settings.subscription.enabled = true;
    settings.subscription.providers = vec![ProviderId::Codex];
    let root = tempfile::tempdir().unwrap();
    let mut app = TuiModel::new(
        config,
        settings,
        ProductPaths::at(root.path().to_owned()),
        true,
    )
    .unwrap();
    assert!(app.take_subscription_request().is_none());
    assert!(!app.is_fetching_subscription());
    app.fetch_subscription();
    assert!(app.take_subscription_request().is_none());
    assert!(app.subscription_status_message.is_some());
}

fn cached_order_model(id: &str, tokens: u64) -> UsageModelEntry {
    UsageModelEntry {
        machine: None,
        cost_provenance: Default::default(),
        model_id: id.into(),
        display_name: id.into(),
        provider: "openai".into(),
        clients: vec![ClientId::Codex],
        tokens: UsageTokenBreakdown {
            input: tokens,
            ..UsageTokenBreakdown::default()
        },
        cost: tokens as f64,
        session_count: 1,
        workspace_key: None,
        workspace_label: None,
    }
}

#[test]
fn render_order_cache_reuses_matching_keys_and_invalidates_semantic_changes() {
    let mut app = TuiModel::new_for_test(config_with_theme(None)).unwrap();
    app.usage_mut_for_test().models = vec![
        cached_order_model("slow", 1),
        cached_order_model("fast", 10),
    ];

    let first = app.model_render_order();
    let repeated = app.model_render_order();
    assert!(Arc::ptr_eq(&first, &repeated));

    app.sort_direction = SortDirection::Ascending;
    let resorted = app.model_render_order();
    assert!(!Arc::ptr_eq(&first, &resorted));

    app.model_detail_models = Some(vec![
        cached_order_model("fast", 3),
        cached_order_model("other", 4),
    ]);
    app.selected_model_detail = Some(ModelDetailSelection {
        model: "fast".to_string(),
        client: None,
    });
    let detail = app.model_render_order();
    assert!(!Arc::ptr_eq(&resorted, &detail));
    assert_eq!(detail.len(), 1);

    app.selected_model_detail = None;
    app.usage_mut_for_test().models[0].cost = 99.0;
    let replaced = app.model_render_order();
    assert!(!Arc::ptr_eq(&resorted, &replaced));
}

#[test]
fn daily_and_hourly_orders_share_only_an_identical_revision_and_sort_key() {
    let mut app = TuiModel::new_for_test(config_with_theme(None)).unwrap();
    let date = NaiveDate::from_ymd_opt(2026, 7, 26).unwrap();
    app.usage_mut_for_test().daily = vec![DailyUsage {
        cost_provenance: Default::default(),
        date,
        tokens: UsageTokenBreakdown::default(),
        cost: 1.0,
        client_breakdown: BTreeMap::new(),
        message_count: 0,
        turn_count: 0,
    }];
    app.usage_mut_for_test().hourly = vec![HourlyUsage {
        datetime: date.and_hms_opt(10, 0, 0).unwrap(),
        tokens: UsageTokenBreakdown::default(),
        cost: 1.0,
        clients: BTreeSet::new(),
        models: Vec::new(),
        message_count: 0,
        turn_count: 0,
    }];

    let daily = app.daily_render_order();
    let hourly = app.hourly_render_order();
    assert!(Arc::ptr_eq(&daily, &app.daily_render_order()));
    assert!(Arc::ptr_eq(&hourly, &app.hourly_render_order()));

    app.sort_field = SortField::Tokens;
    assert!(!Arc::ptr_eq(&daily, &app.daily_render_order()));
    assert!(!Arc::ptr_eq(&hourly, &app.hourly_render_order()));

    let before_revision = app.usage_revision;
    app.update_data(UsageProjection::default());
    assert_ne!(app.usage_revision, before_revision);
    assert!(app.daily_render_order().is_empty());
    assert!(app.hourly_render_order().is_empty());
}

#[test]
fn saved_theme_is_used_when_cli_theme_is_absent() {
    let settings = Settings {
        color_palette: ThemeName::Lagoon,
        ..Settings::default()
    };

    let app = TuiModel::new_for_test_with_settings(config_with_theme(None), settings).unwrap();

    assert_eq!(app.theme.name, ThemeName::Lagoon);
}

#[test]
fn explicit_cli_theme_overrides_saved_theme() {
    let settings = Settings {
        color_palette: ThemeName::Lagoon,
        ..Settings::default()
    };

    let app = TuiModel::new_for_test_with_settings(config_with_theme(Some("graphite")), settings)
        .unwrap();

    assert_eq!(app.theme.name, ThemeName::Graphite);
}

#[test]
fn app_consumes_the_passed_settings_snapshot_without_reading_disk() {
    let temp = tempfile::TempDir::new().unwrap();
    let path = temp.path().join("settings.json");
    std::fs::write(&path, b"{not valid json").unwrap();
    let settings = Settings {
        color_palette: ThemeName::Lagoon,
        auto_refresh_enabled: true,
        auto_refresh_ms: 40_000,
        ..Settings::default()
    };

    let app = TuiModel::new_for_test_with_settings_and_paths(
        config_with_theme(None),
        settings,
        ProductPaths::at(temp.path()),
    )
    .unwrap();

    assert_eq!(app.theme.name, ThemeName::Lagoon);
    assert!(app.auto_refresh_enabled());
    assert_eq!(app.auto_refresh_interval(), Duration::from_secs(40));
}

#[test]
fn test_tab_all() {
    let tabs = Tab::all();
    assert_eq!(tabs.len(), 10);
    assert_eq!(tabs[0], Tab::Overview);
    assert_eq!(tabs[1], Tab::Subscription);
    assert_eq!(tabs[2], Tab::Models);
    assert_eq!(tabs[3], Tab::Monthly);
    assert_eq!(tabs[4], Tab::Weekly);
    assert_eq!(tabs[5], Tab::Daily);
    assert_eq!(tabs[6], Tab::Hourly);
    assert_eq!(tabs[7], Tab::Stats);
    assert_eq!(tabs[8], Tab::Agents);
    assert_eq!(tabs[9], Tab::Sessions);
}

#[test]
fn test_tab_next() {
    assert_eq!(Tab::Overview.next(), Tab::Subscription);
    assert_eq!(Tab::Subscription.next(), Tab::Models);
    assert_eq!(Tab::Models.next(), Tab::Monthly);
    assert_eq!(Tab::Monthly.next(), Tab::Weekly);
    assert_eq!(Tab::Weekly.next(), Tab::Daily);
    assert_eq!(Tab::Daily.next(), Tab::Hourly);
    assert_eq!(Tab::Hourly.next(), Tab::Stats);
    assert_eq!(Tab::Stats.next(), Tab::Agents);
    assert_eq!(Tab::Agents.next(), Tab::Sessions);
    assert_eq!(Tab::Sessions.next(), Tab::Overview);
}

#[test]
fn test_tab_prev() {
    assert_eq!(Tab::Overview.prev(), Tab::Sessions);
    assert_eq!(Tab::Subscription.prev(), Tab::Overview);
    assert_eq!(Tab::Models.prev(), Tab::Subscription);
    assert_eq!(Tab::Monthly.prev(), Tab::Models);
    assert_eq!(Tab::Weekly.prev(), Tab::Monthly);
    assert_eq!(Tab::Daily.prev(), Tab::Weekly);
    assert_eq!(Tab::Hourly.prev(), Tab::Daily);
    assert_eq!(Tab::Stats.prev(), Tab::Hourly);
    assert_eq!(Tab::Agents.prev(), Tab::Stats);
    assert_eq!(Tab::Sessions.prev(), Tab::Agents);
}

#[test]
fn test_tab_as_str() {
    assert_eq!(Tab::Overview.as_str(), "Overview");
    assert_eq!(Tab::Subscription.as_str(), "Subscription");
    assert_eq!(Tab::Models.as_str(), "Models");
    assert_eq!(Tab::Agents.as_str(), "Agents");
    assert_eq!(Tab::Monthly.as_str(), "Monthly");
    assert_eq!(Tab::Weekly.as_str(), "Weekly");
    assert_eq!(Tab::Daily.as_str(), "Daily");
    assert_eq!(Tab::Hourly.as_str(), "Hourly");
    assert_eq!(Tab::Stats.as_str(), "Stats");
    assert_eq!(Tab::Sessions.as_str(), "Sessions");
}

#[test]
fn test_tab_short_name() {
    assert_eq!(Tab::Overview.short_name(), "Ovw");
    assert_eq!(Tab::Subscription.short_name(), "Sub");
    assert_eq!(Tab::Models.short_name(), "Mod");
    assert_eq!(Tab::Agents.short_name(), "Agt");
    assert_eq!(Tab::Monthly.short_name(), "Mth");
    assert_eq!(Tab::Weekly.short_name(), "Wk");
    assert_eq!(Tab::Daily.short_name(), "Day");
    assert_eq!(Tab::Hourly.short_name(), "Hr");
    assert_eq!(Tab::Stats.short_name(), "Sta");
    assert_eq!(Tab::Sessions.short_name(), "Ses");
}

#[test]
fn test_move_selection_up() {
    let mut app = make_app_with_models(2);

    app.set_selected_index(1);
    app.move_selection_up();
    assert_eq!(app.selected_index(), 0);

    // At top boundary - wraps to last item (index 1)
    app.move_selection_up();
    assert_eq!(app.selected_index(), 1);
}

#[test]
fn test_move_selection_down() {
    let mut app = make_app_with_models(2);

    app.set_selected_index(0);
    app.move_selection_down();
    assert_eq!(app.selected_index(), 1);

    // At bottom boundary - wraps to first item (index 0)
    app.move_selection_down();
    assert_eq!(app.selected_index(), 0);
    assert_eq!(app.scroll_offset(), 0);
}

#[test]
fn test_clamp_selection() {
    let mut app = make_app_with_models(1);

    app.set_selected_index(10);
    app.clamp_selection();
    assert_eq!(app.selected_index(), 0);

    // Empty data
    app.usage_mut_for_test().models.clear();
    app.set_selected_index(5);
    app.clamp_selection();
    assert_eq!(app.selected_index(), 0);
    assert_eq!(app.scroll_offset(), 0);
}

#[test]
fn test_set_sort() {
    let mut app = make_app();

    // Initial state
    assert_eq!(app.sort_field, SortField::Cost);
    assert_eq!(app.sort_direction, SortDirection::Descending);

    // Change to different field
    app.set_sort(SortField::Tokens);
    assert_eq!(app.sort_field, SortField::Tokens);
    assert_eq!(app.sort_direction, SortDirection::Descending);

    // Toggle same field
    app.set_sort(SortField::Tokens);
    assert_eq!(app.sort_field, SortField::Tokens);
    assert_eq!(app.sort_direction, SortDirection::Ascending);

    // Toggle again
    app.set_sort(SortField::Tokens);
    assert_eq!(app.sort_field, SortField::Tokens);
    assert_eq!(app.sort_direction, SortDirection::Descending);
}

// ── Helper ──────────────────────────────────────────────────────

fn test_settings() -> Settings {
    Settings::default()
}

fn load_test_generation() -> (tempfile::TempDir, tokenx_engine::Generation) {
    let home = tempfile::TempDir::new().unwrap();
    for (project, workspace, input_tokens) in
        [("project-a", "/work/a", 10), ("project-b", "/work/b", 20)]
    {
        let project_dir = home.path().join(".claude/projects").join(project);
        std::fs::create_dir_all(&project_dir).unwrap();
        std::fs::write(
                project_dir.join("session.jsonl"),
                format!(
                    r#"{{"type":"assistant","timestamp":"2026-05-27T10:00:00.000Z","cwd":"{workspace}","requestId":"request-{project}","message":{{"id":"message-{project}","model":"claude-sonnet-4.6","usage":{{"input_tokens":{input_tokens},"output_tokens":1}}}}}}"#
                ),
            )
            .unwrap();
    }
    let acquisition = acquisition_engine(
        home.path().join(".tokenx-test-cache"),
        home.path().to_path_buf(),
        ClientUniverse::new([ClientId::Claude]).unwrap(),
        tokenx_engine::DateRange::none(),
        tokenx_engine::scanner::ScannerSettings::default(),
        tokenx_engine::CalendarContext::explicit("UTC").unwrap(),
        crate::acquisition::test_pricing_snapshot(),
    )
    .unwrap();
    let prepared = acquisition.prepare().unwrap();
    let generation = build_generation(&acquisition, prepared).unwrap();
    (home, generation)
}

fn make_app() -> TuiModel {
    let config = TuiConfig {
        theme: Some(ThemeName::Blue),
        refresh: 0,
        no_refresh: false,
        client_universe: tokenx_engine::ClientUniverse::all(),
        initial_tab: None,
        effective_date: chrono::NaiveDate::from_ymd_opt(2026, 7, 26).unwrap(),
    };
    TuiModel::new_for_test_with_settings(config, test_settings()).unwrap()
}

fn make_app_with_subscription() -> TuiModel {
    let mut settings = test_settings();
    settings.subscription.enabled = true;
    make_app_with_settings(settings)
}

fn make_app_with_subscription_providers(providers: &[&str]) -> TuiModel {
    let mut settings = test_settings();
    settings.subscription.enabled = true;
    settings.subscription.providers = providers
        .iter()
        .map(|provider| {
            serde_json::from_value(serde_json::Value::String((*provider).to_string())).unwrap()
        })
        .collect();
    make_app_with_settings(settings)
}

fn make_app_with_settings(settings: Settings) -> TuiModel {
    let config = TuiConfig {
        theme: Some(ThemeName::Blue),
        refresh: 0,
        no_refresh: false,
        client_universe: tokenx_engine::ClientUniverse::all(),
        initial_tab: None,
        effective_date: chrono::NaiveDate::from_ymd_opt(2026, 7, 26).unwrap(),
    };
    TuiModel::new_for_test_with_settings(config, settings).unwrap()
}

#[test]
fn test_app_no_filter_default_uses_catalog() {
    let app = make_app();
    let actual = app.selected_clients().collect::<HashSet<_>>();
    let expected: HashSet<ClientId> = ClientId::iter().collect();
    assert_eq!(
        actual, expected,
        "no-filter TUI must select exactly the accepted client catalog"
    );
    assert!(actual.contains(&ClientId::Claude));
    assert_eq!(app.client_universe().as_hash_set(), expected);
}

#[test]
fn explicit_clients_define_an_immutable_client_universe() {
    let config = TuiConfig {
        theme: Some(ThemeName::Blue),
        refresh: 0,
        no_refresh: false,
        client_universe: ClientUniverse::new([ClientId::Claude, ClientId::Codex]).unwrap(),
        initial_tab: None,
        effective_date: chrono::NaiveDate::from_ymd_opt(2026, 7, 26).unwrap(),
    };
    let app = TuiModel::new_for_test_with_settings(config, test_settings()).unwrap();
    let expected = HashSet::from([ClientId::Claude, ClientId::Codex]);

    assert_eq!(
        app.client_universe(),
        ClientUniverse::new([ClientId::Claude, ClientId::Codex]).unwrap()
    );
    assert_eq!(app.selected_clients().collect::<HashSet<_>>(), expected);
}

#[test]
fn installed_generation_has_one_input_footprint_for_overview_and_sessions() {
    let config = TuiConfig {
        theme: Some(ThemeName::Blue),
        refresh: 0,
        no_refresh: false,
        client_universe: ClientUniverse::new([ClientId::Claude, ClientId::Codex]).unwrap(),
        initial_tab: None,
        effective_date: chrono::NaiveDate::from_ymd_opt(2026, 7, 26).unwrap(),
    };
    let mut app = TuiModel::new_for_test_with_settings(config, test_settings()).unwrap();
    let footprint = tokenx_engine::InputFootprint::from_client_bytes([
        (ClientId::Claude, 13),
        (ClientId::Codex, 8),
    ])
    .unwrap();

    app.install_generation_fixture(
        tokenx_engine::FrozenUsageIndex::new(),
        Vec::new(),
        footprint,
    );

    let session_total = app
        .session_snapshot()
        .client_summaries()
        .iter()
        .map(|summary| summary.space_bytes)
        .sum::<u64>();
    assert_eq!(app.total_input_bytes(), 21);
    assert_eq!(app.total_input_bytes(), session_total);
}

fn make_app_with_models(n: usize) -> TuiModel {
    let mut app = make_app();
    app.usage_mut_for_test().models = (0..n)
        .map(|i| UsageModelEntry {
            machine: None,
            cost_provenance: Default::default(),
            model_id: format!("model{}", i).into(),
            display_name: format!("model{}", i).into(),
            provider: "provider".into(),
            clients: vec![ClientId::OpenCode],
            tokens: UsageTokenBreakdown::default(),
            cost: 0.0,
            session_count: 1,
            workspace_key: None,
            workspace_label: None,
        })
        .collect();
    app
}

fn model_detail_accumulator() -> tokenx_engine::FrozenUsageIndex {
    let messages = [
        (
            tokenx_engine::ClientId::Claude,
            "shared-model",
            "anthropic",
            "claude-anthropic",
            11,
        ),
        (
            tokenx_engine::ClientId::Claude,
            "shared-model",
            "openrouter",
            "claude-openrouter",
            22,
        ),
        (
            tokenx_engine::ClientId::Codex,
            "shared-model",
            "openai",
            "codex-openai",
            33,
        ),
        (
            tokenx_engine::ClientId::Claude,
            "other-model",
            "anthropic",
            "claude-other",
            7,
        ),
    ]
    .map(|(client, model, provider, session, input)| {
        tokenx_engine::AttributedUsageRecord::new(
            client,
            model,
            provider,
            session,
            1_800_000_000,
            tokenx_engine::TokenBreakdown {
                input,
                ..Default::default()
            },
            input as f64 / 100.0,
        )
    });
    tokenx_engine::build_usage_index(
        &messages,
        tokenx_engine::DateRange::none(),
        tokenx_engine::CalendarContext::explicit("UTC").unwrap(),
    )
    .unwrap()
}

fn make_app_with_model_projection(group_by: tokenx_engine::GroupBy) -> TuiModel {
    let accumulator = model_detail_accumulator();
    let mut app = make_app();
    app.current_tab = Tab::Models;
    app.set_group_by_for_test(group_by);
    app.install_generation_fixture(accumulator, Vec::new(), Default::default());
    app
}

fn daily_usage(date: &str, cost: f64, models: Vec<(&str, &str, f64)>) -> DailyUsage {
    daily_usage_by_client(date, cost, vec![("claude", models)])
}

fn daily_usage_by_client(date: &str, cost: f64, clients: ClientModelCosts<'_>) -> DailyUsage {
    let mut client_breakdown = BTreeMap::new();
    let mut total_tokens = UsageTokenBreakdown::default();
    let mut total_cost = 0.0;

    for (client, models) in clients {
        let mut model_breakdown = Vec::new();
        let mut client_tokens = UsageTokenBreakdown::default();
        let mut client_cost = 0.0;

        for (model, provider, model_cost) in models {
            let tokens = UsageTokenBreakdown {
                input: (model_cost * 100.0) as u64,
                output: 10,
                cache_read: 5,
                cache_write: 0,
                reasoning: 0,
            };
            client_tokens.input = client_tokens.input.saturating_add(tokens.input);
            client_tokens.output = client_tokens.output.saturating_add(tokens.output);
            client_tokens.cache_read = client_tokens.cache_read.saturating_add(tokens.cache_read);
            total_tokens.input = total_tokens.input.saturating_add(tokens.input);
            total_tokens.output = total_tokens.output.saturating_add(tokens.output);
            total_tokens.cache_read = total_tokens.cache_read.saturating_add(tokens.cache_read);
            client_cost += model_cost;
            total_cost += model_cost;

            model_breakdown.push(DailyModelInfo {
                machine: None,
                provider: provider.into(),
                model_id: model.into(),
                display_name: model.into(),
                workspace_key: None,
                workspace_label: None,
                tokens,
                cost: model_cost,
                messages: 1,
            });
        }

        client_breakdown.insert(
            ClientId::from_str(client).expect("test client must be accepted"),
            DailyClientInfo {
                tokens: client_tokens,
                cost: client_cost,
                models: model_breakdown,
            },
        );
    }

    DailyUsage {
        cost_provenance: Default::default(),
        date: NaiveDate::parse_from_str(date, "%Y-%m-%d").unwrap(),
        tokens: total_tokens,
        cost: if cost > 0.0 { cost } else { total_cost },
        client_breakdown,
        message_count: 1,
        turn_count: 1,
    }
}

fn usage_data_with_graph_for_today(
    graph_today: NaiveDate,
    activity_date: NaiveDate,
) -> UsageProjection {
    let daily = vec![daily_usage(
        &activity_date.format("%Y-%m-%d").to_string(),
        1.0,
        vec![("gpt-5.4", "openai", 1.0)],
    )];
    let graph = tokenx_engine::build_contribution_graph_for_today(&daily, graph_today).unwrap();
    UsageProjection {
        daily,
        graph,
        ..Default::default()
    }
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn key_with_mod(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
    KeyEvent::new(code, modifiers)
}

// ── handle_key_event: quit ──────────────────────────────────────

#[test]
fn test_handle_key_quit_q() {
    let mut app = make_app();
    let outcome = app.handle_key_event(key(KeyCode::Char('q')));
    assert_eq!(outcome, KeyEventOutcome::Exit(TuiExit::Quit));
}

#[test]
fn test_handle_key_quit_ctrl_c() {
    let mut app = make_app();
    let outcome = app.handle_key_event(key_with_mod(KeyCode::Char('c'), KeyModifiers::CONTROL));
    assert_eq!(outcome, KeyEventOutcome::Exit(TuiExit::Interrupted));
}

#[test]
fn test_dialog_ctrl_c_still_global_quit() {
    let mut app = make_app();
    app.install_generation_fixture(
        tokenx_engine::FrozenUsageIndex::new(),
        Vec::new(),
        Default::default(),
    );
    app.open_client_picker();

    let outcome = app.handle_key_event(key_with_mod(KeyCode::Char('c'), KeyModifiers::CONTROL));

    assert_eq!(outcome, KeyEventOutcome::Exit(TuiExit::Interrupted));
    assert!(app.dialog_stack.is_active());
}

// ── handle_key_event: tab switching ─────────────────────────────

#[test]
fn test_handle_key_tab_switch() {
    let mut app = make_app();
    assert_eq!(app.current_tab, Tab::Overview);

    app.handle_key_event(key(KeyCode::Tab));
    assert_eq!(app.current_tab, Tab::Models);

    app.handle_key_event(key(KeyCode::Tab));
    assert_eq!(app.current_tab, Tab::Monthly);

    app.handle_key_event(key(KeyCode::Tab));
    assert_eq!(app.current_tab, Tab::Weekly);

    app.handle_key_event(key(KeyCode::Tab));
    assert_eq!(app.current_tab, Tab::Daily);

    app.handle_key_event(key(KeyCode::Tab));
    assert_eq!(app.current_tab, Tab::Hourly);

    app.handle_key_event(key(KeyCode::Tab));
    assert_eq!(app.current_tab, Tab::Stats);

    app.handle_key_event(key(KeyCode::Tab));
    assert_eq!(app.current_tab, Tab::Agents);

    app.handle_key_event(key(KeyCode::Tab));
    assert_eq!(app.current_tab, Tab::Sessions);

    app.handle_key_event(key(KeyCode::Tab));
    assert_eq!(app.current_tab, Tab::Overview);
}

#[test]
fn test_handle_key_backtab_switch() {
    let mut app = make_app();
    assert_eq!(app.current_tab, Tab::Overview);

    app.handle_key_event(key(KeyCode::BackTab));
    assert_eq!(app.current_tab, Tab::Sessions);

    app.handle_key_event(key(KeyCode::BackTab));
    assert_eq!(app.current_tab, Tab::Agents);

    app.handle_key_event(key(KeyCode::BackTab));
    assert_eq!(app.current_tab, Tab::Stats);

    app.handle_key_event(key(KeyCode::BackTab));
    assert_eq!(app.current_tab, Tab::Hourly);

    app.handle_key_event(key(KeyCode::BackTab));
    assert_eq!(app.current_tab, Tab::Daily);

    app.handle_key_event(key(KeyCode::BackTab));
    assert_eq!(app.current_tab, Tab::Weekly);

    app.handle_key_event(key(KeyCode::BackTab));
    assert_eq!(app.current_tab, Tab::Monthly);

    app.handle_key_event(key(KeyCode::BackTab));
    assert_eq!(app.current_tab, Tab::Models);

    app.handle_key_event(key(KeyCode::BackTab));
    assert_eq!(app.current_tab, Tab::Overview);
}

#[test]
fn test_handle_key_tab_switch_with_subscription_enabled_includes_subscription() {
    let mut app = make_app_with_subscription();
    assert_eq!(app.current_tab, Tab::Overview);

    for expected in [
        Tab::Subscription,
        Tab::Models,
        Tab::Monthly,
        Tab::Weekly,
        Tab::Daily,
        Tab::Hourly,
        Tab::Stats,
        Tab::Agents,
        Tab::Sessions,
        Tab::Overview,
    ] {
        app.handle_key_event(key(KeyCode::Tab));
        assert_eq!(app.current_tab, expected);
    }
}

#[test]
fn cli_no_refresh_overrides_enabled_setting_for_this_run() {
    let config = TuiConfig {
        theme: Some(ThemeName::Blue),
        refresh: 0,
        no_refresh: true,
        client_universe: tokenx_engine::ClientUniverse::all(),
        initial_tab: None,
        effective_date: chrono::NaiveDate::from_ymd_opt(2026, 7, 26).unwrap(),
    };
    let settings = Settings {
        auto_refresh_enabled: true,
        ..Settings::default()
    };

    let app = TuiModel::new_for_test_with_settings(config, settings).unwrap();

    assert!(!app.auto_refresh_enabled());
    assert!(app.settings.auto_refresh_enabled);
}

#[test]
fn test_initial_subscription_tab_fails_when_flag_off() {
    let config = TuiConfig {
        theme: Some(ThemeName::Blue),
        refresh: 0,
        no_refresh: false,
        client_universe: tokenx_engine::ClientUniverse::all(),
        initial_tab: Some(Tab::Subscription),
        effective_date: chrono::NaiveDate::from_ymd_opt(2026, 7, 26).unwrap(),
    };
    let result = TuiModel::new_for_test_with_settings(config, Settings::default());
    let error = match result {
        Ok(_) => panic!("a disabled explicit tab must not silently fall back"),
        Err(error) => error,
    };
    assert!(error.to_string().contains("disabled in settings.json"));
}

#[test]
fn test_get_sorted_agents_by_cost_desc() {
    let mut app = make_app();
    app.usage_mut_for_test().agents = vec![
        AgentEntry {
            cost_provenance: Default::default(),
            agent: "builder".into(),
            client: ClientId::OpenCode,
            tokens: UsageTokenBreakdown {
                input: 10,
                output: 5,
                cache_read: 0,
                cache_write: 0,
                reasoning: 0,
            },
            cost: 3.0,
            message_count: 1,
            instance_count: 1,
        },
        AgentEntry {
            cost_provenance: Default::default(),
            agent: "reviewer".into(),
            client: ClientId::RooCode,
            tokens: UsageTokenBreakdown {
                input: 50,
                output: 20,
                cache_read: 0,
                cache_write: 0,
                reasoning: 0,
            },
            cost: 7.0,
            message_count: 2,
            instance_count: 2,
        },
    ];

    let agents = app.get_sorted_agents();
    assert_eq!(agents[0].agent.as_ref(), "reviewer");
    assert_eq!(agents[1].agent.as_ref(), "builder");
}

#[test]
fn test_get_sorted_agents_by_tokens_asc() {
    let mut app = make_app();
    app.sort_field = SortField::Tokens;
    app.sort_direction = SortDirection::Ascending;
    app.usage_mut_for_test().agents = vec![
        AgentEntry {
            cost_provenance: Default::default(),
            agent: "builder".into(),
            client: ClientId::OpenCode,
            tokens: UsageTokenBreakdown {
                input: 100,
                output: 0,
                cache_read: 0,
                cache_write: 0,
                reasoning: 0,
            },
            cost: 1.0,
            message_count: 1,
            instance_count: 1,
        },
        AgentEntry {
            cost_provenance: Default::default(),
            agent: "reviewer".into(),
            client: ClientId::RooCode,
            tokens: UsageTokenBreakdown {
                input: 20,
                output: 0,
                cache_read: 0,
                cache_write: 0,
                reasoning: 0,
            },
            cost: 5.0,
            message_count: 1,
            instance_count: 1,
        },
    ];

    let agents = app.get_sorted_agents();
    assert_eq!(agents[0].agent.as_ref(), "reviewer");
    assert_eq!(agents[1].agent.as_ref(), "builder");
}

#[test]
fn test_handle_key_left_right_switch() {
    let mut app = make_app();
    app.handle_key_event(key(KeyCode::Right));
    assert_eq!(app.current_tab, Tab::Models);

    app.handle_key_event(key(KeyCode::Left));
    assert_eq!(app.current_tab, Tab::Overview);
}

#[test]
fn test_handle_key_left_right_switch_with_subscription_enabled() {
    let mut app = make_app_with_subscription();
    app.handle_key_event(key(KeyCode::Right));
    assert_eq!(app.current_tab, Tab::Subscription);

    app.handle_key_event(key(KeyCode::Right));
    assert_eq!(app.current_tab, Tab::Models);

    app.handle_key_event(key(KeyCode::Left));
    assert_eq!(app.current_tab, Tab::Subscription);
}

#[test]
fn test_handle_key_tab_restores_target_tab_selection() {
    let mut app = make_app_with_models(5);
    app.switch_tab(Tab::Models);
    app.set_max_visible_items(3);
    app.set_selected_index(3);
    app.set_scroll_offset(1);
    app.switch_tab(Tab::Overview);
    app.selected_graph_cell = Some((2, 4));

    app.handle_key_event(key(KeyCode::Tab));
    assert_eq!(app.current_tab, Tab::Models);
    assert_eq!(app.selected_index(), 3);
    assert_eq!(app.scroll_offset(), 1);
    assert_eq!(app.selected_graph_cell, None);
}

#[test]
fn test_switch_tab_preserves_each_tab_list_interaction() {
    let mut app = make_app_with_models(5);
    app.usage_mut_for_test().daily = vec![
        daily_usage("2026-05-10", 1.0, vec![("old-model", "anthropic", 1.0)]),
        daily_usage("2026-05-17", 7.0, vec![("target", "openai", 7.0)]),
        daily_usage("2026-05-18", 3.0, vec![("other", "google", 3.0)]),
    ];

    app.switch_tab(Tab::Models);
    app.set_max_visible_items(3);
    app.set_selected_index(4);
    app.set_scroll_offset(2);

    app.switch_tab(Tab::Daily);
    app.set_max_visible_items(2);
    app.set_selected_index(1);
    app.set_scroll_offset(1);

    app.switch_tab(Tab::Models);
    assert_eq!(app.selected_index(), 4);
    assert_eq!(app.scroll_offset(), 2);

    app.switch_tab(Tab::Daily);
    assert_eq!(app.selected_index(), 1);
    assert_eq!(app.scroll_offset(), 1);
}

#[test]
fn test_switch_tab_from_daily_detail_restores_daily_parent_state() {
    let mut app = make_app_with_models(5);
    app.current_tab = Tab::Daily;
    app.sort_field = SortField::Date;
    app.sort_direction = SortDirection::Descending;
    app.usage_mut_for_test().daily = vec![
        daily_usage("2026-05-10", 1.0, vec![("old-model", "anthropic", 1.0)]),
        daily_usage("2026-05-17", 7.0, vec![("target", "openai", 7.0)]),
        daily_usage("2026-05-18", 3.0, vec![("other", "google", 3.0)]),
    ];

    app.set_max_visible_items(2);
    app.set_selected_index(1);
    app.set_scroll_offset(1);
    app.handle_key_event(key(KeyCode::Enter));
    assert!(app.is_daily_detail_active());

    app.switch_tab(Tab::Models);
    assert!(!app.is_daily_detail_active());

    app.switch_tab(Tab::Daily);
    assert_eq!(app.selected_index(), 1);
    assert_eq!(app.scroll_offset(), 1);
}

#[test]
fn test_handle_key_tab_resets_selection_when_target_has_no_saved_state() {
    let mut app = make_app_with_models(5);
    app.set_selected_index(3);
    app.set_scroll_offset(1);

    app.handle_key_event(key(KeyCode::Tab));
    assert_eq!(app.selected_index(), 0);
    assert_eq!(app.scroll_offset(), 0);
}

#[test]
fn test_enter_on_daily_opens_selected_day_detail_rows() {
    let mut app = make_app();
    app.current_tab = Tab::Daily;
    app.sort_field = SortField::Date;
    app.sort_direction = SortDirection::Descending;
    app.usage_mut_for_test().daily = vec![
        daily_usage("2026-05-10", 1.0, vec![("old-model", "anthropic", 1.0)]),
        daily_usage(
            "2026-05-17",
            7.0,
            vec![("target-a", "openai", 5.0), ("target-b", "anthropic", 2.0)],
        ),
        daily_usage("2026-05-18", 3.0, vec![("other-model", "google", 3.0)]),
    ];

    app.set_selected_index(0);
    app.handle_key_event(key(KeyCode::Down));
    app.handle_key_event(key(KeyCode::Enter));

    assert_eq!(app.current_list_len(), 2);
}

#[test]
fn test_enter_on_daily_detail_uses_token_sort_default() {
    let mut app = make_app();
    app.current_tab = Tab::Daily;
    app.sort_field = SortField::Date;
    app.sort_direction = SortDirection::Descending;
    app.usage_mut_for_test().daily = vec![daily_usage(
        "2026-05-17",
        8.0,
        vec![
            ("a-low-token", "anthropic", 1.0),
            ("z-high-token", "openai", 7.0),
        ],
    )];

    app.handle_key_event(key(KeyCode::Enter));

    assert_eq!(app.sort_field, SortField::Tokens);
    assert_eq!(app.sort_direction, SortDirection::Descending);
    assert_eq!(app.daily_detail_rows()[0].model.as_ref(), "z-high-token");
}

#[test]
fn daily_detail_reuses_materialized_rows_and_sort_order() {
    let mut app = make_app();
    app.current_tab = Tab::Daily;
    app.usage_mut_for_test().daily = vec![daily_usage(
        "2026-05-17",
        8.0,
        vec![
            ("a-low-token", "anthropic", 1.0),
            ("z-high-token", "openai", 7.0),
        ],
    )];

    app.handle_key_event(key(KeyCode::Enter));
    let first = app.daily_detail_rows();
    let first_rows = first.rows.as_ptr();
    let first_order = Arc::clone(&first.order);
    let repeated = app.daily_detail_rows();

    assert_eq!(repeated.rows.as_ptr(), first_rows);
    assert!(Arc::ptr_eq(&first_order, &repeated.order));

    app.handle_key_event(key(KeyCode::Esc));
    app.handle_key_event(key(KeyCode::Enter));
    let reopened = app.daily_detail_rows();
    assert_eq!(reopened.rows.as_ptr(), first_rows);
}

#[test]
fn test_esc_from_daily_detail_restores_daily_selection() {
    let mut app = make_app();
    app.current_tab = Tab::Daily;
    app.sort_field = SortField::Date;
    app.sort_direction = SortDirection::Descending;
    app.usage_mut_for_test().daily = vec![
        daily_usage("2026-05-10", 1.0, vec![("old-model", "anthropic", 1.0)]),
        daily_usage(
            "2026-05-17",
            7.0,
            vec![("target-a", "openai", 5.0), ("target-b", "anthropic", 2.0)],
        ),
        daily_usage("2026-05-18", 3.0, vec![("other-model", "google", 3.0)]),
    ];

    app.set_max_visible_items(2);
    app.set_selected_index(1);
    app.set_scroll_offset(1);
    app.handle_key_event(key(KeyCode::Enter));
    app.handle_key_event(key(KeyCode::Down));
    assert_eq!(app.selected_index(), 1);
    assert_eq!(app.sort_field, SortField::Tokens);
    assert_eq!(app.sort_direction, SortDirection::Descending);
    app.set_max_visible_items(5);

    app.handle_key_event(key(KeyCode::Esc));

    assert_eq!(app.current_tab, Tab::Daily);
    assert_eq!(app.sort_field, SortField::Date);
    assert_eq!(app.sort_direction, SortDirection::Descending);
    assert_eq!(app.selected_index(), 1);
    assert_eq!(app.scroll_offset(), 1);
    assert_eq!(app.max_visible_items(), 2);
    assert_eq!(app.stored_list_interaction(Tab::Daily).visible, 2);
    assert_eq!(app.current_list_len(), 3);
}

#[test]
fn test_close_daily_detail_reanchors_selection_by_date_after_sort_change() {
    let mut app = make_app();
    app.current_tab = Tab::Daily;
    app.sort_field = SortField::Date;
    app.sort_direction = SortDirection::Descending;
    app.usage_mut_for_test().daily = vec![
        daily_usage("2026-05-10", 1.0, vec![("old-model", "anthropic", 1.0)]),
        daily_usage(
            "2026-05-17",
            7.0,
            vec![("target-a", "openai", 5.0), ("target-b", "anthropic", 2.0)],
        ),
        daily_usage("2026-05-18", 3.0, vec![("other-model", "google", 3.0)]),
    ];

    app.set_selected_index(1);
    let target_date = app.get_sorted_daily()[app.selected_index()].date;

    app.handle_key_event(key(KeyCode::Enter));
    assert!(app.is_daily_detail_active());
    assert_eq!(app.daily_detail_date(), Some(target_date));

    app.handle_key_event(key(KeyCode::Char('c')));
    assert_eq!(app.sort_field, SortField::Cost);

    app.handle_key_event(key(KeyCode::Esc));

    assert!(!app.is_daily_detail_active());
    let restored_index = app.selected_index();
    let restored_date = app.get_sorted_daily()[restored_index].date;
    assert_eq!(
        restored_date, target_date,
        "Closing detail after sort change should re-anchor on the original date"
    );
}

#[test]
fn test_update_data_exits_daily_detail_when_date_disappears() {
    let mut app = make_app();
    app.current_tab = Tab::Daily;
    app.sort_field = SortField::Date;
    app.sort_direction = SortDirection::Descending;
    app.usage_mut_for_test().daily = vec![
        daily_usage("2026-05-10", 1.0, vec![("old-model", "anthropic", 1.0)]),
        daily_usage(
            "2026-05-17",
            7.0,
            vec![("target-a", "openai", 5.0), ("target-b", "anthropic", 2.0)],
        ),
        daily_usage("2026-05-18", 3.0, vec![("other-model", "google", 3.0)]),
    ];

    app.set_selected_index(1);
    app.handle_key_event(key(KeyCode::Enter));
    assert!(app.is_daily_detail_active());

    let refreshed = UsageProjection {
        daily: vec![
            daily_usage("2026-05-10", 1.0, vec![("old-model", "anthropic", 1.0)]),
            daily_usage("2026-05-18", 3.0, vec![("other-model", "google", 3.0)]),
        ],
        ..Default::default()
    };
    app.update_data(refreshed);

    assert!(
        !app.is_daily_detail_active(),
        "update_data should drop detail mode when the selected date is gone"
    );
    assert_eq!(app.daily_detail_date(), None);
    assert!(app.daily_detail_rows().is_empty());
}

#[test]
fn test_update_data_keeps_daily_detail_when_date_still_present() {
    let mut app = make_app();
    app.current_tab = Tab::Daily;
    app.sort_field = SortField::Date;
    app.sort_direction = SortDirection::Descending;
    app.usage_mut_for_test().daily = vec![
        daily_usage("2026-05-10", 1.0, vec![("old-model", "anthropic", 1.0)]),
        daily_usage(
            "2026-05-17",
            7.0,
            vec![("target-a", "openai", 5.0), ("target-b", "anthropic", 2.0)],
        ),
    ];

    app.set_selected_index(1);
    let target_date = app.get_sorted_daily()[app.selected_index()].date;
    app.handle_key_event(key(KeyCode::Enter));
    assert!(app.is_daily_detail_active());

    let refreshed = UsageProjection {
        daily: vec![
            daily_usage("2026-05-10", 1.0, vec![("old-model", "anthropic", 1.0)]),
            daily_usage(
                "2026-05-17",
                9.0,
                vec![("target-a", "openai", 7.0), ("target-b", "anthropic", 2.0)],
            ),
        ],
        ..Default::default()
    };
    app.update_data(refreshed);

    assert!(app.is_daily_detail_active());
    assert_eq!(app.daily_detail_date(), Some(target_date));
}

#[test]
fn test_daily_detail_updates_rows_after_group_by_reload() {
    let mut app = make_app();
    app.current_tab = Tab::Daily;
    app.sort_field = SortField::Date;
    app.sort_direction = SortDirection::Descending;
    app.set_group_by_for_test(tokenx_engine::GroupBy::ClientModel);
    app.usage_mut_for_test().daily = vec![daily_usage_by_client(
        "2026-05-17",
        0.0,
        vec![
            ("claude", vec![("claude:gpt-5", "openai", 5.0)]),
            ("codex", vec![("codex:gpt-5", "openai", 2.0)]),
        ],
    )];

    app.handle_key_event(key(KeyCode::Enter));
    assert!(app.is_daily_detail_active());
    assert_eq!(app.daily_detail_rows().len(), 2);

    app.set_group_by_for_test(tokenx_engine::GroupBy::Model);
    app.update_data(UsageProjection {
        daily: vec![daily_usage_by_client(
            "2026-05-17",
            0.0,
            vec![
                ("claude", vec![("gpt-5", "openai", 5.0)]),
                ("codex", vec![("gpt-5", "openai", 2.0)]),
            ],
        )],
        ..Default::default()
    });

    let rows = app.daily_detail_rows();
    assert!(app.is_daily_detail_active());
    assert_eq!(
        app.daily_detail_date(),
        Some(NaiveDate::from_ymd_opt(2026, 5, 17).unwrap())
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].clients.as_ref(),
        &[ClientId::Claude, ClientId::Codex]
    );
    assert_eq!(rows[0].model.as_ref(), "gpt-5");
    assert_eq!(rows[0].tokens.total(), 730);
    assert_eq!(rows[0].messages, 2);
    assert!((rows[0].cost - 7.0).abs() < f64::EPSILON);
}

#[test]
fn test_enter_on_monthly_opens_selected_period_detail_rows() {
    let mut app = make_app();
    app.current_tab = Tab::Monthly;
    app.sort_field = SortField::Date;
    app.sort_direction = SortDirection::Descending;
    app.usage_mut_for_test().daily = vec![
        daily_usage("2026-05-17", 7.0, vec![("target-a", "openai", 5.0)]),
        daily_usage(
            "2026-05-18",
            5.0,
            vec![("target-a", "openai", 3.0), ("target-b", "anthropic", 2.0)],
        ),
        daily_usage("2026-06-01", 1.0, vec![("june-model", "google", 1.0)]),
    ];

    app.set_selected_index(1);
    let selected_period =
        app.get_sorted_periods(PeriodKind::Monthly)[app.selected_index()].start_date;
    app.handle_key_event(key(KeyCode::Enter));

    assert!(app.is_period_detail_active_for_kind(PeriodKind::Monthly));
    assert_eq!(app.sort_field, SortField::Tokens);
    assert_eq!(app.sort_direction, SortDirection::Descending);
    assert_eq!(app.current_list_len(), 2);
    assert_eq!(app.period_detail_rows()[0].model.as_ref(), "target-a");
    assert_eq!(
        app.selected_period_detail.unwrap().start_date,
        selected_period
    );
}

#[test]
fn test_period_detail_model_name_falls_back_to_model_key() {
    let mut app = make_app();
    app.current_tab = Tab::Monthly;
    app.usage_mut_for_test().daily = vec![daily_usage(
        "2026-05-17",
        7.0,
        vec![("fallback-model", "openai", 7.0)],
    )];
    app.usage_mut_for_test().daily[0]
        .client_breakdown
        .get_mut(&ClientId::Claude)
        .unwrap()
        .models
        .iter_mut()
        .find(|model| model.model_id.as_ref() == "fallback-model")
        .unwrap()
        .display_name = "".into();

    app.handle_key_event(key(KeyCode::Enter));

    assert_eq!(app.period_detail_rows()[0].model.as_ref(), "fallback-model");
}

#[test]
fn test_esc_from_weekly_detail_restores_week_selection() {
    let mut app = make_app();
    app.current_tab = Tab::Weekly;
    app.sort_field = SortField::Date;
    app.sort_direction = SortDirection::Descending;
    app.usage_mut_for_test().daily = vec![
        daily_usage("2026-06-01", 1.0, vec![("week-23-a", "openai", 1.0)]),
        daily_usage("2026-06-03", 2.0, vec![("week-23-b", "anthropic", 2.0)]),
        daily_usage("2026-06-10", 3.0, vec![("week-24", "google", 3.0)]),
        daily_usage("2026-06-17", 4.0, vec![("week-25", "google", 4.0)]),
    ];

    app.set_max_visible_items(2);
    app.set_selected_index(2);
    app.set_scroll_offset(1);
    let selected_period =
        app.get_sorted_periods(PeriodKind::Weekly)[app.selected_index()].start_date;

    app.handle_key_event(key(KeyCode::Enter));
    app.handle_key_event(key(KeyCode::Down));
    assert!(app.is_period_detail_active_for_kind(PeriodKind::Weekly));
    assert_eq!(app.selected_index(), 1);
    app.set_max_visible_items(5);

    app.handle_key_event(key(KeyCode::Esc));

    assert!(!app.is_period_detail_active());
    assert_eq!(app.current_tab, Tab::Weekly);
    assert_eq!(app.sort_field, SortField::Date);
    assert_eq!(app.sort_direction, SortDirection::Descending);
    assert_eq!(app.selected_index(), 2);
    assert_eq!(app.scroll_offset(), 1);
    assert_eq!(app.max_visible_items(), 2);
    assert_eq!(app.stored_list_interaction(Tab::Weekly).visible, 2);
    assert_eq!(
        app.get_sorted_periods(PeriodKind::Weekly)[app.selected_index()].start_date,
        selected_period
    );
}

#[test]
fn test_update_data_exits_period_detail_when_period_disappears() {
    let mut app = make_app();
    app.current_tab = Tab::Weekly;
    app.sort_field = SortField::Date;
    app.sort_direction = SortDirection::Descending;
    app.usage_mut_for_test().daily = vec![
        daily_usage("2026-06-01", 1.0, vec![("target", "openai", 1.0)]),
        daily_usage("2026-06-10", 3.0, vec![("other", "google", 3.0)]),
    ];

    app.set_selected_index(1);
    app.handle_key_event(key(KeyCode::Enter));
    assert!(app.is_period_detail_active_for_kind(PeriodKind::Weekly));

    let refreshed = UsageProjection {
        daily: vec![daily_usage(
            "2026-06-10",
            3.0,
            vec![("other", "google", 3.0)],
        )],
        ..Default::default()
    };
    app.update_data(refreshed);

    assert!(
        !app.is_period_detail_active(),
        "update_data should drop period detail mode when the selected period is gone"
    );
    assert!(app.period_detail_rows().is_empty());
}

#[test]
fn test_period_detail_uses_grouped_detail_rows() {
    let mut app = make_app();
    app.current_tab = Tab::Monthly;
    app.sort_field = SortField::Date;
    app.sort_direction = SortDirection::Descending;
    app.set_group_by_for_test(tokenx_engine::GroupBy::Model);
    app.usage_mut_for_test().daily = vec![
        daily_usage_by_client(
            "2026-05-17",
            0.0,
            vec![
                ("claude", vec![("gpt-5", "openai", 5.0)]),
                ("codex", vec![("gpt-5", "openai", 2.0)]),
            ],
        ),
        daily_usage("2026-06-01", 1.0, vec![("june-model", "google", 1.0)]),
    ];

    app.set_selected_index(1);
    app.handle_key_event(key(KeyCode::Enter));

    let rows = app.period_detail_rows();
    assert!(app.is_period_detail_active_for_kind(PeriodKind::Monthly));
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].clients.as_ref(),
        &[ClientId::Claude, ClientId::Codex]
    );
    assert_eq!(rows[0].model.as_ref(), "gpt-5");
    assert_eq!(rows[0].tokens.total(), 730);
    assert_eq!(rows[0].messages, 2);
    assert!((rows[0].cost - 7.0).abs() < f64::EPSILON);
}

// ── handle_key_event: sort ──────────────────────────────────────

#[test]
fn test_handle_key_sort_cost() {
    let mut app = make_app();
    app.handle_key_event(key(KeyCode::Char('c')));
    assert_eq!(app.sort_field, SortField::Cost);
    assert_eq!(app.sort_direction, SortDirection::Ascending);
}

#[test]
fn test_handle_key_sort_tokens() {
    let mut app = make_app();
    app.handle_key_event(key(KeyCode::Char('t')));
    assert_eq!(app.sort_field, SortField::Tokens);
    assert_eq!(app.sort_direction, SortDirection::Descending);
}

#[test]
fn test_handle_key_sort_date() {
    let mut app = make_app();
    app.handle_key_event(key(KeyCode::Char('d')));
    assert_eq!(app.sort_field, SortField::Date);
    assert_eq!(app.sort_direction, SortDirection::Descending);
}

#[test]
fn test_handle_key_sort_toggle_direction() {
    let mut app = make_app();
    app.handle_key_event(key(KeyCode::Char('t')));
    assert_eq!(app.sort_direction, SortDirection::Descending);

    app.handle_key_event(key(KeyCode::Char('t')));
    assert_eq!(app.sort_direction, SortDirection::Ascending);

    app.handle_key_event(key(KeyCode::Char('t')));
    assert_eq!(app.sort_direction, SortDirection::Descending);
}

#[test]
fn test_switch_tab_restores_hourly_date_default() {
    let mut app = make_app();
    assert_eq!(app.sort_field, SortField::Cost);

    app.switch_tab(Tab::Hourly);
    assert_eq!(app.sort_field, SortField::Date);
    assert_eq!(app.sort_direction, SortDirection::Descending);

    app.switch_tab(Tab::Models);
    assert_eq!(app.sort_field, SortField::Tokens);
    assert_eq!(app.sort_direction, SortDirection::Descending);
}

#[test]
fn test_initial_models_tab_uses_token_sort_default() {
    let config = TuiConfig {
        theme: Some(ThemeName::Blue),
        refresh: 0,
        no_refresh: false,
        client_universe: tokenx_engine::ClientUniverse::all(),
        initial_tab: Some(Tab::Models),
        effective_date: chrono::NaiveDate::from_ymd_opt(2026, 7, 26).unwrap(),
    };

    let app = TuiModel::new_for_test_with_settings(config, Settings::default()).unwrap();

    assert_eq!(app.current_tab, Tab::Models);
    assert_eq!(app.sort_field, SortField::Tokens);
    assert_eq!(app.sort_direction, SortDirection::Descending);
}

#[test]
fn test_models_default_sort_shows_highest_tokens_first() {
    let mut app = make_app();
    app.usage_mut_for_test().models = vec![
        UsageModelEntry {
            machine: None,
            cost_provenance: Default::default(),
            model_id: "expensive-low-token".into(),
            display_name: "expensive-low-token".into(),
            provider: "anthropic".into(),
            clients: vec![ClientId::Claude],
            tokens: UsageTokenBreakdown {
                input: 10,
                output: 0,
                cache_read: 0,
                cache_write: 0,
                reasoning: 0,
            },
            cost: 100.0,
            session_count: 1,
            workspace_key: None,
            workspace_label: None,
        },
        UsageModelEntry {
            machine: None,
            cost_provenance: Default::default(),
            model_id: "cheap-high-token".into(),
            display_name: "cheap-high-token".into(),
            provider: "anthropic".into(),
            clients: vec![ClientId::Claude],
            tokens: UsageTokenBreakdown {
                input: 1_000,
                output: 0,
                cache_read: 0,
                cache_write: 0,
                reasoning: 0,
            },
            cost: 1.0,
            session_count: 1,
            workspace_key: None,
            workspace_label: None,
        },
    ];

    app.switch_tab(Tab::Models);

    assert_eq!(app.sort_field, SortField::Tokens);
    assert_eq!(app.sort_direction, SortDirection::Descending);
    assert_eq!(
        app.get_sorted_models()[0].model_id.as_ref(),
        "cheap-high-token"
    );
}

#[test]
fn test_initial_hourly_tab_uses_hourly_sort_default() {
    let config = TuiConfig {
        theme: Some(ThemeName::Blue),
        refresh: 0,
        no_refresh: false,
        client_universe: tokenx_engine::ClientUniverse::all(),
        initial_tab: Some(Tab::Hourly),
        effective_date: chrono::NaiveDate::from_ymd_opt(2026, 7, 26).unwrap(),
    };

    let app = TuiModel::new_for_test(config).unwrap();

    assert_eq!(app.current_tab, Tab::Hourly);
    assert_eq!(app.sort_field, SortField::Date);
    assert_eq!(app.sort_direction, SortDirection::Descending);
}

#[test]
fn test_switch_tab_uses_daily_date_default() {
    let mut app = make_app();

    app.switch_tab(Tab::Daily);

    assert_eq!(app.sort_field, SortField::Date);
    assert_eq!(app.sort_direction, SortDirection::Descending);
}

#[test]
fn test_daily_default_sort_shows_latest_date_first() {
    let mut app = make_app();
    app.usage_mut_for_test().daily = vec![
        daily_usage(
            "2026-05-24",
            99.0,
            vec![("older-expensive", "anthropic", 99.0)],
        ),
        daily_usage("2026-05-26", 1.0, vec![("newer-cheap", "anthropic", 1.0)]),
        daily_usage("2026-05-25", 50.0, vec![("middle", "anthropic", 50.0)]),
    ];

    app.switch_tab(Tab::Daily);

    let dates = app
        .get_sorted_daily()
        .iter()
        .map(|entry| entry.date)
        .collect::<Vec<_>>();
    assert_eq!(
        dates,
        vec![
            NaiveDate::from_ymd_opt(2026, 5, 26).unwrap(),
            NaiveDate::from_ymd_opt(2026, 5, 25).unwrap(),
            NaiveDate::from_ymd_opt(2026, 5, 24).unwrap(),
        ]
    );
}

#[test]
fn test_switch_tab_preserves_user_sort() {
    let mut app = make_app();
    app.switch_tab(Tab::Models);

    app.set_sort(SortField::Cost);
    assert_eq!(app.sort_field, SortField::Cost);
    assert_eq!(app.sort_direction, SortDirection::Descending);

    app.switch_tab(Tab::Daily);
    assert_eq!(app.sort_field, SortField::Date);
    assert_eq!(app.sort_direction, SortDirection::Descending);

    app.switch_tab(Tab::Models);
    assert_eq!(app.sort_field, SortField::Cost);
    assert_eq!(app.sort_direction, SortDirection::Descending);
}

#[test]
fn test_switch_tab_preserves_daily_sort_after_hourly_roundtrip() {
    let mut app = make_app();

    app.switch_tab(Tab::Daily);
    app.set_sort(SortField::Tokens);
    assert_eq!(app.sort_field, SortField::Tokens);
    assert_eq!(app.sort_direction, SortDirection::Descending);

    app.switch_tab(Tab::Hourly);
    assert_eq!(app.sort_field, SortField::Date);
    assert_eq!(app.sort_direction, SortDirection::Descending);

    app.switch_tab(Tab::Daily);
    assert_eq!(app.sort_field, SortField::Tokens);
    assert_eq!(app.sort_direction, SortDirection::Descending);
}

// ── handle_key_event: navigation ────────────────────────────────

#[test]
fn test_handle_key_navigation_up_down() {
    let mut app = make_app_with_models(5);
    assert_eq!(app.selected_index(), 0);

    app.handle_key_event(key(KeyCode::Down));
    assert_eq!(app.selected_index(), 1);

    app.handle_key_event(key(KeyCode::Down));
    assert_eq!(app.selected_index(), 2);

    app.handle_key_event(key(KeyCode::Up));
    assert_eq!(app.selected_index(), 1);

    app.handle_key_event(key(KeyCode::Up));
    assert_eq!(app.selected_index(), 0);

    // At top boundary - wraps to last item (index 4, 5 models)
    app.handle_key_event(key(KeyCode::Up));
    assert_eq!(app.selected_index(), 4);
}

#[test]
fn test_handle_key_navigation_boundary() {
    let mut app = make_app_with_models(3);
    app.handle_key_event(key(KeyCode::Down));
    app.handle_key_event(key(KeyCode::Down));
    assert_eq!(app.selected_index(), 2);

    // At bottom boundary - wraps to first item (index 0)
    app.handle_key_event(key(KeyCode::Down));
    assert_eq!(app.selected_index(), 0);
}

#[test]
fn hourly_profile_key_scrolls_text_viewport_without_table_selection() {
    let mut app = make_app();
    let mut state = crate::tui::page_state::PageStates::default();
    app.current_tab = Tab::Hourly;
    state.set_hourly_mode_for_test(HourlyViewMode::Profile);
    app.set_selected_index(2);
    app.set_scroll_offset(1);
    state.set_hourly_profile_text_viewport(4, 10);

    state.handle_intent(&mut app, Intent::Move(MoveCommand::PageDown));

    assert_eq!(state.hourly_profile_scroll(), 2);
    assert_eq!(app.selected_index(), 2);
    assert_eq!(app.scroll_offset(), 1);
}

#[test]
fn subscription_move_intent_scrolls_text_viewport_without_table_selection() {
    let mut app = make_app_with_subscription();
    let mut state = crate::tui::page_state::PageStates::default();
    app.current_tab = Tab::Subscription;
    app.set_selected_index(4);
    app.set_scroll_offset(3);
    state.set_subscription_text_viewport(4, 10);

    state.handle_intent(&mut app, Intent::Move(MoveCommand::Down));

    assert_eq!(state.subscription_scroll(), 1);
    assert_eq!(app.selected_index(), 4);
    assert_eq!(app.scroll_offset(), 3);
}

// ── wrap-around navigation ──────────────────────────────────────

#[test]
fn test_move_selection_up_empty_list_noop() {
    let mut app = make_app();
    app.usage_mut_for_test().models.clear();
    app.set_selected_index(0);
    app.move_selection_up();
    assert_eq!(app.selected_index(), 0);
}

#[test]
fn test_move_selection_down_empty_list_noop() {
    let mut app = make_app();
    app.usage_mut_for_test().models.clear();
    app.set_selected_index(0);
    app.move_selection_down();
    assert_eq!(app.selected_index(), 0);
}

#[test]
fn test_move_selection_up_wrap_scroll_offset() {
    let mut app = make_app_with_models(10);
    app.set_max_visible_items(3);
    app.set_selected_index(0);
    app.move_selection_up();
    // Should wrap to index 9 and scroll so last item is visible
    assert_eq!(app.selected_index(), 9);
    assert_eq!(app.scroll_offset(), 7); // 10 - 3 = 7
}

#[test]
fn test_move_selection_down_wrap_resets_scroll() {
    let mut app = make_app_with_models(10);
    app.set_max_visible_items(3);
    app.set_selected_index(9);
    app.set_scroll_offset(7);
    app.move_selection_down();
    assert_eq!(app.selected_index(), 0);
    assert_eq!(app.scroll_offset(), 0);
}

#[test]
fn test_overview_scroll_keeps_rendered_capacity_after_resize() {
    let mut app = make_app_with_models(33);
    app.current_tab = Tab::Overview;
    app.set_max_visible_items(9);

    for _ in 0..32 {
        app.move_selection_down();
        app.handle_resize(120, 40);
        app.set_max_visible_items(9);
    }

    assert_eq!(app.selected_index(), 32);
    assert_eq!(app.scroll_offset(), 24);
}

// ── handle_key_event: theme ─────────────────────────────────────

#[test]
fn theme_transition_queues_persistence_without_writing_during_transition() {
    let mut app = make_app();
    let settings_path = app.product_paths.settings_file();

    app.apply_intent(Intent::Theme);

    assert!(!settings_path.exists());
    let effects = app.take_effects();
    assert_eq!(effects.len(), 1);
    assert!(matches!(&effects[0], TuiEffect::PersistSettings { .. }));
}

#[test]
fn language_transition_updates_the_snapshot_and_queues_persistence() {
    let mut app = make_app();
    let settings_path = app.product_paths.settings_file();
    app.set_subscription_status_with_tone("stale language", StatusTone::Info);

    app.prepare_language_change(Language::ZhCn);

    assert_eq!(app.language(), Language::ZhCn);
    assert_eq!(app.settings.language, Some(Language::ZhCn));
    assert!(app.subscription_status_message.is_none());
    assert!(!settings_path.exists());

    let effects = app.take_effects();
    assert_eq!(effects.len(), 1);
    match &effects[0] {
        TuiEffect::PersistSettings {
            settings,
            success_message,
            ..
        } => {
            assert_eq!(settings.language, Some(Language::ZhCn));
            assert_eq!(success_message, "语言：中文");
        }
        effect => panic!("unexpected effect: {effect:?}"),
    }
}

#[test]
fn test_handle_key_theme_cycle() {
    let mut app = make_app();
    let initial_theme = app.theme.name;
    let settings_path = app.product_paths.settings_file();

    app.handle_key_event(key(KeyCode::Char('p')));
    assert_ne!(app.theme.name, initial_theme);
    assert!(
        settings_path.exists(),
        "theme save must use the isolated test settings file"
    );

    for _ in 1..ThemeName::all().len() {
        app.handle_key_event(key(KeyCode::Char('p')));
    }
    assert_eq!(app.theme.name, initial_theme);
}

// ── handle_key_event: export ────────────────────────────────────

#[test]
fn test_handle_key_export() {
    let mut app = make_app();
    app.handle_key_event(key(KeyCode::Char('e')));
    assert!(app.status_message.is_some());
    let msg = app.status_message.as_ref().unwrap();
    assert!(
        msg.contains("Exported to") || msg.contains("Export failed"),
        "unexpected status: {}",
        msg
    );
}

// ── handle_key_event: refresh ───────────────────────────────────

#[test]
fn test_handle_key_refresh() {
    let mut app = make_app();
    app.handle_key_event(key(KeyCode::Char('r')));
    assert_eq!(app.take_refresh_requests(), vec![RefreshRequest::Manual]);
}

#[test]
fn background_loading_tracks_one_elapsed_interval_per_run() {
    let mut app = make_app();
    assert!(app.background_load_elapsed().is_none());

    app.set_refresh_loading_for_test(true);
    let first_elapsed = app.background_load_elapsed().unwrap();
    assert!(app.background_load_elapsed().is_some());

    app.set_refresh_loading_for_test(true);
    assert!(app.background_load_elapsed().unwrap() >= first_elapsed);

    app.set_refresh_loading_for_test(false);
    assert!(app.background_load_elapsed().is_none());
}

#[test]
fn test_handle_key_refresh_while_loading_does_not_queue_reload() {
    let mut app = make_app();
    app.set_refresh_loading_for_test(true);

    app.handle_key_event(key(KeyCode::Char('r')));

    assert_eq!(app.take_refresh_requests(), vec![RefreshRequest::Manual]);
    assert_eq!(app.status_message.as_deref(), Some("Refresh queued"));
}

#[test]
fn model_group_enter_splits_one_model_by_client_and_provider() {
    let mut app = make_app_with_model_projection(tokenx_engine::GroupBy::Model);
    app.set_selected_index(
        app.get_sorted_models()
            .iter()
            .position(|model| model.model_id.as_ref() == "shared-model")
            .unwrap(),
    );

    app.handle_key_event(key(KeyCode::Enter));

    assert_eq!(
        app.selected_model_detail,
        Some(ModelDetailSelection {
            model: "shared-model".to_string(),
            client: None,
        })
    );
    let mut rows = app
        .get_sorted_models()
        .into_iter()
        .map(|model| (model.clients.clone(), model.provider.to_string()))
        .collect::<Vec<_>>();
    rows.sort();
    assert_eq!(
        rows,
        vec![
            (vec![ClientId::Claude], "anthropic".to_string()),
            (vec![ClientId::Claude], "openrouter".to_string()),
            (vec![ClientId::Codex], "openai".to_string()),
        ]
    );
}

#[test]
fn client_model_group_enter_keeps_outer_client_and_splits_providers() {
    let mut app = make_app_with_model_projection(tokenx_engine::GroupBy::ClientModel);
    app.set_selected_index(
        app.get_sorted_models()
            .iter()
            .position(|model| {
                model.model_id.as_ref() == "shared-model"
                    && model.clients.as_slice() == [ClientId::Claude]
            })
            .unwrap(),
    );

    app.handle_key_event(key(KeyCode::Enter));

    assert_eq!(
        app.selected_model_detail,
        Some(ModelDetailSelection {
            model: "shared-model".to_string(),
            client: Some(ClientId::Claude),
        })
    );
    let mut providers = app
        .get_sorted_models()
        .into_iter()
        .map(|model| {
            assert_eq!(model.clients, [ClientId::Claude]);
            model.provider.to_string()
        })
        .collect::<Vec<_>>();
    providers.sort();
    assert_eq!(providers, vec!["anthropic", "openrouter"]);
}

#[test]
fn model_detail_escape_restores_outer_state_without_reprojection() {
    let mut app = make_app_with_model_projection(tokenx_engine::GroupBy::Model);
    app.sort_field = SortField::Cost;
    app.sort_direction = SortDirection::Ascending;
    app.set_selected_index(
        app.get_sorted_models()
            .iter()
            .position(|model| model.model_id.as_ref() == "shared-model")
            .unwrap(),
    );
    let outer_selection = app.selected_index();

    app.handle_key_event(key(KeyCode::Enter));
    assert!(app.is_model_detail_active());
    assert!(app.model_detail_models.is_some());
    app.set_sort(SortField::Cost);

    app.handle_key_event(key(KeyCode::Esc));

    assert!(!app.is_model_detail_active());
    assert!(app.model_detail_models.is_some());
    assert_eq!(app.selected_index(), outer_selection);
    assert_eq!(app.sort_field, SortField::Cost);
    assert_eq!(app.sort_direction, SortDirection::Ascending);

    app.handle_key_event(key(KeyCode::Enter));
    assert!(
        app.is_model_detail_active(),
        "re-enter should reuse the cached provider projection"
    );
}

#[test]
fn model_detail_is_disabled_for_already_expanded_groupings() {
    for group_by in [
        tokenx_engine::GroupBy::ClientProviderModel,
        tokenx_engine::GroupBy::WorkspaceModel,
    ] {
        let mut app = make_app_with_model_projection(group_by);
        app.handle_key_event(key(KeyCode::Enter));

        assert!(!app.is_model_detail_active());
        assert!(app.model_detail_models.is_none());
    }
}

#[test]
fn refresh_invalidates_model_detail_but_client_projection_refreshes_it() {
    let mut app = make_app_with_model_projection(tokenx_engine::GroupBy::Model);
    app.set_selected_index(
        app.get_sorted_models()
            .iter()
            .position(|model| model.model_id.as_ref() == "shared-model")
            .unwrap(),
    );
    app.handle_key_event(key(KeyCode::Enter));
    assert!(app.model_detail_models.is_some());

    let data = app.usage().clone();
    app.update_data(data);
    assert!(!app.is_model_detail_active());
    assert!(app.model_detail_models.is_none());

    app.set_selected_index(
        app.get_sorted_models()
            .iter()
            .position(|model| model.model_id.as_ref() == "shared-model")
            .unwrap(),
    );
    app.handle_key_event(key(KeyCode::Enter));
    assert!(app.model_detail_models.is_some());
    app.apply_ui_command(UiCommand::ProjectClients(HashSet::from([ClientId::Claude])));

    assert!(app.is_model_detail_active());
    assert_eq!(
        app.selected_model_detail,
        Some(ModelDetailSelection {
            model: "shared-model".to_string(),
            client: None,
        })
    );
    let rows = app.get_sorted_models();
    assert_eq!(rows.len(), 2);
    assert!(rows
        .iter()
        .all(|model| model.clients.as_slice() == [ClientId::Claude]));
    assert!(app.model_detail_models.is_some());
    assert_eq!(
        app.selected_clients().collect::<HashSet<_>>(),
        HashSet::from([ClientId::Claude])
    );
    assert_eq!(
        app.status_message.as_deref(),
        Some("Clients filtered locally; model details updated")
    );

    app.handle_key_event(key(KeyCode::Esc));

    assert!(!app.is_model_detail_active());
    assert_eq!(
        app.get_sorted_models()[app.selected_index()]
            .model_id
            .as_ref(),
        "shared-model"
    );
}

#[test]
fn client_projection_exits_model_detail_when_locked_selection_disappears() {
    let mut app = make_app_with_model_projection(tokenx_engine::GroupBy::ClientModel);
    app.set_selected_index(
        app.get_sorted_models()
            .iter()
            .position(|model| {
                model.model_id.as_ref() == "shared-model"
                    && model.clients.as_slice() == [ClientId::Codex]
            })
            .unwrap(),
    );
    app.handle_key_event(key(KeyCode::Enter));
    assert!(app.is_model_detail_active());
    assert!(app.model_detail_models.is_some());
    app.apply_ui_command(UiCommand::ProjectClients(HashSet::from([ClientId::Claude])));

    assert!(!app.is_model_detail_active());
    assert!(app.model_detail_models.is_none());
    assert_eq!(
        app.selected_clients().collect::<HashSet<_>>(),
        HashSet::from([ClientId::Claude])
    );
    assert_eq!(
        app.status_message.as_deref(),
        Some("Selected model is not available for the current Client filter")
    );
}

#[test]
#[serial]
fn group_by_change_reprojects_the_installed_generation() {
    let (_home, generation) = load_test_generation();
    let mut app = make_app();
    let universe = generation.universe().clone();
    let clients = universe.as_hash_set();
    app.set_selected_clients_for_test(clients);
    app.current_tab = Tab::Models;
    app.set_group_by_for_test(tokenx_engine::GroupBy::ClientModel);
    app.install_generation(generation).unwrap();
    let health_complete = app.generation_health().unwrap().complete();
    let failed_inputs = app.generation_health().unwrap().failed_inputs();
    app.set_refresh_loading_for_test(true);
    app.fail_refresh_for_test("retained error".to_string());
    app.set_generation_cache_warning(Some("retained cache warning".to_string()));
    let refresh_status = app.refresh_status();

    app.handle_key_event(key(KeyCode::Char('g')));
    app.handle_key_event(key(KeyCode::Down));
    app.handle_key_event(key(KeyCode::Down));
    app.handle_key_event(key(KeyCode::Enter));

    assert_eq!(app.group_by(), tokenx_engine::GroupBy::WorkspaceModel);
    assert_eq!(app.usage().models.len(), 2);
    assert!(app
        .usage()
        .models
        .iter()
        .all(|model| model.workspace_key.is_some()));
    assert!(app.take_refresh_requests().is_empty());
    assert!(!app.is_background_loading());
    assert_eq!(app.refresh_status(), refresh_status);
    assert_eq!(app.generation_health().unwrap().complete(), health_complete);
    assert_eq!(
        app.generation_health().unwrap().failed_inputs(),
        failed_inputs
    );
    assert!(matches!(
        app.local_usage_status(),
        LocalUsageStatus::Degraded {
            diagnostic: "retained error"
        }
    ));
    assert_eq!(
        app.generation_cache_warning(),
        Some("retained cache warning")
    );
    assert_eq!(
        app.status_message.as_deref(),
        Some("Regrouped by workspace,model")
    );

    let export = crate::report::build_usage_report_json(
        app.usage(),
        app.generation_health().unwrap(),
        &app.export_group_by(),
    )
    .unwrap();
    let exported: serde_json::Value = serde_json::from_str(&export).unwrap();
    assert_eq!(exported["groupBy"], "workspace,model");
    assert!(exported["models"]
        .as_array()
        .unwrap()
        .iter()
        .all(|model| model.get("workspaceKey").is_some()));
}

#[test]
fn overview_summary_tracks_local_projection_without_digest_invalidation() {
    let mut app = make_app();
    app.update_data(UsageProjection {
        daily: vec![daily_usage(
            "2026-07-20",
            1.0,
            vec![("gpt-5.5", "openai", 1.0)],
        )],
        ..UsageProjection::default()
    });

    assert_eq!(app.overview_summary().active_days, 1);
    assert_eq!(app.overview_summary().model_count, 1);
    assert_eq!(
        app.overview_summary()
            .favorite_model
            .as_ref()
            .map(|favorite| favorite.id.as_str()),
        Some("gpt-5.5")
    );

    app.update_projected_data(UsageProjection {
        daily: vec![
            daily_usage("2026-07-20", 2.0, vec![("qwen3-coder-plus", "qwen", 2.0)]),
            daily_usage("2026-07-21", 3.0, vec![("kimi-k2", "kimi", 3.0)]),
        ],
        ..UsageProjection::default()
    });

    assert_eq!(app.overview_summary().active_days, 2);
    assert_eq!(app.overview_summary().model_count, 2);
    assert_eq!(
        app.overview_summary()
            .favorite_model
            .as_ref()
            .map(|favorite| favorite.id.as_str()),
        Some("kimi-k2")
    );
}

#[test]
fn test_group_by_is_unavailable_without_an_installed_generation() {
    let mut app = make_app();
    app.current_tab = Tab::Models;
    app.set_group_by_for_test(tokenx_engine::GroupBy::ClientModel);

    app.handle_key_event(key(KeyCode::Char('g')));

    assert_eq!(app.group_by(), tokenx_engine::GroupBy::ClientModel);
    assert!(!app.dialog_stack.is_active());
    assert!(app.take_refresh_requests().is_empty());
    assert_eq!(
        app.status_message.as_deref(),
        Some("Group By is unavailable until local data finishes loading")
    );
    assert_eq!(app.status_message_tone(), StatusTone::Warning);
}

#[test]
fn test_group_by_is_unavailable_during_cold_load() {
    let mut app = make_app();
    app.current_tab = Tab::Models;
    app.set_refresh_loading_for_test(true);
    app.set_group_by_for_test(tokenx_engine::GroupBy::ClientModel);

    app.handle_key_event(key(KeyCode::Char('g')));

    assert_eq!(app.group_by(), tokenx_engine::GroupBy::ClientModel);
    assert!(!app.dialog_stack.is_active());
    assert!(app.take_refresh_requests().is_empty());
    assert_eq!(
        app.status_message.as_deref(),
        Some("Group By is unavailable until local data finishes loading")
    );
    assert_eq!(app.status_message_tone(), StatusTone::Warning);
}

#[test]
fn export_group_by_matches_the_installed_projection() {
    let app = make_app_with_model_projection(tokenx_engine::GroupBy::WorkspaceModel);
    assert_eq!(
        app.export_group_by(),
        tokenx_engine::GroupBy::WorkspaceModel
    );
}

#[test]
fn test_client_picker_reprojects_without_requesting_reload() {
    let mut app = make_app();
    app.install_generation_fixture(
        tokenx_engine::FrozenUsageIndex::new(),
        Vec::new(),
        Default::default(),
    );
    let refresh_status = app.refresh_status();

    app.handle_key_event(key(KeyCode::Char('s')));
    app.handle_key_event(key(KeyCode::Char(' ')));
    app.handle_key_event(key(KeyCode::Enter));

    assert!(app.take_refresh_requests().is_empty());
    assert_eq!(app.refresh_status(), refresh_status);
}

#[test]
fn invalid_projection_command_leaves_the_installed_state_unchanged() {
    let mut app = make_app();
    app.set_selected_clients_for_test(HashSet::from([ClientId::Claude]));
    app.install_generation_fixture(
        tokenx_engine::FrozenUsageIndex::new(),
        Vec::new(),
        Default::default(),
    );
    let original_clients = app.selected_clients().collect::<HashSet<_>>();
    let original_group_by = app.group_by();

    app.apply_ui_command(UiCommand::ProjectClients(HashSet::from([ClientId::Codex])));

    assert_eq!(
        app.selected_clients().collect::<HashSet<_>>(),
        original_clients
    );
    assert_eq!(app.group_by(), original_group_by);
    assert_eq!(
        app.status_message.as_deref(),
        Some("Client selection is outside the loaded client universe")
    );
    assert!(app.take_refresh_requests().is_empty());
}

#[test]
fn test_g_opens_group_picker_only_on_group_keyed_tabs() {
    for tab in [Tab::Models, Tab::Daily, Tab::Monthly, Tab::Weekly] {
        let mut app = make_app();
        app.current_tab = tab;
        app.install_generation_fixture(
            tokenx_engine::FrozenUsageIndex::new(),
            Vec::new(),
            Default::default(),
        );

        app.handle_key_event(key(KeyCode::Char('g')));

        assert!(
            app.dialog_stack.is_active(),
            "g should open the Group By picker on {tab:?}"
        );
    }

    for tab in [
        Tab::Overview,
        Tab::Subscription,
        Tab::Hourly,
        Tab::Stats,
        Tab::Agents,
        Tab::Sessions,
    ] {
        let mut app = make_app();
        app.current_tab = tab;

        app.handle_key_event(key(KeyCode::Char('g')));

        assert!(
            !app.dialog_stack.is_active(),
            "g should be a no-op on {tab:?}"
        );
    }
}

#[test]
fn test_client_picker_waits_until_close_before_local_projection() {
    let mut app = make_app();
    app.install_generation_fixture(
        tokenx_engine::FrozenUsageIndex::new(),
        Vec::new(),
        Default::default(),
    );
    let original_clients = app.selected_clients().collect::<HashSet<_>>();

    app.handle_key_event(key(KeyCode::Char('s')));
    assert!(app.dialog_stack.is_active());

    app.handle_key_event(key(KeyCode::Char(' ')));
    assert!(app.dialog_stack.is_active());
    assert!(app.take_refresh_requests().is_empty());
    assert_eq!(
        app.selected_clients().collect::<HashSet<_>>(),
        original_clients
    );

    app.on_tick();
    assert!(app.take_refresh_requests().is_empty());
    assert_eq!(
        app.selected_clients().collect::<HashSet<_>>(),
        original_clients
    );

    app.handle_key_event(key(KeyCode::Enter));

    assert!(!app.dialog_stack.is_active());
    assert!(app.take_refresh_requests().is_empty());
    assert_ne!(
        app.selected_clients().collect::<HashSet<_>>(),
        original_clients
    );
}

#[test]
fn test_client_picker_is_unavailable_during_cold_load() {
    let mut app = make_app();
    app.set_refresh_loading_for_test(true);

    app.handle_key_event(key(KeyCode::Char('s')));

    assert!(!app.dialog_stack.is_active());
    assert!(app.take_refresh_requests().is_empty());
    assert_eq!(
        app.status_message.as_deref(),
        Some("Clients are unavailable until local data finishes loading")
    );
    assert_eq!(app.status_message_tone(), StatusTone::Warning);
}

// ── handle_key_event: misc keys ─────────────────────────────────

#[test]
fn test_entering_stats_selects_today_instead_of_latest_subscription_day() {
    let mut app = make_app();
    let today = app.effective_date();
    let activity_date = today - chrono::Duration::days(1);
    app.update_data(usage_data_with_graph_for_today(today, activity_date));
    let today_cell = app.graph_cell_for_date(today).unwrap();
    let activity_cell = app.graph_cell_for_date(activity_date).unwrap();
    assert_ne!(today_cell, activity_cell);

    app.selected_graph_cell = Some(activity_cell);
    app.switch_tab(Tab::Stats);

    assert_eq!(app.selected_graph_cell, Some(today_cell));
    assert_eq!(app.graph_date_for_cell(today_cell), Some(today));
}

#[test]
fn test_initial_stats_tab_selects_today_from_cached_graph() {
    let today = chrono::NaiveDate::from_ymd_opt(2026, 7, 26).unwrap();
    let activity_date = today - chrono::Duration::days(2);
    let config = TuiConfig {
        theme: Some(ThemeName::Blue),
        refresh: 0,
        no_refresh: false,
        client_universe: tokenx_engine::ClientUniverse::all(),
        initial_tab: Some(Tab::Stats),
        effective_date: chrono::NaiveDate::from_ymd_opt(2026, 7, 26).unwrap(),
    };

    let mut app = TuiModel::new_for_test_with_settings(config, test_settings()).unwrap();
    app.update_data(usage_data_with_graph_for_today(today, activity_date));

    assert_eq!(
        app.selected_graph_cell
            .and_then(|cell| app.graph_date_for_cell(cell)),
        Some(today)
    );
}

#[test]
fn test_stats_selects_today_when_graph_arrives_after_entry() {
    let mut app = make_app();
    let today = app.effective_date();

    app.switch_tab(Tab::Stats);
    assert_eq!(app.selected_graph_cell, None);

    app.update_data(usage_data_with_graph_for_today(
        today,
        today - chrono::Duration::days(1),
    ));

    assert_eq!(
        app.selected_graph_cell
            .and_then(|cell| app.graph_date_for_cell(cell)),
        Some(today)
    );
}

#[test]
fn test_stats_tab_reclick_keeps_manual_day_selection() {
    let mut app = make_app();
    let today = app.effective_date();
    let activity_date = today - chrono::Duration::days(3);
    app.update_data(usage_data_with_graph_for_today(today, activity_date));
    app.switch_tab(Tab::Stats);
    let activity_cell = app.graph_cell_for_date(activity_date).unwrap();
    app.selected_graph_cell = Some(activity_cell);

    // Clicking the already-active Stats tab header must not reset the
    // manually chosen day back to today.
    app.switch_tab(Tab::Stats);

    assert_eq!(app.selected_graph_cell, Some(activity_cell));
}

#[test]
fn test_stats_escape_prevents_refresh_from_reselecting_today() {
    let mut app = make_app();
    let today = app.effective_date();
    let activity_date = today - chrono::Duration::days(1);
    app.update_data(usage_data_with_graph_for_today(today, activity_date));
    app.switch_tab(Tab::Stats);
    assert!(app.selected_graph_cell.is_some());

    app.handle_key_event(key(KeyCode::Esc));
    assert_eq!(app.selected_graph_cell, None);

    app.update_data(usage_data_with_graph_for_today(today, activity_date));
    assert_eq!(app.selected_graph_cell, None);
}

#[test]
fn test_stats_selection_is_remapped_by_date_after_graph_rebuild() {
    let mut app = make_app();
    let today = app.effective_date();
    let selected_date = today - chrono::Duration::days(10);
    app.update_data(usage_data_with_graph_for_today(today, selected_date));
    app.switch_tab(Tab::Stats);
    let old_cell = app.graph_cell_for_date(selected_date).unwrap();
    app.selected_graph_cell = Some(old_cell);

    app.update_data(usage_data_with_graph_for_today(
        today + chrono::Duration::days(7),
        selected_date,
    ));

    let new_cell = app.selected_graph_cell.unwrap();
    assert_ne!(new_cell, old_cell);
    assert_eq!(app.graph_date_for_cell(new_cell), Some(selected_date));
}

#[test]
fn test_handle_key_esc_clears_graph_selection() {
    let mut app = make_app();
    app.selected_graph_cell = Some((1, 2));

    app.handle_key_event(key(KeyCode::Esc));
    assert_eq!(app.selected_graph_cell, None);
}

#[test]
fn test_handle_key_enter_on_stats() {
    let mut app = make_app();
    app.current_tab = Tab::Stats;
    app.selected_graph_cell = Some((1, 2));

    app.handle_key_event(key(KeyCode::Enter));
    assert!(app.status_message.is_some());
}

#[test]
fn test_handle_key_unrecognized_continues() {
    let mut app = make_app();
    let outcome = app.handle_key_event(key(KeyCode::F(12)));
    assert_eq!(outcome, KeyEventOutcome::Continue);
}

#[test]
fn test_handle_key_auto_refresh_toggle() {
    let mut app = make_app();
    app.handle_key_event(key_with_mod(KeyCode::Char('R'), KeyModifiers::SHIFT));
    assert_eq!(
        app.take_refresh_controls(),
        vec![RefreshControl::ToggleAutomatic]
    );
}

#[test]
fn test_subscription_fetch_completion_updates_subscription_status_and_timestamp() {
    let mut app = make_app();
    let (_tx, rx) = std::sync::mpsc::channel();

    app.start_subscription_fetch_for_test(rx);

    assert_eq!(
        app.subscription_status_message.as_deref(),
        Some("Fetching subscription...")
    );
    assert!(app.status_message.is_none());
    assert!(app.has_subscription_fetch_history());
    assert!(app.is_fetching_subscription());
    assert!(app.subscription_fetch_elapsed().is_some());
}

#[test]
fn test_subscription_fetch_completion_sets_subscription_check_clock() {
    let mut app = make_app();
    let (tx, rx) = std::sync::mpsc::channel();
    app.start_subscription_fetch_for_test(rx);
    tx.send(SubscriptionBatch::default()).unwrap();

    app.on_tick();

    assert_eq!(
        app.subscription_status_message.as_deref(),
        Some("No subscription data available")
    );
    assert!(app.status_message.is_none());
    assert!(app.last_subscription_check().is_some());
    assert!(!app.is_fetching_subscription());
    assert!(app.subscription_fetch_elapsed().is_none());
}

fn subscription_output(provider: ProviderId) -> SubscriptionOutput {
    SubscriptionOutput {
        provider,
        stale: false,
        account: None,
        plan: None,
        email: None,
        metrics: vec![crate::subscription::UsageMetric {
            label: "Weekly".to_string(),
            used_percent: 20.0,
            remaining_percent: 80.0,
            remaining_label: None,
            resets_at: None,
        }],
    }
}

#[test]
fn empty_subscription_batch_retains_installed_snapshot() {
    let mut app = make_app();
    let installed = subscription_output(ProviderId::Codex);
    app.replace_subscription_outputs_for_test(vec![installed.clone()]);

    app.install_subscription_batch(SubscriptionBatch {
        outputs: Vec::new(),
        errors: vec![crate::subscription::SubscriptionError {
            provider_id: Some(ProviderId::Claude),
            provider: "Claude".to_string(),
            issue: crate::subscription::SubscriptionIssue::unexpected("credential expired"),
        }],
    });

    let retained = &app.subscription_outputs()[0];
    assert_eq!(retained.provider, installed.provider);
    assert!(retained.stale);
    assert_eq!(app.subscription_errors().len(), 1);
    assert_eq!(
        app.subscription_status_message.as_deref(),
        Some("Subscription data loaded with provider errors")
    );
}

#[test]
fn nonempty_subscription_batch_replaces_snapshot_and_keeps_all_faults_visible() {
    let mut app = make_app();
    app.replace_subscription_outputs_for_test(vec![subscription_output(ProviderId::Claude)]);
    let replacement = subscription_output(ProviderId::Codex);

    app.install_subscription_batch(SubscriptionBatch {
        outputs: vec![replacement.clone()],
        errors: vec![crate::subscription::SubscriptionError {
            provider_id: Some(ProviderId::Claude),
            provider: "Claude".to_string(),
            issue: crate::subscription::SubscriptionIssue::unexpected("credential rejected"),
        }],
    });
    app.apply_effect_outcome(EffectOutcome::SubscriptionCachePersisted {
        result: Err(anyhow::anyhow!("cache directory is read-only")),
    });

    assert_eq!(app.subscription_outputs().len(), 2);
    assert_eq!(app.subscription_outputs()[0].provider, ProviderId::Claude);
    assert!(app.subscription_outputs()[0].stale);
    assert_eq!(app.subscription_outputs()[1], replacement);
    assert_eq!(app.subscription_errors().len(), 2);
    assert_eq!(app.subscription_errors()[1].provider, "Subscription cache");
    assert_eq!(
        app.subscription_status_message.as_deref(),
        Some("Subscription data loaded with provider errors")
    );
}

#[test]
fn test_fetch_subscription_while_fetching_reports_in_progress() {
    let mut app = make_app();
    app.current_tab = Tab::Subscription;
    let (_tx, rx) = std::sync::mpsc::channel();
    app.start_subscription_fetch_for_test(rx);
    let started_elapsed = app.subscription_fetch_elapsed().unwrap();

    app.fetch_subscription();

    assert_eq!(
        app.subscription_status_message.as_deref(),
        Some("Subscription fetch already in progress")
    );
    assert!(app.status_message.is_none());
    assert!(app.is_fetching_subscription());
    assert!(app.subscription_fetch_elapsed().unwrap() >= started_elapsed);
}

#[test]
fn test_switching_to_subscription_without_enabled_providers_does_not_fetch() {
    let mut app = make_app_with_subscription();
    app.current_tab = Tab::Overview;

    app.update_data(UsageProjection::default());
    app.switch_tab(Tab::Subscription);

    assert!(!app.has_subscription_fetch_history());
    assert!(!app.is_fetching_subscription());
}

#[test]
fn test_initial_subscription_fetch_requires_enabled_provider() {
    let mut app = make_app_with_subscription();
    app.current_tab = Tab::Subscription;
    assert!(!app.should_start_initial_subscription_fetch());

    let mut app = make_app_with_subscription_providers(&["codex"]);
    app.current_tab = Tab::Subscription;
    assert!(app.should_start_initial_subscription_fetch());
}

#[test]
fn test_initial_subscription_fetch_starts_only_once_per_session() {
    let mut app = make_app_with_subscription_providers(&["codex"]);
    app.current_tab = Tab::Overview;
    assert!(!app.should_start_initial_subscription_fetch());

    app.current_tab = Tab::Subscription;
    assert!(app.should_start_initial_subscription_fetch());
    app.maybe_fetch_subscription_on_entry();

    app.current_tab = Tab::Models;
    assert!(!app.should_start_initial_subscription_fetch());

    app.current_tab = Tab::Subscription;
    assert!(!app.should_start_initial_subscription_fetch());
}

#[test]
fn completed_manual_fetch_prevents_an_initial_fetch_on_first_tab_entry() {
    let mut app = make_app_with_subscription_providers(&["codex"]);
    assert_eq!(app.current_tab, Tab::Overview);

    app.fetch_subscription();
    let (_, sender) = app
        .take_subscription_request()
        .expect("manual request is queued");
    sender.send(SubscriptionBatch::default()).unwrap();
    app.on_tick();
    assert!(!app.is_fetching_subscription());

    app.switch_tab(Tab::Subscription);

    assert!(app.take_subscription_request().is_none());
    assert!(!app.is_fetching_subscription());
    assert!(app.has_subscription_fetch_history());
}

#[test]
fn test_subscription_tab_rejects_generation_refresh_keys() {
    let mut app = make_app_with_subscription_providers(&["codex"]);
    app.current_tab = Tab::Subscription;
    let refresh_status = app.refresh_status();

    app.handle_key_event(key(KeyCode::Char('r')));
    app.handle_key_event(key_with_mod(KeyCode::Char('R'), KeyModifiers::SHIFT));
    app.handle_key_event(key(KeyCode::Char('+')));
    app.handle_key_event(key(KeyCode::Char('-')));
    app.handle_key_event(key(KeyCode::Char('e')));

    assert!(app.take_refresh_requests().is_empty());
    assert!(app.take_refresh_controls().is_empty());
    assert_eq!(app.refresh_status(), refresh_status);
    assert!(app.status_message.is_none());
    assert!(!app.has_subscription_fetch_history());
    assert!(!app.is_fetching_subscription());
}

#[test]
fn test_u_outside_subscription_does_not_fetch_subscription() {
    let mut app = make_app_with_subscription_providers(&["codex"]);
    app.current_tab = Tab::Overview;

    app.handle_key_event(key(KeyCode::Char('u')));

    assert!(!app.has_subscription_fetch_history());
    assert!(!app.is_fetching_subscription());
}

#[test]
fn test_u_on_subscription_without_enabled_providers_reports_disabled() {
    let mut app = make_app_with_subscription();
    app.current_tab = Tab::Subscription;

    app.handle_key_event(key(KeyCode::Char('u')));

    assert!(!app.has_subscription_fetch_history());
    assert_eq!(
        app.subscription_status_message.as_deref(),
        Some("No subscription providers enabled")
    );
    assert!(app.status_message.is_none());
}

#[test]
fn test_handle_key_increase_decrease_refresh() {
    let mut app = make_app();

    app.handle_key_event(key(KeyCode::Char('+')));
    app.handle_key_event(key(KeyCode::Char('-')));
    assert_eq!(
        app.take_refresh_controls(),
        vec![
            RefreshControl::IncreaseInterval,
            RefreshControl::DecreaseInterval
        ]
    );
}

#[test]
fn pointer_tab_intent_switches_the_model() {
    let mut app = make_app();
    app.apply_intent(Intent::SelectTab(Tab::Models));
    assert_eq!(app.current_tab, Tab::Models);
}

#[test]
fn pointer_sort_intent_uses_the_normal_sort_transition() {
    let mut app = make_app();
    app.apply_intent(Intent::Sort(SortField::Tokens));
    assert_eq!(app.sort_field, SortField::Tokens);
}

#[test]
fn pointer_graph_intent_selects_the_cell() {
    let mut app = make_app();
    app.apply_intent(Intent::SelectGraphCell { week: 2, day: 3 });
    assert_eq!(app.selected_graph_cell, Some((2, 3)));
}

#[test]
fn move_intent_scrolls_up() {
    let mut app = make_app_with_models(5);
    app.set_selected_index(2);
    app.apply_intent(Intent::Move(MoveCommand::Up));
    assert_eq!(app.selected_index(), 1);
}

#[test]
fn move_intent_scrolls_down() {
    let mut app = make_app_with_models(5);
    app.set_selected_index(2);
    app.apply_intent(Intent::Move(MoveCommand::Down));
    assert_eq!(app.selected_index(), 3);
}

// ── handle_resize ───────────────────────────────────────────────

#[test]
fn test_handle_resize() {
    let mut app = make_app();
    assert_eq!(app.terminal_width, 80);
    assert_eq!(app.terminal_height, 24);

    app.handle_resize(120, 40);
    assert_eq!(app.terminal_width, 120);
    assert_eq!(app.terminal_height, 40);
    assert_eq!(app.max_visible_items(), 20);
}

#[test]
fn test_handle_resize_small_terminal() {
    let mut app = make_app();
    app.handle_resize(40, 12);
    assert_eq!(app.terminal_width, 40);
    assert_eq!(app.terminal_height, 12);
    assert_eq!(app.max_visible_items(), 20);
}

#[test]
fn test_handle_resize_preserves_rendered_capacity() {
    let mut app = make_app_with_models(5);
    app.set_selected_index(4);
    app.set_scroll_offset(2);
    app.set_max_visible_items(3);

    app.handle_resize(80, 24);

    assert_eq!(app.max_visible_items(), 3);
    assert_eq!(app.selected_index(), 4);
    assert_eq!(app.scroll_offset(), 2);
}

#[test]
fn test_set_max_visible_items_clamps_scroll_offset() {
    let mut app = make_app_with_models(10);
    app.set_selected_index(9);
    app.set_scroll_offset(9);

    app.set_max_visible_items(3);

    assert_eq!(app.max_visible_items(), 3);
    assert_eq!(app.selected_index(), 9);
    assert_eq!(app.scroll_offset(), 7);
}

// ── on_tick ─────────────────────────────────────────────────────

#[test]
fn test_on_tick_increments_frame() {
    let mut app = make_app();
    assert_eq!(app.spinner_frame, 0);

    app.on_tick();
    assert_eq!(app.spinner_frame, 1);

    app.on_tick();
    assert_eq!(app.spinner_frame, 2);
}

#[test]
fn tick_advances_effective_date_without_an_installed_generation() {
    let mut app = make_app();
    let next_day = NaiveDate::from_ymd_opt(2026, 7, 27).unwrap();

    app.on_tick_for_date(next_day);

    assert_eq!(app.effective_date(), next_day);
    assert!(!app.has_installed_generation());
}

#[test]
fn tick_reprojects_an_installed_generation_at_local_midnight() {
    let mut app = make_app();
    app.install_generation_fixture(
        tokenx_engine::FrozenUsageIndex::new(),
        Vec::new(),
        tokenx_engine::InputFootprint::default(),
    );
    let next_day = NaiveDate::from_ymd_opt(2026, 7, 27).unwrap();

    app.on_tick_for_date(next_day);

    assert_eq!(app.effective_date(), next_day);
    assert!(app.has_installed_generation());
}

#[test]
fn test_on_tick_wraps_spinner_frame() {
    let mut app = make_app();
    app.spinner_frame = 19;
    app.on_tick();
    assert_eq!(app.spinner_frame, 0);
}

#[test]
fn test_on_tick_clears_expired_status() {
    let mut app = make_app();
    app.set_status("test message");
    assert!(app.status_message.is_some());

    app.status_message_time = Some(Instant::now() - Duration::from_secs(5));
    app.on_tick();
    assert!(app.status_message.is_none());
    assert!(app.status_message_time.is_none());
}

#[test]
fn test_on_tick_keeps_fresh_status() {
    let mut app = make_app();
    app.set_status("fresh message");

    app.on_tick();
    assert!(app.status_message.is_some());
    assert_eq!(app.status_message.as_ref().unwrap(), "fresh message");
}

#[test]
fn installing_generation_restores_typed_pricing_status() {
    use tokenx_engine::pricing::PricingDiagnostic;

    let mut app = make_app();

    app.install_generation_fixture_with_pricing_diagnostics(vec![PricingDiagnostic::warning(
        "message wording says pricing unavailable and using cached pricing",
    )]);
    assert_eq!(
        app.pricing_warning().as_deref(),
        Some("Pricing warnings; some costs may be missing")
    );

    app.install_generation_fixture_with_pricing_diagnostics(vec![
        PricingDiagnostic::cached_fallback("network error"),
    ]);
    assert_eq!(
        app.pricing_warning().as_deref(),
        Some("Pricing refresh failed; using cached rates")
    );

    app.install_generation_fixture_with_pricing_diagnostics(vec![
        PricingDiagnostic::cached_fallback("stale cache"),
        PricingDiagnostic::unavailable("network error"),
    ]);
    assert_eq!(
        app.pricing_warning().as_deref(),
        Some("Pricing unavailable; costs may be missing")
    );

    app.install_generation_fixture_with_pricing_diagnostics(Vec::new());
    assert_eq!(app.pricing_warning(), None);
}

// ── click area management ───────────────────────────────────────

// ── narrow detection ────────────────────────────────────────────

#[test]
fn test_is_narrow() {
    let mut app = make_app();
    app.terminal_width = 79;
    assert!(app.is_narrow());

    app.terminal_width = 80;
    assert!(!app.is_narrow());
}

#[test]
fn test_is_very_narrow() {
    let mut app = make_app();
    app.terminal_width = 59;
    assert!(app.is_very_narrow());

    app.terminal_width = 60;
    assert!(!app.is_very_narrow());
}

// ── HourlyViewMode tests ─────────────────────────────────────────

#[test]
fn test_hourly_view_mode_default() {
    let mode = HourlyViewMode::default();
    assert_eq!(mode, HourlyViewMode::Table);
}

#[test]
fn test_hourly_view_mode_toggle() {
    let mut app = make_app();
    let mut state = crate::tui::page_state::PageStates::default();
    assert_eq!(state.hourly_mode(), HourlyViewMode::Table);

    // Toggle to Profile when on Hourly tab
    app.current_tab = Tab::Hourly;
    state.handle_intent(&mut app, Intent::ToggleView);
    assert_eq!(state.hourly_mode(), HourlyViewMode::Profile);

    // Toggle back to Table
    state.handle_intent(&mut app, Intent::ToggleView);
    assert_eq!(state.hourly_mode(), HourlyViewMode::Table);
}

#[test]
fn test_hourly_view_toggle_preserves_other_tab_interactions() {
    let mut app = make_app_with_models(5);
    app.usage_mut_for_test().daily = vec![
        daily_usage("2026-05-10", 1.0, vec![("old-model", "anthropic", 1.0)]),
        daily_usage("2026-05-17", 7.0, vec![("target", "openai", 7.0)]),
        daily_usage("2026-05-18", 3.0, vec![("other", "google", 3.0)]),
    ];

    app.switch_tab(Tab::Models);
    app.set_max_visible_items(3);
    app.set_selected_index(4);
    app.set_scroll_offset(2);

    app.switch_tab(Tab::Daily);
    app.set_max_visible_items(2);
    app.set_selected_index(1);
    app.set_scroll_offset(1);

    app.switch_tab(Tab::Hourly);
    app.set_selected_index(3);
    app.set_scroll_offset(1);
    let mut state = crate::tui::page_state::PageStates::default();
    state.set_hourly_profile_text_viewport(1, 5);
    state.handle_intent(&mut app, Intent::Move(MoveCommand::End));

    state.handle_intent(&mut app, Intent::ToggleView);

    assert_eq!(state.hourly_mode(), HourlyViewMode::Profile);
    assert_eq!(app.selected_index(), 0);
    assert_eq!(app.scroll_offset(), 0);
    assert_eq!(state.hourly_profile_scroll(), 0);

    app.switch_tab(Tab::Models);
    assert_eq!(app.selected_index(), 4);
    assert_eq!(app.scroll_offset(), 2);
    assert_eq!(app.max_visible_items(), 3);

    app.switch_tab(Tab::Daily);
    assert_eq!(app.selected_index(), 1);
    assert_eq!(app.scroll_offset(), 1);
    assert_eq!(app.max_visible_items(), 2);
}

#[test]
fn test_hourly_view_mode_no_toggle_on_other_tabs() {
    let mut app = make_app();
    let mut state = crate::tui::page_state::PageStates::default();
    assert_eq!(state.hourly_mode(), HourlyViewMode::Table);

    // 'v' should not toggle when not on Hourly tab
    app.current_tab = Tab::Overview;
    assert!(state.handle_intent(&mut app, Intent::ToggleView));
    assert_eq!(state.hourly_mode(), HourlyViewMode::Table);

    app.current_tab = Tab::Daily;
    state.handle_intent(&mut app, Intent::ToggleView);
    assert_eq!(state.hourly_mode(), HourlyViewMode::Table);
}
