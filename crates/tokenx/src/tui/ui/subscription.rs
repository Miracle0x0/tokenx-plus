use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph, Scrollbar, ScrollbarOrientation};

use crate::subscription::providers::helpers;
use crate::subscription::{SubscriptionError, SubscriptionOutput};
use crate::tui::model::TuiModel;
use crate::tui::page_state::PageStates;
use crate::tui::presentation::SubscriptionPresentation;
use crate::tui::render_artifacts::RenderArtifacts;
use crate::tui::themes::Theme;
use crate::tui::ui::widgets::viewport_scrollbar_state;

const BAR_WIDTH: usize = 20;

pub fn render(
    frame: &mut Frame,
    app: &TuiModel,
    state: &PageStates,
    artifacts: &mut RenderArtifacts,
    area: Rect,
    presentation: SubscriptionPresentation,
) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(app.theme.chrome.border))
        .title(rust_i18n::t!("tui.ui.subscription.panel_title"))
        .title_style(Style::default().fg(app.theme.chrome.heading))
        .style(app.theme.panel_style());

    let inner = block.inner(area);
    frame.render_widget(block, area);

    match presentation {
        SubscriptionPresentation::ColdFetching => {
            artifacts.measure_subscription(state.subscription_viewport(), inner.height as usize, 0);
            render_fetching(frame, app, inner);
        }
        SubscriptionPresentation::Prompt => {
            artifacts.measure_subscription(state.subscription_viewport(), inner.height as usize, 0);
            render_prompt(frame, app, inner);
        }
        SubscriptionPresentation::Empty { .. } => {
            artifacts.measure_subscription(state.subscription_viewport(), inner.height as usize, 0);
            render_empty(frame, app, inner);
        }
        SubscriptionPresentation::Results { .. } => {
            render_loaded(frame, app, state, artifacts, inner)
        }
    }
}

fn render_fetching(frame: &mut Frame, app: &TuiModel, area: Rect) {
    super::loading::render(
        frame,
        app,
        area,
        rust_i18n::t!("tui.ui.loading.fetching_subscription_data"),
    );
}

fn render_prompt(frame: &mut Frame, app: &TuiModel, area: Rect) {
    render_centered_message(frame, app, area, prompt_message(app).as_ref());
}

fn render_empty(frame: &mut Frame, app: &TuiModel, area: Rect) {
    render_centered_message(frame, app, area, empty_message(app).as_ref());
}

fn render_centered_message(frame: &mut Frame, app: &TuiModel, area: Rect, message: &str) {
    let center = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage(40),
            Constraint::Length(3),
            Constraint::Percentage(40),
        ])
        .split(area)[1];

    let paragraph = Paragraph::new(message)
        .style(Style::default().fg(app.theme.text.secondary))
        .alignment(Alignment::Center);
    frame.render_widget(paragraph, center);
}

fn prompt_message(app: &TuiModel) -> std::borrow::Cow<'static, str> {
    if app.has_enabled_subscription_providers() {
        rust_i18n::t!("tui.ui.subscription.prompt_fetch")
    } else {
        rust_i18n::t!("tui.ui.subscription.prompt_no_providers")
    }
}

fn empty_message(app: &TuiModel) -> std::borrow::Cow<'static, str> {
    if app.has_enabled_subscription_providers() {
        rust_i18n::t!("tui.ui.subscription.empty")
    } else {
        rust_i18n::t!("tui.ui.subscription.prompt_no_providers")
    }
}

fn cache_display_notice(app: &TuiModel) -> Option<std::borrow::Cow<'static, str>> {
    if app.subscription_outputs().is_empty() || app.has_enabled_subscription_providers() {
        None
    } else {
        Some(rust_i18n::t!("tui.ui.subscription.cache_notice"))
    }
}

pub(crate) fn build_subscription_lines(
    theme: &Theme,
    outputs: &[SubscriptionOutput],
    errors: &[SubscriptionError],
) -> Vec<Line<'static>> {
    let mut lines: Vec<Line> = Vec::new();

    for (i, output) in outputs.iter().enumerate() {
        if i > 0 {
            lines.push(Line::from(""));
        }

        lines.push(Line::from(Span::styled(
            format!(" {} ", output.display_name()),
            Style::default()
                .fg(theme.text.primary)
                .add_modifier(Modifier::BOLD),
        )));

        for m in &output.metrics {
            let remaining = m.remaining_label.clone().unwrap_or_else(|| {
                rust_i18n::t!(
                    "tui.ui.subscription.percent_left",
                    percent = format!("{:.0}", m.remaining_percent)
                )
                .into_owned()
            });
            let bar = helpers::render_ascii_bar(m.remaining_percent, BAR_WIDTH);
            let reset = m
                .resets_at
                .as_ref()
                .map(|r| helpers::format_reset_time(r))
                .unwrap_or_default();

            let label = Span::styled(
                format!(" {:<14}", m.label),
                Style::default().fg(theme.text.primary),
            );
            let value = Span::styled(
                format!("{:<11}", remaining),
                Style::default().fg(theme.text.primary),
            );
            let bar_span = Span::styled(
                format!("{:<24}", bar),
                Style::default().fg(if m.remaining_percent < 10.0 {
                    theme.status.danger
                } else if m.remaining_percent < 25.0 {
                    theme.status.warning
                } else {
                    theme.status.success
                }),
            );
            let reset_span = Span::styled(reset, Style::default().fg(theme.text.secondary));

            lines.push(Line::from(vec![label, value, bar_span, reset_span]));
        }

        if let Some(ref email) = output.email {
            lines.push(Line::from(Span::styled(
                format!(
                    " {:<12}{email}",
                    rust_i18n::t!("tui.ui.subscription.account_label")
                ),
                Style::default().fg(theme.text.secondary),
            )));
        }
        if let Some(ref plan) = output.plan {
            lines.push(Line::from(Span::styled(
                format!(
                    " {:<12}{plan}",
                    rust_i18n::t!("tui.ui.subscription.plan_label")
                ),
                Style::default().fg(theme.text.secondary),
            )));
        }
    }

    if !errors.is_empty() {
        if !lines.is_empty() {
            lines.push(Line::from(""));
        }
        lines.push(Line::from(Span::styled(
            rust_i18n::t!("tui.ui.subscription.provider_errors"),
            Style::default()
                .fg(theme.status.danger)
                .add_modifier(Modifier::BOLD),
        )));
        for error in errors {
            lines.push(Line::from(vec![
                Span::styled(
                    format!(" {:<14}", error.provider),
                    Style::default().fg(theme.text.primary),
                ),
                Span::styled(
                    error.message.clone(),
                    Style::default().fg(theme.text.secondary),
                ),
            ]));
        }
    }

    lines
}

fn render_loaded(
    frame: &mut Frame,
    app: &TuiModel,
    state: &PageStates,
    artifacts: &mut RenderArtifacts,
    area: Rect,
) {
    let mut lines = build_subscription_lines(
        &app.theme,
        app.subscription_outputs(),
        app.subscription_errors(),
    );
    if let Some(notice) = cache_display_notice(app) {
        lines.insert(
            0,
            Line::from(Span::styled(
                notice,
                Style::default().fg(app.theme.text.secondary),
            )),
        );
        lines.insert(1, Line::from(""));
    }
    let total_lines = lines.len();
    let visible_height = area.height as usize;
    let viewport =
        artifacts.measure_subscription(state.subscription_viewport(), visible_height, total_lines);

    let range = viewport.visible_range(total_lines);
    let paragraph = Paragraph::new(lines.drain(range).collect::<Vec<_>>());
    frame.render_widget(paragraph, area);

    if total_lines > visible_height {
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(Some("▲"))
            .end_symbol(Some("▼"));

        let mut scrollbar_state =
            viewport_scrollbar_state(total_lines, viewport.scroll, visible_height);

        frame.render_stateful_widget(
            scrollbar,
            area.inner(Margin {
                horizontal: 0,
                vertical: 0,
            }),
            &mut scrollbar_state,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::Settings;
    use crate::subscription::{ProviderId, UsageAccount, UsageMetric};
    use crate::tui::model::{Tab, TuiConfig};
    use crate::tui::themes::{Theme, ThemeName};
    use ratatui::{backend::TestBackend, Terminal};

    fn make_subscription_app() -> TuiModel {
        let config = TuiConfig {
            theme: Some(crate::theme::ThemeName::Blue),
            refresh: 0,
            no_refresh: false,
            client_universe: tokenx_engine::ClientUniverse::all(),
            initial_tab: Some(Tab::Subscription),
            effective_date: chrono::NaiveDate::from_ymd_opt(2026, 7, 26).unwrap(),
        };
        let mut settings = Settings::default();
        settings.subscription.enabled = true;
        let mut app = TuiModel::new_for_test_with_settings(config, settings).unwrap();
        app.current_tab = Tab::Subscription;
        app.set_subscription_provider_ids_for_test(Vec::new());
        app
    }

    fn line_text(line: &Line<'_>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    }

    fn render_text(app: &mut TuiModel) -> String {
        let mut terminal = Terminal::new(TestBackend::new(100, 24)).unwrap();
        let mut state = PageStates::default();
        let mut artifacts = RenderArtifacts::default();
        terminal
            .draw(|frame| {
                let presentation = SubscriptionPresentation::for_app(app);
                render(
                    frame,
                    app,
                    &state,
                    &mut artifacts,
                    frame.area(),
                    presentation,
                )
            })
            .unwrap();
        state.install_render_measurements(&artifacts);
        let buffer = terminal.backend().buffer();
        let width = buffer.area.width as usize;
        buffer
            .content()
            .chunks(width)
            .map(|row| {
                row.iter()
                    .map(|cell| cell.symbol().to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn idle_prompt_does_not_render_a_loading_animation() {
        let mut app = make_subscription_app();

        let screen = render_text(&mut app);

        assert!(
            screen.contains(
                rust_i18n::t!("tui.ui.subscription.prompt_no_providers", locale = "en").as_ref()
            ),
            "{screen}"
        );
        assert!(!screen.contains('⠋'), "idle prompt must not spin: {screen}");
        assert!(
            !screen.contains("~  ~"),
            "idle prompt must not show the pond: {screen}"
        );
    }

    #[test]
    fn subscription_lines_render_provider_errors_without_outputs() {
        let theme = Theme::from_name(ThemeName::Blue);
        let errors = vec![SubscriptionError {
            provider_id: Some(ProviderId::MiniMaxTokenPlanCn),
            provider: "MiniMax Token Plan CN".to_string(),
            message: "session expired".to_string(),
        }];

        let lines = build_subscription_lines(&theme, &[], &errors);
        let text = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

        assert!(text.contains("Provider errors"));
        assert!(text.contains("MiniMax Token Plan CN"));
        assert!(text.contains("session expired"));
    }

    #[test]
    fn subscription_lines_render_the_canonical_provider_label() {
        let theme = Theme::from_name(ThemeName::Blue);
        let output = SubscriptionOutput {
            provider: ProviderId::Zai,
            stale: false,
            account: Some(UsageAccount {
                id: "account-1".to_string(),
                label: Some("Work".to_string()),
                is_active: true,
            }),
            plan: None,
            email: None,
            metrics: Vec::new(),
        };

        let lines = build_subscription_lines(&theme, &[output], &[]);
        let text = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

        assert!(text.contains("Z.ai GLM Coding Plan (Work)"));
    }

    #[test]
    fn idle_prompt_reflects_provider_availability() {
        let mut app = make_subscription_app();

        assert_eq!(
            prompt_message(&app),
            rust_i18n::t!("tui.ui.subscription.prompt_no_providers", locale = "en")
        );

        app.set_subscription_provider_ids_for_test(vec![ProviderId::Codex]);

        assert_eq!(
            prompt_message(&app),
            rust_i18n::t!("tui.ui.subscription.prompt_fetch", locale = "en")
        );
    }

    #[test]
    fn empty_prompt_requires_enabled_provider() {
        let mut app = make_subscription_app();

        assert_eq!(
            empty_message(&app),
            rust_i18n::t!("tui.ui.subscription.prompt_no_providers", locale = "en")
        );

        app.set_subscription_provider_ids_for_test(vec![ProviderId::Codex]);

        assert_eq!(
            empty_message(&app),
            rust_i18n::t!("tui.ui.subscription.empty", locale = "en")
        );
    }

    #[test]
    fn cached_subscription_notice_only_shows_without_enabled_provider() {
        let mut app = make_subscription_app();
        app.subscription_outputs_mut_for_test()
            .push(SubscriptionOutput {
                provider: ProviderId::Codex,
                stale: false,
                account: None,
                plan: None,
                email: None,
                metrics: vec![UsageMetric {
                    label: "Weekly".to_string(),
                    used_percent: 10.0,
                    remaining_percent: 90.0,
                    remaining_label: None,
                    resets_at: None,
                }],
            });

        assert_eq!(
            cache_display_notice(&app),
            Some(rust_i18n::t!(
                "tui.ui.subscription.cache_notice",
                locale = "en"
            ))
        );

        app.set_subscription_provider_ids_for_test(vec![ProviderId::Codex]);

        assert_eq!(cache_display_notice(&app), None);
    }
}
