use std::borrow::Cow;
use std::collections::BTreeSet;

use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Padding, Paragraph};

use super::widgets::{format_cost, format_tokens, truncate_display_width};
use crate::terminal_text::{pad_left, width as text_width, width_u16};
use crate::tui::actions::{Action, ActionSet};
use crate::tui::data::PeriodKind;
use crate::tui::display_labels::{group_by_label, theme_label};
use crate::tui::intent::Intent;
use crate::tui::model::{SortField, StatusTone, Tab, TuiModel};
use crate::tui::page_state::PageStates;
use crate::tui::presentation::SubscriptionPresentation;
use crate::tui::render_artifacts::RenderArtifacts;

pub(super) const HEIGHT: u16 = 5;

#[derive(Clone)]
pub(super) struct SortControl {
    pub(super) field: SortField,
    pub(super) label: Cow<'static, str>,
}

impl SortControl {
    pub(super) fn new(field: SortField, label: impl Into<Cow<'static, str>>) -> Self {
        Self {
            field,
            label: label.into(),
        }
    }
}

pub(super) struct ResponsiveLine {
    full: Line<'static>,
    compact: Line<'static>,
}

impl ResponsiveLine {
    pub(super) fn new(full: Line<'static>, compact: Line<'static>) -> Self {
        Self { full, compact }
    }

    fn for_width(&self, width: usize) -> Line<'static> {
        if self.full.width() <= width {
            self.full.clone()
        } else {
            self.compact.clone()
        }
    }
}

impl From<Line<'static>> for ResponsiveLine {
    fn from(line: Line<'static>) -> Self {
        Self {
            compact: line.clone(),
            full: line,
        }
    }
}

struct HelpItem {
    full: String,
    compact: String,
    style: Style,
}

impl HelpItem {
    fn new(full: impl Into<String>, compact: impl Into<String>, style: Style) -> Self {
        Self {
            full: full.into(),
            compact: compact.into(),
            style,
        }
    }
}

pub(super) struct HelpLine {
    items: Vec<HelpItem>,
    full_separator: &'static str,
    separator_style: Style,
}

impl HelpLine {
    fn new(items: Vec<HelpItem>, full_separator: &'static str, separator_style: Style) -> Self {
        Self {
            items,
            full_separator,
            separator_style,
        }
    }

    fn for_width(&self, width: usize) -> Line<'static> {
        let full = self.full_line();
        if full.width() <= width {
            return full;
        }

        if let Some(line) = self.progressive_line(width) {
            return line;
        }

        self.fitted_compact_line(width)
    }

    fn full_line(&self) -> Line<'static> {
        let mut spans = Vec::new();
        for item in &self.items {
            push_help_span(
                &mut spans,
                &item.full,
                item.style,
                self.full_separator,
                self.separator_style,
            );
        }
        Line::from(spans)
    }

    fn progressive_line(&self, width: usize) -> Option<Line<'static>> {
        let separator = "·";
        let separator_width = text_width(separator) * self.items.len().saturating_sub(1);
        let compact_width = self
            .items
            .iter()
            .map(|item| text_width(&item.compact))
            .sum::<usize>()
            + separator_width;
        let mut remaining = width.checked_sub(compact_width)?;
        let mut spans = Vec::new();

        // Preserve display order while letting smaller later expansions use
        // space that an earlier full label cannot consume.
        for item in &self.items {
            let expansion_width = text_width(&item.full)
                .checked_sub(text_width(&item.compact))
                .expect("full help label must not be narrower than compact label");
            let label = if expansion_width <= remaining {
                remaining -= expansion_width;
                &item.full
            } else {
                &item.compact
            };
            push_help_span(
                &mut spans,
                label,
                item.style,
                separator,
                self.separator_style,
            );
        }

        Some(Line::from(spans))
    }

    fn fitted_compact_line(&self, width: usize) -> Line<'static> {
        // Keep the final action visible and elide only at item boundaries.
        let Some(last) = self.items.last() else {
            return Line::default();
        };
        if self.items.len() == 1 || width < text_width("…·") + text_width(&last.compact) {
            return Line::from(Span::styled(
                truncate_display_width(&last.compact, width),
                last.style,
            ));
        }

        let suffix_width = text_width("·…·") + text_width(&last.compact);
        let separator_width = text_width("·");
        let mut prefix_width = 0;
        let mut kept = Vec::new();
        for item in &self.items[..self.items.len() - 1] {
            let item_width = text_width(&item.compact);
            let next_width =
                prefix_width + separator_width * usize::from(!kept.is_empty()) + item_width;
            if next_width + suffix_width > width {
                break;
            }
            kept.push(item);
            prefix_width = next_width;
        }

        let mut spans = Vec::new();
        for item in kept {
            push_help_span(
                &mut spans,
                &item.compact,
                item.style,
                "·",
                self.separator_style,
            );
        }
        push_help_span(
            &mut spans,
            "…",
            self.separator_style,
            "·",
            self.separator_style,
        );
        push_help_span(
            &mut spans,
            &last.compact,
            last.style,
            "·",
            self.separator_style,
        );
        Line::from(spans)
    }
}

fn push_help_span(
    spans: &mut Vec<Span<'static>>,
    label: &str,
    style: Style,
    separator: &str,
    separator_style: Style,
) {
    if !spans.is_empty() {
        spans.push(Span::styled(separator.to_string(), separator_style));
    }
    spans.push(Span::styled(label.to_string(), style));
}

pub(super) struct FooterContent {
    sort_controls: Vec<SortControl>,
    leading: Option<String>,
    summary: ResponsiveLine,
    help: HelpLine,
    status: Option<Line<'static>>,
}

impl FooterContent {
    pub(super) fn new(
        sort_controls: Vec<SortControl>,
        summary: impl Into<ResponsiveLine>,
        help: HelpLine,
    ) -> Self {
        Self {
            sort_controls,
            leading: None,
            summary: summary.into(),
            help,
            status: None,
        }
    }

    pub(super) fn with_leading(mut self, leading: String) -> Self {
        self.leading = Some(leading);
        self
    }

    pub(super) fn with_status(mut self, status: Line<'static>) -> Self {
        self.status = Some(status);
        self
    }
}

pub(super) fn standard_content(
    app: &TuiModel,
    state: &PageStates,
    actions: &ActionSet,
) -> FooterContent {
    debug_assert_ne!(app.current_tab, Tab::Sessions);
    let content = FooterContent::new(
        standard_sort_controls(actions),
        summary_row_line(app, actions),
        help_row_line(app, state, actions),
    );
    with_empty_scope(content, app, actions)
}

pub(super) fn subscription_content(
    app: &TuiModel,
    presentation: SubscriptionPresentation,
    actions: &ActionSet,
) -> FooterContent {
    FooterContent::new(
        Vec::new(),
        subscription_summary_line(app, presentation),
        subscription_help_line(app, actions),
    )
    .with_status(subscription_status_row_line(app))
}

pub(super) fn standard_sort_controls(actions: &ActionSet) -> Vec<SortControl> {
    [
        SortControl::new(SortField::Date, rust_i18n::t!("tui.ui.footer.sort.date")),
        SortControl::new(SortField::Cost, rust_i18n::t!("tui.ui.footer.sort.cost")),
        SortControl::new(
            SortField::Tokens,
            rust_i18n::t!("tui.ui.footer.sort.tokens"),
        ),
    ]
    .into_iter()
    .filter(|control| actions.contains(Action::Sort(control.field)))
    .collect()
}

pub(super) fn with_empty_scope(
    content: FooterContent,
    app: &TuiModel,
    actions: &ActionSet,
) -> FooterContent {
    if !actions.is_empty_view() {
        return content;
    }

    content.with_leading(
        rust_i18n::t!(
            "tui.ui.footer.scope_label",
            scope = super::empty_state::scope_summary(app)
        )
        .into_owned(),
    )
}

pub(super) fn render(
    frame: &mut Frame,
    app: &TuiModel,
    artifacts: &mut RenderArtifacts,
    area: Rect,
    content: FooterContent,
) {
    let inner = render_shell(frame, app, area);
    if inner.is_empty() {
        return;
    }

    render_rows(frame, app, artifacts, inner, content);
}

fn render_shell(frame: &mut Frame, app: &TuiModel, area: Rect) -> Rect {
    let block = Block::default()
        .borders(Borders::ALL)
        .padding(Padding::horizontal(1))
        .border_style(Style::default().fg(app.theme.chrome.border))
        .style(app.theme.panel_style());

    let inner = block.inner(area);
    frame.render_widget(block, area);
    inner
}

fn render_rows(
    frame: &mut Frame,
    app: &TuiModel,
    artifacts: &mut RenderArtifacts,
    inner: Rect,
    content: FooterContent,
) {
    // Split into 3 rows: main summary, help text, status.
    let row_constraints = if inner.height >= 3 {
        vec![
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ]
    } else if inner.height >= 2 {
        vec![Constraint::Length(1), Constraint::Length(1)]
    } else {
        vec![Constraint::Length(1)]
    };

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints(row_constraints)
        .split(inner);

    let FooterContent {
        sort_controls,
        leading,
        summary,
        help,
        status,
    } = content;
    render_main_row(
        frame,
        app,
        artifacts,
        rows[0],
        &sort_controls,
        leading,
        summary,
    );

    if let Some(area) = rows.get(1).copied() {
        frame.render_widget(Paragraph::new(help.for_width(area.width as usize)), area);
    }

    if let Some(area) = rows.get(2).copied() {
        if let Some(status) = status {
            frame.render_widget(Paragraph::new(status), area);
        } else {
            render_status_row(frame, app, area);
        }
    }
}

pub(super) fn render_cold_loading(frame: &mut Frame, app: &TuiModel, area: Rect) {
    render_timed_activity(
        frame,
        app,
        area,
        rust_i18n::t!("tui.ui.loading.scanning_local_data"),
        rust_i18n::t!("tui.ui.footer.activity.scanning"),
        app.background_load_elapsed().unwrap_or_default().as_secs(),
    );
}

pub(super) fn render_timed_activity(
    frame: &mut Frame,
    app: &TuiModel,
    area: Rect,
    message: Cow<'static, str>,
    compact_message: Cow<'static, str>,
    elapsed_secs: u64,
) {
    let inner = render_shell(frame, app, area);
    if inner.is_empty() {
        return;
    }

    render_centered_line(
        frame,
        inner,
        timed_activity_line(app, inner.width, message, compact_message, elapsed_secs),
    );
}

pub(super) fn render_cold_failed(
    frame: &mut Frame,
    app: &TuiModel,
    area: Rect,
    actions: &ActionSet,
) {
    debug_assert!(actions.contains(Action::RefreshLocal));
    debug_assert!(actions.contains(Action::Quit));

    let inner = render_shell(frame, app, area);
    if inner.is_empty() {
        return;
    }

    render_centered_line(frame, inner, cold_failed_line(app, inner.width));
}

fn render_centered_line(frame: &mut Frame, area: Rect, line: Line<'static>) {
    let row = Rect {
        y: area.y + area.height / 2,
        height: 1,
        ..area
    };
    frame.render_widget(Paragraph::new(line).alignment(Alignment::Center), row);
}

fn timed_activity_line(
    app: &TuiModel,
    width: u16,
    message: Cow<'static, str>,
    compact_message: Cow<'static, str>,
    elapsed_secs: u64,
) -> Line<'static> {
    const WAVE: &str = "~ ~";
    const MIN_WAVE_WIDTH: usize = 56;
    const TIMER_WIDTH: usize = 4;

    let elapsed = elapsed_label_for_locale(elapsed_secs, &rust_i18n::locale());
    let elapsed = pad_left(elapsed.as_ref(), TIMER_WIDTH);
    let plain = format!("{message} ·{elapsed}");
    let decorated = format!("{WAVE}  {plain}  {WAVE}");
    let available = width as usize;

    if available >= MIN_WAVE_WIDTH && text_width(decorated.as_str()) <= available {
        return Line::from(vec![
            Span::styled(
                WAVE.to_string(),
                Style::default().fg(app.theme.status.pending),
            ),
            Span::raw("  "),
            Span::styled(message, Style::default().fg(app.theme.text.secondary)),
            Span::styled(" ·", Style::default().fg(app.theme.text.secondary)),
            Span::styled(
                elapsed,
                Style::default()
                    .fg(app.theme.text.primary)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("  "),
            Span::styled(
                WAVE.to_string(),
                Style::default().fg(app.theme.status.pending),
            ),
        ]);
    }

    if text_width(plain.as_str()) <= available {
        return Line::from(vec![
            Span::styled(message, Style::default().fg(app.theme.text.secondary)),
            Span::styled(" ·", Style::default().fg(app.theme.text.secondary)),
            Span::styled(
                elapsed,
                Style::default()
                    .fg(app.theme.text.primary)
                    .add_modifier(Modifier::BOLD),
            ),
        ]);
    }

    let compact = format!("{compact_message} ·{elapsed}");
    Line::from(Span::styled(
        truncate_display_width(&compact, available),
        Style::default().fg(app.theme.text.secondary),
    ))
}

fn elapsed_label_for_locale(elapsed_secs: u64, locale: &str) -> Cow<'static, str> {
    rust_i18n::t!(
        "tui.ui.footer.activity.elapsed",
        locale = locale,
        secs = elapsed_secs.to_string()
    )
}

fn cold_failed_line(app: &TuiModel, width: u16) -> Line<'static> {
    let scan_failed = rust_i18n::t!("tui.ui.footer.cold_failed.scan_failed");
    let retry = rust_i18n::t!("tui.ui.footer.cold_failed.retry");
    let quit = rust_i18n::t!("tui.ui.footer.cold_failed.quit");
    let separator = " · ";
    let available = width as usize;

    let full_width = text_width(scan_failed.as_ref())
        + text_width(separator) * 2
        + text_width(retry.as_ref())
        + text_width(quit.as_ref());
    if full_width <= available {
        return Line::from(vec![
            Span::styled(
                scan_failed,
                Style::default()
                    .fg(app.theme.status.danger)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(separator, Style::default().fg(app.theme.text.secondary)),
            Span::styled(retry.clone(), Style::default().fg(app.theme.chrome.focus)),
            Span::styled(separator, Style::default().fg(app.theme.text.secondary)),
            Span::styled(quit.clone(), Style::default().fg(app.theme.text.secondary)),
        ]);
    }

    let actions_width =
        text_width(retry.as_ref()) + text_width(separator) + text_width(quit.as_ref());
    if actions_width <= available {
        return Line::from(vec![
            Span::styled(retry, Style::default().fg(app.theme.chrome.focus)),
            Span::styled(separator, Style::default().fg(app.theme.text.secondary)),
            Span::styled(quit, Style::default().fg(app.theme.text.secondary)),
        ]);
    }

    let compact_label = rust_i18n::t!("tui.ui.footer.cold_failed.compact");
    let compact = if text_width(compact_label.as_ref()) <= available {
        compact_label.into_owned()
    } else if available >= 7 {
        rust_i18n::t!("tui.ui.footer.cold_failed.keys").into_owned()
    } else {
        truncate_display_width(
            &rust_i18n::t!("tui.ui.footer.cold_failed.minimal"),
            available,
        )
    };
    Line::from(Span::styled(
        compact,
        Style::default().fg(app.theme.text.secondary),
    ))
}

fn render_main_row(
    frame: &mut Frame,
    app: &TuiModel,
    artifacts: &mut RenderArtifacts,
    area: Rect,
    sort_controls: &[SortControl],
    leading: Option<String>,
    summary: ResponsiveLine,
) {
    let left_width = if sort_controls.is_empty() {
        leading.as_deref().map_or(0, text_width)
    } else {
        sort_controls_width(sort_controls)
    };
    let summary_width = (area.width as usize).saturating_sub(left_width.saturating_add(1));
    let split_summary = summary.for_width(summary_width);
    let split_fits =
        left_width > 0 && left_width + 1 + split_summary.width() <= area.width as usize;

    if split_fits {
        let left_area = Rect::new(area.x, area.y, left_width as u16, 1);
        let summary_area = Rect::new(
            left_area.right().saturating_add(1),
            area.y,
            area.width.saturating_sub(left_area.width + 1),
            1,
        );
        if sort_controls.is_empty() {
            frame.render_widget(
                Paragraph::new(leading.expect("measured leading footer text"))
                    .style(Style::default().fg(app.theme.text.secondary)),
                left_area,
            );
        } else {
            render_sort_controls(frame, app, artifacts, left_area, sort_controls);
        }
        frame.render_widget(
            Paragraph::new(split_summary).alignment(Alignment::Right),
            summary_area,
        );
        return;
    }

    if let Some(leading) = leading {
        frame.render_widget(
            Paragraph::new(truncate_display_width(&leading, area.width as usize))
                .style(Style::default().fg(app.theme.text.secondary)),
            area,
        );
        return;
    }

    frame.render_widget(
        Paragraph::new(summary.for_width(area.width as usize)).alignment(Alignment::Right),
        area,
    );
}

fn sort_controls_width(sort_controls: &[SortControl]) -> usize {
    text_width(rust_i18n::t!("tui.ui.footer.sort_prefix").as_ref())
        + sort_controls
            .iter()
            .map(|control| text_width(control.label.as_ref()))
            .sum::<usize>()
        + sort_controls.len().saturating_sub(1)
}

fn render_sort_controls(
    frame: &mut Frame,
    app: &TuiModel,
    artifacts: &mut RenderArtifacts,
    area: Rect,
    sort_controls: &[SortControl],
) {
    let sort_prefix = rust_i18n::t!("tui.ui.footer.sort_prefix");
    let mut spans = vec![Span::styled(
        sort_prefix.clone(),
        Style::default().fg(app.theme.text.secondary),
    )];
    let mut x_offset = area.x.saturating_add(width_u16(sort_prefix.as_ref()));

    for (index, control) in sort_controls.iter().enumerate() {
        if index > 0 {
            spans.push(Span::raw(" "));
            x_offset = x_offset.saturating_add(1);
        }
        let style = if app.sort_field == control.field {
            Style::default()
                .fg(app.theme.chrome.current)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(app.theme.text.secondary)
        };
        spans.push(Span::styled(control.label.clone(), style));

        let label_width = width_u16(control.label.as_ref());
        artifacts.add_hit_target(
            Rect::new(x_offset, area.y, label_width, 1),
            Intent::Sort(control.field),
        );
        x_offset = x_offset.saturating_add(label_width);
    }

    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

pub(super) fn summary_row_line(app: &TuiModel, actions: &ActionSet) -> ResponsiveLine {
    let mut right_spans: Vec<Span> = Vec::new();

    // Total tokens
    let total_tokens = app.usage().total_tokens;
    right_spans.push(Span::styled(
        format_tokens(total_tokens),
        Style::default().fg(app.theme.metrics.tokens),
    ));
    if !actions.is_empty_view() {
        right_spans.push(Span::styled(
            rust_i18n::t!("tui.ui.footer.tokens_suffix"),
            Style::default().fg(app.theme.text.secondary),
        ));
    }

    right_spans.push(Span::styled(
        " | ",
        Style::default().fg(app.theme.text.secondary),
    ));

    // Total cost
    right_spans.push(Span::styled(
        format_cost(app.usage().total_cost),
        Style::default()
            .fg(app.theme.metrics.cost)
            .add_modifier(Modifier::BOLD),
    ));

    right_spans.push(Span::styled(
        current_count_label(app),
        Style::default().fg(app.theme.text.secondary),
    ));

    ResponsiveLine::new(
        Line::from(right_spans),
        Line::from(vec![
            Span::styled(
                format_tokens(total_tokens),
                Style::default().fg(app.theme.metrics.tokens),
            ),
            Span::styled(" | ", Style::default().fg(app.theme.text.secondary)),
            Span::styled(
                format_cost(app.usage().total_cost),
                Style::default()
                    .fg(app.theme.metrics.cost)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
    )
}

fn subscription_summary_line(
    app: &TuiModel,
    presentation: SubscriptionPresentation,
) -> Line<'static> {
    match presentation {
        SubscriptionPresentation::ColdFetching => Line::default(),
        SubscriptionPresentation::Prompt => {
            let configured = app.enabled_subscription_provider_count();
            if configured == 0 {
                Line::from(Span::styled(
                    rust_i18n::t!("tui.ui.footer.summary.no_providers_configured"),
                    Style::default().fg(app.theme.text.secondary),
                ))
            } else {
                Line::from(vec![
                    Span::styled(
                        count_label(
                            configured,
                            &rust_i18n::t!("tui.ui.footer.word.provider_one"),
                            &rust_i18n::t!("tui.ui.footer.word.provider_other"),
                        ),
                        Style::default().fg(app.theme.metrics.total),
                    ),
                    Span::styled(
                        rust_i18n::t!("tui.ui.footer.summary.configured_suffix"),
                        Style::default().fg(app.theme.text.secondary),
                    ),
                ])
            }
        }
        SubscriptionPresentation::Empty { .. } if app.subscription_outputs().is_empty() => {
            Line::from(Span::styled(
                rust_i18n::t!("tui.ui.footer.summary.no_subscription_results"),
                Style::default().fg(app.theme.text.secondary),
            ))
        }
        SubscriptionPresentation::Empty { .. } | SubscriptionPresentation::Results { .. } => {
            let subscriptions = app.subscription_outputs().len();
            let errors = app.subscription_errors().len();
            let mut spans = vec![Span::styled(
                count_label(
                    subscriptions,
                    &rust_i18n::t!("tui.ui.footer.word.subscription_one"),
                    &rust_i18n::t!("tui.ui.footer.word.subscription_other"),
                ),
                Style::default().fg(app.theme.metrics.total),
            )];
            if errors > 0 {
                spans.push(Span::styled(
                    " · ",
                    Style::default().fg(app.theme.text.secondary),
                ));
                spans.push(Span::styled(
                    count_label(
                        errors,
                        &rust_i18n::t!("tui.ui.footer.word.error_one"),
                        &rust_i18n::t!("tui.ui.footer.word.error_other"),
                    ),
                    Style::default().fg(app.theme.status.danger),
                ));
            }
            Line::from(spans)
        }
    }
}

fn count_label(count: usize, singular: &str, plural: &str) -> String {
    format!("{count} {}", if count == 1 { singular } else { plural })
}

fn current_count_label(app: &TuiModel) -> String {
    match app.current_tab {
        Tab::Overview => {
            let mut models = BTreeSet::new();
            let mut clients = BTreeSet::new();
            for day in &app.usage().daily {
                for (client, client_info) in &day.client_breakdown {
                    clients.insert(client.as_str());
                    for model in &client_info.models {
                        models.insert(model.model_id.as_ref());
                    }
                }
            }
            rust_i18n::t!(
                "tui.ui.footer.count.overview",
                models = models.len(),
                clients = clients.len(),
                days = app.usage().daily.len()
            )
            .into_owned()
        }
        Tab::Models if app.is_model_detail_active() => rust_i18n::t!(
            "tui.ui.footer.count.provider_rows",
            count = app.model_row_count()
        )
        .into_owned(),
        Tab::Models => rust_i18n::t!(
            "tui.ui.footer.count.models",
            count = app.usage().models.len()
        )
        .into_owned(),
        Tab::Agents => rust_i18n::t!(
            "tui.ui.footer.count.agents",
            count = app.usage().agents.len()
        )
        .into_owned(),
        Tab::Daily if app.is_daily_detail_active() => rust_i18n::t!(
            "tui.ui.footer.count.models",
            count = app.daily_detail_row_count()
        )
        .into_owned(),
        Tab::Monthly if app.is_period_detail_active_for_kind(PeriodKind::Monthly) => rust_i18n::t!(
            "tui.ui.footer.count.models",
            count = app.period_detail_row_count()
        )
        .into_owned(),
        Tab::Weekly if app.is_period_detail_active_for_kind(PeriodKind::Weekly) => rust_i18n::t!(
            "tui.ui.footer.count.models",
            count = app.period_detail_row_count()
        )
        .into_owned(),
        Tab::Monthly => rust_i18n::t!(
            "tui.ui.footer.count.months",
            count = app.period_usage(PeriodKind::Monthly).len()
        )
        .into_owned(),
        Tab::Weekly => rust_i18n::t!(
            "tui.ui.footer.count.weeks",
            count = app.period_usage(PeriodKind::Weekly).len()
        )
        .into_owned(),
        Tab::Daily => {
            rust_i18n::t!("tui.ui.footer.count.days", count = app.usage().daily.len()).into_owned()
        }
        Tab::Hourly => rust_i18n::t!(
            "tui.ui.footer.count.hours",
            count = app.usage().hourly.len()
        )
        .into_owned(),
        Tab::Sessions => unreachable!("sessions footer supplies its own summary"),
        Tab::Stats | Tab::Subscription => String::new(),
    }
}

pub(super) fn help_row_line(app: &TuiModel, state: &PageStates, actions: &ActionSet) -> HelpLine {
    action_help_row_line(app, state, actions, None)
}

fn subscription_help_line(app: &TuiModel, actions: &ActionSet) -> HelpLine {
    let mut items = Vec::new();

    if actions.contains(Action::RefreshSubscription) {
        items.push(HelpItem::new(
            rust_i18n::t!("tui.ui.footer.help.refresh_subscription"),
            "[u]",
            Style::default().fg(app.theme.chrome.focus),
        ));
    }
    if actions.contains(Action::Scroll) {
        items.push(HelpItem::new(
            rust_i18n::t!("tui.ui.footer.help.scroll"),
            "↑↓",
            Style::default().fg(app.theme.text.secondary),
        ));
    }
    if actions.contains(Action::PreviousTab) || actions.contains(Action::NextTab) {
        items.push(HelpItem::new(
            rust_i18n::t!("tui.ui.footer.help.tab_view"),
            "←→",
            Style::default().fg(app.theme.text.secondary),
        ));
    }
    if actions.contains(Action::Theme) {
        items.push(HelpItem::new(
            rust_i18n::t!("tui.ui.footer.help.theme"),
            "[p]",
            Style::default().fg(app.theme.chrome.focus),
        ));
    }
    if actions.contains(Action::Language) {
        items.push(HelpItem::new(
            format!("[L:{}]", app.language().native_name()),
            "[L]",
            Style::default().fg(app.theme.chrome.focus),
        ));
    }
    if actions.contains(Action::Quit) {
        items.push(HelpItem::new(
            "q",
            "q",
            Style::default().fg(app.theme.text.secondary),
        ));
    }

    HelpLine::new(items, " · ", Style::default().fg(app.theme.text.secondary))
}

pub(super) fn action_help_row_line(
    app: &TuiModel,
    state: &PageStates,
    actions: &ActionSet,
    toggle_target: Option<Cow<'static, str>>,
) -> HelpLine {
    debug_assert_ne!(app.current_tab, Tab::Subscription);

    let mut items = Vec::new();
    let mut emitted_navigation = false;
    let mut emitted_sort = false;

    for action in actions.iter() {
        if actions.is_empty_view()
            && !matches!(
                action,
                Action::Clients | Action::RefreshLocal | Action::PreviousTab | Action::NextTab
            )
        {
            continue;
        }

        let (full, compact) = match action {
            Action::PreviousTab | Action::NextTab => {
                if emitted_navigation {
                    continue;
                }
                emitted_navigation = true;
                (
                    rust_i18n::t!("tui.ui.footer.help.tab_view").into_owned(),
                    "←→".to_string(),
                )
            }
            Action::Sort(_) => {
                if emitted_sort {
                    continue;
                }
                emitted_sort = true;
                (
                    rust_i18n::t!("tui.ui.footer.help.sort").into_owned(),
                    "d/t/c".to_string(),
                )
            }
            Action::Scroll => (
                rust_i18n::t!("tui.ui.footer.help.scroll").into_owned(),
                "↑↓".to_string(),
            ),
            Action::OpenDetails => (
                if app.current_tab == Tab::Sessions {
                    rust_i18n::t!("tui.ui.footer.help.enter_sessions")
                } else {
                    rust_i18n::t!("tui.ui.footer.help.enter_details")
                }
                .into_owned(),
                "↵".to_string(),
            ),
            Action::Back => (
                rust_i18n::t!("tui.ui.footer.help.back").into_owned(),
                "esc".to_string(),
            ),
            Action::ToggleView => toggle_action_labels(app, state, toggle_target.clone()),
            Action::Clients => (
                rust_i18n::t!("tui.ui.footer.help.clients").into_owned(),
                "[s]".to_string(),
            ),
            Action::GroupBy => (
                format!("[g:{}]", group_by_label(app.group_by())),
                "[g]".to_string(),
            ),
            Action::Theme => (
                format!("[p:{}]", theme_label(app.theme.name)),
                "[p]".to_string(),
            ),
            Action::Language => (
                format!("[L:{}]", app.language().native_name()),
                "[L]".to_string(),
            ),
            Action::ToggleAutoRefresh => (
                if app.auto_refresh_enabled() {
                    rust_i18n::t!(
                        "tui.ui.footer.help.auto_refresh_on",
                        secs = app.auto_refresh_interval().as_secs()
                    )
                    .into_owned()
                } else {
                    rust_i18n::t!("tui.ui.footer.help.auto_refresh_off").into_owned()
                },
                "[R]".to_string(),
            ),
            Action::RefreshLocal => (
                rust_i18n::t!("tui.ui.footer.help.rescan").into_owned(),
                "[r]".to_string(),
            ),
            Action::IncreaseRefreshInterval
            | Action::DecreaseRefreshInterval
            | Action::RefreshSubscription
            | Action::Copy => continue,
            Action::Export => ("e".to_string(), "e".to_string()),
            Action::Quit => ("q".to_string(), "q".to_string()),
        };

        items.push(HelpItem::new(full, compact, action_style(app, action)));
    }

    HelpLine::new(items, " • ", Style::default().fg(app.theme.text.secondary))
}

fn toggle_action_labels(
    app: &TuiModel,
    state: &PageStates,
    target: Option<Cow<'static, str>>,
) -> (String, String) {
    let default_view = || rust_i18n::t!("tui.ui.footer.toggle.view");
    let (key, target) = match app.current_tab {
        Tab::Overview => (
            'h',
            match state.overview_granularity() {
                crate::tui::model::ChartGranularity::Daily => {
                    rust_i18n::t!("tui.ui.footer.toggle.hourly")
                }
                crate::tui::model::ChartGranularity::Hourly => {
                    rust_i18n::t!("tui.ui.footer.toggle.daily")
                }
            },
        ),
        Tab::Daily => ('v', target.unwrap_or_else(default_view)),
        Tab::Hourly => (
            'v',
            match state.hourly_mode() {
                crate::tui::model::HourlyViewMode::Table => {
                    rust_i18n::t!("tui.ui.footer.toggle.profile")
                }
                crate::tui::model::HourlyViewMode::Profile => {
                    rust_i18n::t!("tui.ui.footer.toggle.table")
                }
            },
        ),
        _ => ('v', target.unwrap_or_else(default_view)),
    };
    (format!("[{key}:{target}]"), key.to_string())
}

fn action_style(app: &TuiModel, action: Action) -> Style {
    let color = match action {
        Action::Sort(_) => app.theme.chrome.current,
        Action::Clients | Action::GroupBy | Action::Theme | Action::Language => {
            app.theme.chrome.focus
        }
        Action::ToggleAutoRefresh if app.auto_refresh_enabled() => app.theme.status.success,
        Action::OpenDetails | Action::Back | Action::ToggleView | Action::RefreshLocal => {
            app.theme.chrome.focus
        }
        Action::Scroll
        | Action::PreviousTab
        | Action::NextTab
        | Action::ToggleAutoRefresh
        | Action::IncreaseRefreshInterval
        | Action::DecreaseRefreshInterval
        | Action::RefreshSubscription
        | Action::Copy
        | Action::Export
        | Action::Quit => app.theme.text.secondary,
    };
    Style::default().fg(color)
}

pub(super) fn render_status_row(frame: &mut Frame, app: &TuiModel, area: Rect) {
    debug_assert_ne!(app.current_tab, Tab::Subscription);
    let paragraph = Paragraph::new(status_row_line(app));
    frame.render_widget(paragraph, area);
}

fn status_style(app: &TuiModel, tone: StatusTone) -> Style {
    let color = match tone {
        StatusTone::Info => app.theme.status.info,
        StatusTone::Success => app.theme.status.success,
        StatusTone::Warning => app.theme.status.warning,
        StatusTone::Danger => app.theme.status.danger,
    };
    Style::default().fg(color).add_modifier(Modifier::BOLD)
}

fn status_row_line(app: &TuiModel) -> Line<'static> {
    if let Some(warning) = app.generation_cache_warning() {
        return Line::from(Span::styled(
            warning.to_string(),
            Style::default()
                .fg(app.theme.status.warning)
                .add_modifier(Modifier::BOLD),
        ));
    }

    // Cold loading and cold failure use the centered footer presentation.
    // The standard status row stays reserved for installed generations.
    if app.is_cold_loading() || app.is_cold_failed() {
        return Line::default();
    }

    let mut spans: Vec<Span> = Vec::new();

    if app.is_background_loading() {
        spans.push(Span::styled(
            rust_i18n::t!("tui.ui.footer.status.refreshing"),
            Style::default().fg(app.theme.status.pending),
        ));
    } else if let Some(ref msg) = app.status_message {
        spans.push(Span::styled(
            msg.clone(),
            status_style(app, app.status_message_tone()),
        ));
    } else if let Some(warning) = app.pricing_warning() {
        spans.push(Span::styled(
            warning,
            Style::default()
                .fg(app.theme.status.warning)
                .add_modifier(Modifier::BOLD),
        ));
    } else {
        let elapsed = app.last_refresh_elapsed();
        let ago = elapsed_label(elapsed);
        spans.push(Span::styled(
            rust_i18n::t!("tui.ui.footer.status.last_updated", ago = ago).into_owned(),
            Style::default().fg(app.theme.text.secondary),
        ));

        if app.auto_refresh_enabled() {
            spans.push(Span::styled(
                rust_i18n::t!(
                    "tui.ui.footer.status.auto_refresh",
                    secs = app.auto_refresh_interval().as_secs()
                )
                .into_owned(),
                Style::default().fg(app.theme.text.secondary),
            ));
        }
    }

    Line::from(spans)
}

fn subscription_status_row_line(app: &TuiModel) -> Line<'static> {
    let (text, style) = if app.is_fetching_subscription() {
        (
            rust_i18n::t!("tui.ui.footer.subscription.refreshing").into_owned(),
            Style::default()
                .fg(app.theme.status.pending)
                .add_modifier(Modifier::BOLD),
        )
    } else if let Some(msg) = subscription_status_message(app) {
        (
            msg.to_string(),
            status_style(app, app.subscription_status_message_tone()),
        )
    } else if let Some(msg) = app.general_status_message() {
        (
            msg.to_string(),
            status_style(app, app.status_message_tone()),
        )
    } else if let Some(updated_at) = app.last_subscription_check() {
        (
            rust_i18n::t!(
                "tui.ui.footer.subscription.checked",
                ago = elapsed_label(updated_at.elapsed())
            )
            .into_owned(),
            Style::default().fg(app.theme.text.secondary),
        )
    } else if !app.subscription_outputs().is_empty() {
        (
            if app.has_enabled_subscription_providers() {
                rust_i18n::t!("tui.ui.footer.subscription.loaded_from_cache").into_owned()
            } else {
                rust_i18n::t!("tui.ui.footer.subscription.cached_no_providers").into_owned()
            },
            Style::default().fg(app.theme.text.secondary),
        )
    } else if !app.has_enabled_subscription_providers() {
        (
            rust_i18n::t!("tui.ui.footer.subscription.no_providers").into_owned(),
            Style::default().fg(app.theme.text.secondary),
        )
    } else {
        (
            rust_i18n::t!("tui.ui.footer.subscription.press_u").into_owned(),
            Style::default().fg(app.theme.text.secondary),
        )
    };

    Line::from(vec![Span::styled(text, style)])
}

fn subscription_status_message(app: &TuiModel) -> Option<&str> {
    app.subscription_status_message.as_deref()
}

fn elapsed_label(elapsed: std::time::Duration) -> String {
    if elapsed.as_secs() < 60 {
        rust_i18n::t!("tui.ui.footer.status.ago_secs", secs = elapsed.as_secs()).into_owned()
    } else if elapsed.as_secs() < 3600 {
        rust_i18n::t!(
            "tui.ui.footer.status.ago_mins",
            mins = elapsed.as_secs() / 60
        )
        .into_owned()
    } else {
        rust_i18n::t!(
            "tui.ui.footer.status.ago_hours",
            hours = elapsed.as_secs() / 3600
        )
        .into_owned()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::settings::Settings;
    use crate::subscription::{ProviderId, SubscriptionOutput, UsageMetric};
    use crate::tui::data::{DailyUsage, UsageModelEntry, UsageTokenBreakdown};
    use crate::tui::model::TuiConfig;
    use chrono::NaiveDate;

    #[test]
    fn elapsed_timer_unit_uses_the_requested_locale() {
        assert_eq!(elapsed_label_for_locale(12, "en"), "12s");
        assert_eq!(elapsed_label_for_locale(12, "zh-CN"), "12秒");
    }

    fn make_cold_app_on(tab: Tab) -> TuiModel {
        let config = TuiConfig {
            theme: Some(crate::theme::ThemeName::Blue),
            refresh: 0,
            no_refresh: false,
            client_universe: tokenx_engine::ClientUniverse::all(),
            initial_tab: Some(tab),
            effective_date: chrono::NaiveDate::from_ymd_opt(2026, 7, 26).unwrap(),
        };
        let mut settings = Settings::default();
        settings.subscription.enabled = true;
        TuiModel::new_for_test_with_settings(config, settings).unwrap()
    }

    fn make_app_on(tab: Tab) -> TuiModel {
        let mut app = make_cold_app_on(tab);
        app.install_generation_fixture(
            tokenx_engine::FrozenUsageIndex::new(),
            Vec::new(),
            tokenx_engine::InputFootprint::default(),
        );
        app
    }

    fn line_text(line: Line<'static>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>()
    }

    fn progressive_help_line() -> HelpLine {
        let style = Style::default();
        HelpLine::new(
            vec![
                HelpItem::new("alpha", "a", style),
                HelpItem::new("bravo", "b", style),
                HelpItem::new("q", "q", style),
            ],
            " • ",
            style,
        )
    }

    #[test]
    fn help_line_progressively_expands_labels_with_available_width() {
        let help = progressive_help_line();
        let compact_width = text_width("a·b·q");
        let first_expanded_width = text_width("alpha·b·q");
        let tight_full_width = text_width("alpha·bravo·q");
        let full_width = text_width("alpha • bravo • q");

        assert_eq!(
            line_text(help.for_width(compact_width.saturating_sub(1))),
            "…·q"
        );
        assert_eq!(line_text(help.for_width(compact_width)), "a·b·q");
        assert_eq!(line_text(help.for_width(first_expanded_width)), "alpha·b·q");
        assert_eq!(line_text(help.for_width(tight_full_width)), "alpha·bravo·q");
        assert_eq!(
            line_text(help.for_width(full_width.saturating_sub(1))),
            "alpha·bravo·q"
        );
        assert_eq!(line_text(help.for_width(full_width)), "alpha • bravo • q");
    }

    #[test]
    fn help_line_skips_an_expansion_that_does_not_fit() {
        let style = Style::default();
        let help = HelpLine::new(
            vec![
                HelpItem::new("expensive", "x", style),
                HelpItem::new("mid", "m", style),
                HelpItem::new("q", "q", style),
            ],
            " • ",
            style,
        );

        assert_eq!(line_text(help.for_width(text_width("x·mid·q"))), "x·mid·q");
    }

    #[test]
    fn help_line_uses_terminal_display_width_for_unicode_labels() {
        let style = Style::default();
        let help = HelpLine::new(
            vec![
                HelpItem::new("操作", "操", style),
                HelpItem::new("q", "q", style),
            ],
            " • ",
            style,
        );
        let width = text_width("操作·q");
        let line = help.for_width(width);

        assert_eq!(line_text(line.clone()), "操作·q");
        assert_eq!(line.width(), width);
    }

    #[test]
    fn help_line_never_exceeds_its_width_budget() {
        let help = progressive_help_line();
        let full_width = text_width("alpha • bravo • q");

        for width in 0..=full_width {
            let line = help.for_width(width);
            assert!(line.width() <= width, "width {width}: {}", line_text(line));
        }
    }

    #[test]
    fn chinese_profile_toggle_matches_the_profile_page_name() {
        assert_eq!(
            rust_i18n::t!("tui.ui.footer.toggle.profile", locale = "zh-CN"),
            "概览"
        );
    }

    fn installed_app_on(tab: Tab) -> TuiModel {
        let mut app = make_app_on(tab);
        app.install_generation_fixture(
            tokenx_engine::FrozenUsageIndex::new(),
            Vec::new(),
            Default::default(),
        );
        app
    }

    fn nonempty_installed_app_on(tab: Tab) -> TuiModel {
        let mut app = installed_app_on(tab);
        app.usage_mut_for_test().daily.push(DailyUsage {
            date: NaiveDate::from_ymd_opt(2026, 7, 22).unwrap(),
            tokens: UsageTokenBreakdown::default(),
            cost: 0.0,
            client_breakdown: BTreeMap::new(),
            message_count: 0,
            turn_count: 0,
        });
        app.usage_mut_for_test().models.push(UsageModelEntry {
            model_id: "test-model".into(),
            display_name: "Test Model".into(),
            provider: "test-provider".into(),
            clients: vec![tokenx_engine::ClientId::Codex],
            workspace_key: None,
            workspace_label: None,
            tokens: UsageTokenBreakdown::default(),
            cost: 0.0,
            session_count: 1,
        });
        app
    }

    fn help_text(app: &TuiModel) -> String {
        let state = crate::tui::page_state::PageStates::default();
        let presentation = crate::tui::presentation::Presentation::for_view(app, &state);
        let actions = ActionSet::for_view(app, &state, presentation);
        let help = match presentation {
            crate::tui::presentation::Presentation::Subscription(_) => {
                subscription_help_line(app, &actions)
            }
            _ => help_row_line(app, &state, &actions),
        };
        line_text(help.for_width(app.terminal_width.saturating_sub(4) as usize))
    }

    #[test]
    fn footer_actions_use_semantic_interaction_and_status_colors() {
        let mut app = nonempty_installed_app_on(Tab::Overview);

        assert_eq!(
            action_style(&app, Action::Sort(SortField::Date)).fg,
            Some(app.theme.chrome.current)
        );
        assert_eq!(
            action_style(&app, Action::Clients).fg,
            Some(app.theme.chrome.focus)
        );
        assert_eq!(
            action_style(&app, Action::Theme).fg,
            Some(app.theme.chrome.focus)
        );
        assert_eq!(
            action_style(&app, Action::Language).fg,
            Some(app.theme.chrome.focus)
        );
        assert_eq!(
            action_style(&app, Action::Scroll).fg,
            Some(app.theme.text.secondary)
        );

        app.set_refresh_status_for_test(
            true,
            std::time::Duration::from_secs(30),
            std::time::Instant::now(),
        );
        assert_eq!(
            action_style(&app, Action::ToggleAutoRefresh).fg,
            Some(app.theme.status.success)
        );
    }

    #[test]
    fn standard_status_messages_use_their_semantic_tone_colors() {
        let mut app = installed_app_on(Tab::Overview);

        app.set_status("Informational status");
        assert_eq!(
            status_row_line(&app).spans[0].style.fg,
            Some(app.theme.status.info)
        );
        app.set_generation_status("Local informational status");
        assert_eq!(
            status_row_line(&app).spans[0].style.fg,
            Some(app.theme.status.info)
        );

        for (tone, expected) in [
            (StatusTone::Success, app.theme.status.success),
            (StatusTone::Warning, app.theme.status.warning),
            (StatusTone::Danger, app.theme.status.danger),
        ] {
            app.set_status_with_tone("Transient status", tone);
            assert_eq!(status_row_line(&app).spans[0].style.fg, Some(expected));
        }
    }

    #[test]
    fn subscription_status_messages_use_their_semantic_tone_colors() {
        let mut app = make_app_on(Tab::Subscription);

        for (tone, expected) in [
            (StatusTone::Success, app.theme.status.success),
            (StatusTone::Warning, app.theme.status.warning),
            (StatusTone::Danger, app.theme.status.danger),
        ] {
            app.set_subscription_status_with_tone("Subscription status", tone);
            assert_eq!(
                subscription_status_row_line(&app).spans[0].style.fg,
                Some(expected)
            );
        }
    }

    #[test]
    fn cold_failure_footer_uses_danger_focus_and_muted_roles() {
        let app = make_app_on(Tab::Overview);
        let line = cold_failed_line(&app, 80);

        assert_eq!(line.spans[0].style.fg, Some(app.theme.status.danger));
        assert_eq!(line.spans[1].style.fg, Some(app.theme.text.secondary));
        assert_eq!(line.spans[2].style.fg, Some(app.theme.chrome.focus));
        assert_eq!(line.spans[4].style.fg, Some(app.theme.text.secondary));
    }

    #[test]
    fn test_current_count_label_matches_active_tab() {
        assert_eq!(
            current_count_label(&make_app_on(Tab::Overview)),
            " (0 models · 0 clients · 0 days)"
        );
        assert_eq!(
            current_count_label(&make_app_on(Tab::Models)),
            " (0 models)"
        );
        assert_eq!(
            current_count_label(&make_app_on(Tab::Agents)),
            " (0 agents)"
        );
        assert_eq!(
            current_count_label(&make_app_on(Tab::Monthly)),
            " (0 months)"
        );
        assert_eq!(current_count_label(&make_app_on(Tab::Weekly)), " (0 weeks)");
        assert_eq!(current_count_label(&make_app_on(Tab::Daily)), " (0 days)");
        assert_eq!(current_count_label(&make_app_on(Tab::Hourly)), " (0 hours)");
        assert_eq!(current_count_label(&make_app_on(Tab::Stats)), "");
    }

    #[test]
    fn subscription_help_row_only_shows_subscription_and_shell_actions() {
        let mut app = make_app_on(Tab::Overview);
        app.current_tab = Tab::Subscription;
        app.set_subscription_provider_ids_for_test(vec![ProviderId::Codex]);

        let text = help_text(&app);

        assert!(text.contains("[u:refresh]"));
        assert!(text.contains("←→/tab view"));
        assert!(text.contains("[p:theme]"));
        assert!(text.contains("[L:English]"));
        assert!(text.ends_with('q'));
        for local in ["[r:", "[R:", "[e:"] {
            assert!(!text.contains(local), "{text}");
        }
    }

    #[test]
    fn subscription_help_row_hides_subscription_refresh_without_enabled_providers() {
        let mut app = make_app_on(Tab::Overview);
        app.current_tab = Tab::Subscription;
        app.set_subscription_provider_ids_for_test(Vec::new());

        let text = help_text(&app);

        assert!(!text.contains("[u:refresh]"));
        assert!(text.contains("←→/tab view"));
        assert!(text.contains("[p:theme]"));
        assert!(text.contains("[L:English]"));
        assert!(text.ends_with('q'));
        assert!(!text.contains("local"));
    }

    #[test]
    fn fitting_subscription_help_keeps_full_labels_at_50_columns() {
        let mut app = make_app_on(Tab::Overview);
        app.current_tab = Tab::Subscription;
        app.terminal_width = 50;
        app.set_subscription_provider_ids_for_test(Vec::new());

        let text = help_text(&app);

        assert!(!text.contains("[u]"));
        assert!(text.contains("←→/tab view"));
        assert!(text.contains("[p:theme]"));
        assert!(text.contains("[L:English]"));
        assert!(text.ends_with('q'));
        assert!(!text.contains("local"));
    }

    #[test]
    fn group_by_hint_only_shows_on_group_keyed_tabs() {
        for tab in [Tab::Models, Tab::Daily, Tab::Monthly, Tab::Weekly] {
            let text = help_text(&nonempty_installed_app_on(tab));
            assert!(text.contains("[g"), "expected group hint on {tab:?}");
        }
        for tab in [
            Tab::Overview,
            Tab::Stats,
            Tab::Hourly,
            Tab::Subscription,
            Tab::Sessions,
            Tab::Agents,
        ] {
            let text = help_text(&nonempty_installed_app_on(tab));
            assert!(!text.contains("[g"), "unexpected group hint on {tab:?}");
        }
    }

    #[test]
    fn narrow_group_by_hint_only_shows_on_group_keyed_tabs() {
        for (tab, expected) in [
            (Tab::Models, true),
            (Tab::Daily, true),
            (Tab::Monthly, true),
            (Tab::Weekly, true),
            (Tab::Overview, false),
            (Tab::Stats, false),
            (Tab::Hourly, false),
            (Tab::Sessions, false),
            (Tab::Agents, false),
        ] {
            let mut app = nonempty_installed_app_on(tab);
            app.terminal_width = 50;
            let text = help_text(&app);
            assert_eq!(text.contains("[g]"), expected, "tab {tab:?}");
        }
    }

    #[test]
    fn cold_local_generation_states_leave_the_footer_status_empty() {
        let mut app = make_cold_app_on(Tab::Overview);
        app.set_refresh_loading_for_test(true);
        assert_eq!(line_text(status_row_line(&app)), "");

        app.set_refresh_loading_for_test(false);
        app.set_refresh_loading_for_test(true);
        app.fail_refresh_for_test("injected cold failure".to_string());
        app.set_generation_status("Error: injected cold failure");
        assert_eq!(line_text(status_row_line(&app)), "");
    }

    #[test]
    fn empty_installed_generation_uses_the_warm_refresh_status() {
        let mut app = make_app_on(Tab::Overview);
        app.install_generation_fixture(
            tokenx_engine::FrozenUsageIndex::new(),
            Vec::new(),
            Default::default(),
        );
        app.set_refresh_loading_for_test(true);

        assert_eq!(
            line_text(status_row_line(&app)),
            "Refreshing cached data in background..."
        );
    }

    #[test]
    fn subscription_status_row_uses_subscription_check_clock() {
        let mut app = make_app_on(Tab::Overview);
        app.current_tab = Tab::Subscription;
        app.set_refresh_status_for_test(
            false,
            std::time::Duration::from_secs(30),
            std::time::Instant::now() - std::time::Duration::from_secs(600),
        );
        app.set_last_subscription_check_for_test(Some(
            std::time::Instant::now() - std::time::Duration::from_secs(10),
        ));

        let text = line_text(subscription_status_row_line(&app));

        assert!(text.contains("Subscription checked:"));
        assert!(!text.contains("Last updated"));
        assert!(!text.contains("Auto:"));
    }

    #[test]
    fn subscription_status_row_does_not_reuse_local_cache_status() {
        let mut app = make_app_on(Tab::Overview);
        app.current_tab = Tab::Subscription;
        app.set_subscription_provider_ids_for_test(vec![ProviderId::Codex]);
        app.set_generation_status("Loaded from cache");

        let text = line_text(subscription_status_row_line(&app));

        assert_eq!(text, "Press u to refresh subscription data");
    }

    #[test]
    fn subscription_status_row_ignores_local_usage_status() {
        let mut app = make_app_on(Tab::Overview);
        app.current_tab = Tab::Subscription;
        app.set_subscription_provider_ids_for_test(vec![ProviderId::Codex]);
        app.set_generation_status("Data refreshed");

        let text = line_text(subscription_status_row_line(&app));

        assert_eq!(text, "Press u to refresh subscription data");
    }

    #[test]
    fn subscription_status_row_shows_general_action_status() {
        let mut app = make_app_on(Tab::Overview);
        app.current_tab = Tab::Subscription;
        app.set_subscription_provider_ids_for_test(vec![ProviderId::Codex]);
        app.set_status("Theme save failed: permission denied");

        let text = line_text(subscription_status_row_line(&app));

        assert_eq!(text, "Theme save failed: permission denied");
    }

    #[test]
    fn generation_warnings_do_not_cross_into_subscription_status() {
        let mut app = make_app_on(Tab::Models);
        app.status_message = None;
        app.status_message_time = None;
        app.install_generation_fixture_with_pricing_diagnostics(vec![
            tokenx_engine::pricing::PricingDiagnostic::unavailable("network error"),
        ]);

        assert_eq!(
            line_text(status_row_line(&app)),
            "Pricing unavailable; costs may be missing"
        );

        app.set_generation_cache_warning(Some(
            "Cache persistence warning: permission denied".to_string(),
        ));
        app.current_tab = Tab::Subscription;
        app.set_subscription_provider_ids_for_test(vec![ProviderId::Codex]);
        assert_eq!(
            line_text(subscription_status_row_line(&app)),
            "Press u to refresh subscription data"
        );
    }

    #[test]
    fn subscription_status_row_reports_cache_display_mode_without_providers() {
        let mut app = make_app_on(Tab::Overview);
        app.current_tab = Tab::Subscription;
        app.set_subscription_provider_ids_for_test(Vec::new());
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

        let text = line_text(subscription_status_row_line(&app));

        assert_eq!(
            text,
            "Showing cached subscription data; no remote providers enabled"
        );
    }

    #[test]
    fn subscription_status_row_reports_missing_provider_configuration() {
        let mut app = make_app_on(Tab::Overview);
        app.current_tab = Tab::Subscription;
        app.set_subscription_provider_ids_for_test(Vec::new());

        let text = line_text(subscription_status_row_line(&app));

        assert_eq!(
            text,
            "No remote subscription providers enabled; configure subscription.providers"
        );
    }

    #[test]
    fn cache_persistence_warning_stays_visible_over_transient_status() {
        let mut app = make_app_on(Tab::Models);
        app.set_status("Data loaded");
        app.set_generation_cache_warning(Some(
            "Cache persistence warning: permission denied".to_string(),
        ));

        let text = line_text(status_row_line(&app));

        assert_eq!(text, "Cache persistence warning: permission denied");
    }
}
