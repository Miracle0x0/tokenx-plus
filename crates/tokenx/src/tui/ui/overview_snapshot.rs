use std::borrow::Cow;

use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph};

use super::achievements;
use super::portraits;
use super::sessions::format_bytes;
use super::widgets::{
    format_cost, format_tokens, format_tokens_with_commas, get_client_display_name,
};
#[cfg(test)]
use crate::terminal_text::width as terminal_width;
use crate::terminal_text::{char_width, pad_right, truncate_with_ellipsis};
use crate::tui::actions::ActionSet;
use crate::tui::data::OverviewSummary;
use crate::tui::model::TuiModel;
use crate::tui::model_family::ModelFamily;
use crate::tui::page_state::PageStates;
use crate::tui::presentation::EmptySubject;
use crate::tui::render_artifacts::RenderArtifacts;
use tokenx_engine::ClientId;

const THREE_COLUMN_MIN_WIDTH: u16 = 110;
const TWO_COLUMN_MIN_WIDTH: u16 = 80;
const ONE_COLUMN_MIN_WIDTH: u16 = 40;
const METRIC_LABEL_WIDTH: usize = 20;
const CONTENT_PADDING: u16 = 1;
const EMPTY_FUN_THINGS_HEIGHT: usize = portraits::PORTRAIT_HEIGHT + 1;
// Portrait plus section/favorite spacing, slogan and family stats.
const FULL_FUN_THINGS_HEIGHT: usize = portraits::PORTRAIT_HEIGHT + 7;
// Section title, portrait, slogan and family stats without blank rows.
const COMPACT_FUN_THINGS_HEIGHT: usize = portraits::PORTRAIT_HEIGHT + 3;

pub(crate) fn render(
    frame: &mut Frame,
    app: &TuiModel,
    state: &PageStates,
    artifacts: &mut RenderArtifacts,
    area: Rect,
    empty: Option<EmptySubject>,
    actions: &ActionSet,
) {
    let snapshot_area = super::overview::render(frame, app, state, artifacts, area, empty, actions);
    if snapshot_area.is_empty() {
        return;
    }

    let data = app.overview_summary();
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(app.theme.chrome.border))
        .title(Span::styled(
            rust_i18n::t!("tui.ui.overview.snapshot_title"),
            Style::default()
                .fg(app.theme.chrome.heading)
                .add_modifier(Modifier::BOLD),
        ))
        .style(app.theme.panel_style());
    let inner = block.inner(snapshot_area);
    frame.render_widget(block, snapshot_area);
    if inner.is_empty() {
        return;
    }

    // Snapshot rows are ordered by display priority. Compact layouts intentionally
    // clip lower-priority rows from the tail so the Overview remains usable.
    if inner.width >= THREE_COLUMN_MIN_WIDTH {
        let columns = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage(38),
                Constraint::Length(1),
                Constraint::Percentage(28),
                Constraint::Length(1),
                Constraint::Percentage(34),
            ])
            .split(inner);
        render_fun_things(frame, app, section_area(columns[0]), data);
        render_divider(frame, app, columns[1]);
        render_core(frame, app, section_area(columns[2]), data);
        render_divider(frame, app, columns[3]);
        render_right(frame, app, section_area(columns[4]), data);
    } else if inner.width >= TWO_COLUMN_MIN_WIDTH {
        let columns = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage(45),
                Constraint::Length(1),
                Constraint::Percentage(55),
            ])
            .split(inner);
        render_fun_things(frame, app, section_area(columns[0]), data);
        render_divider(frame, app, columns[1]);
        render_core(frame, app, section_area(columns[2]), data);
    } else if inner.width >= ONE_COLUMN_MIN_WIDTH {
        render_core(frame, app, section_area(inner), data);
    } else {
        let inner = inner.inner(Margin {
            horizontal: CONTENT_PADDING,
            vertical: 0,
        });
        let width = inner.width as usize;
        let height = inner.height as usize;
        let mut lines = left_lines(app, data, width, height);
        lines.truncate(height);
        frame.render_widget(Paragraph::new(lines), inner);
    }
}

/// Insets a section so its content never touches the panel border or dividers.
fn section_area(area: Rect) -> Rect {
    area.inner(Margin {
        horizontal: 1,
        vertical: 0,
    })
}

/// The middle Core column: hero totals, then the Fact block. The first five
/// Fact rows line up with the achievement ladders in the right column. Input
/// health and data size stay together at the bottom as acquisition diagnostics,
/// separated from the scoped facts when vertical space permits.
fn render_core(frame: &mut Frame, app: &TuiModel, area: Rect, data: &OverviewSummary) {
    let mut lines = vec![
        section_title(app, rust_i18n::t!("tui.ui.overview.section_core")),
        Line::default(),
        Line::from(vec![
            Span::styled(
                format_tokens(app.usage().total_tokens),
                Style::default()
                    .fg(app.theme.metrics.tokens)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                rust_i18n::t!("tui.ui.overview.hero_unit_tokens"),
                Style::default().fg(app.theme.text.secondary),
            ),
            Span::styled(
                format_cost(app.usage().total_cost),
                Style::default()
                    .fg(app.theme.metrics.cost)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                rust_i18n::t!("tui.ui.overview.hero_unit_cost"),
                Style::default().fg(app.theme.text.secondary),
            ),
        ]),
        separator_line(app, area.width as usize),
        section_title(app, rust_i18n::t!("tui.ui.overview.section_fact")),
        Line::default(),
        metric_line(
            app,
            rust_i18n::t!("tui.ui.overview.metric_active_days").as_ref(),
            data.active_days.to_string(),
            app.theme.metrics.total,
        ),
        metric_line(
            app,
            rust_i18n::t!("tui.ui.overview.metric_sessions_scanned").as_ref(),
            data.main_session_count.to_string(),
            app.theme.metrics.total,
        ),
        metric_line(
            app,
            rust_i18n::t!("tui.ui.overview.metric_cache_rate").as_ref(),
            data.cache_rate.to_string(),
            app.theme.metrics.rate,
        ),
        metric_line(
            app,
            rust_i18n::t!("tui.ui.overview.metric_models_eaten").as_ref(),
            data.model_count.to_string(),
            app.theme.metrics.total,
        ),
        metric_line(
            app,
            rust_i18n::t!("tui.ui.overview.metric_clients_used").as_ref(),
            data.client_count.to_string(),
            app.theme.metrics.total,
        ),
    ];
    let diagnostics = [
        inputs_healthy_metric_line(app),
        metric_line(
            app,
            rust_i18n::t!("tui.ui.overview.metric_data_size").as_ref(),
            format_bytes(app.total_input_bytes()),
            app.theme.text.primary,
        ),
    ];

    if lines.len() + 1 + diagnostics.len() <= area.height as usize {
        lines.push(Line::default());
    }
    lines.extend(diagnostics);
    frame.render_widget(Paragraph::new(lines), area);
}

/// The Fun column: favorite model family's slogan, portrait and
/// stats, then the favorite model, client (with its own slogan) and day.
fn render_fun_things(frame: &mut Frame, app: &TuiModel, area: Rect, data: &OverviewSummary) {
    let total = data.tokens.total();
    let width = area.width as usize;

    let Some(favorite) = data.favorite_family.as_ref() else {
        render_empty_fun_things(frame, app, area);
        return;
    };
    let family = favorite.family;
    let color = portraits::family_color(app, family);
    let favorite_label = Some(Line::from(Span::styled(
        rust_i18n::t!("tui.ui.overview.metric_favorite_model"),
        Style::default().fg(app.theme.text.secondary),
    )));
    let portrait = portraits::lines(app, family).map(|line| center_line(line, width));
    let slogan = Some(centered_identity_slogan_line(
        portraits::slogan(family),
        color,
        width,
    ));
    let family_stats = Some(centered_identity_usage_line(
        app,
        portraits::display_name(family).to_string(),
        favorite.tokens,
        favorite.cost,
        total,
        color,
        width,
    ));

    let model_stats = data.favorite_model.as_ref().map(|favorite| {
        centered_identity_usage_line(
            app,
            favorite.id.clone(),
            favorite.tokens,
            favorite.cost,
            total,
            app.model_color(&favorite.id),
            width,
        )
    });

    let client_block = data
        .favorite_client
        .as_ref()
        .map(|favorite| {
            favorite_client_block(
                app,
                favorite.client,
                favorite.tokens,
                favorite.cost,
                total,
                width,
            )
        })
        .unwrap_or_default();

    let height = area.height as usize;
    let has_favorite_label = favorite_label.is_some();
    let full_height = if has_favorite_label {
        FULL_FUN_THINGS_HEIGHT
    } else {
        COMPACT_FUN_THINGS_HEIGHT
    };

    let mut lines = Vec::new();
    if height >= full_height {
        lines.push(section_title(
            app,
            rust_i18n::t!("tui.ui.overview.section_fun"),
        ));
        lines.push(Line::default());
        if let Some(label) = favorite_label {
            lines.push(label);
            lines.push(Line::default());
        }
        lines.extend(portrait);
        if let Some(slogan) = slogan {
            if has_favorite_label {
                lines.push(Line::default());
            }
            lines.push(slogan);
        }
        if let Some(family_stats) = family_stats {
            lines.push(family_stats);
        }
        if let Some(model_stats) = model_stats {
            if lines.len() < height {
                lines.push(model_stats);
            }
        }
        append_favorite_client_block(&mut lines, client_block, height);
    } else if height >= COMPACT_FUN_THINGS_HEIGHT {
        lines.push(section_title(
            app,
            rust_i18n::t!("tui.ui.overview.section_fun"),
        ));
        lines.extend(portrait);
        if let Some(slogan) = slogan {
            lines.push(slogan);
        }
        if let Some(family_stats) = family_stats {
            lines.push(family_stats);
        }
    } else {
        lines.extend(portrait);
        if let Some(family_stats) = family_stats {
            lines.push(family_stats);
        } else if let Some(slogan) = slogan {
            lines.push(slogan);
        }
    }
    lines.truncate(height);

    // No wrap: the center padding on the portrait block is meaningful and
    // `Wrap { trim: true }` would strip it.
    frame.render_widget(Paragraph::new(lines), area);
}

fn favorite_client_block(
    app: &TuiModel,
    client: ClientId,
    tokens: u64,
    cost: f64,
    total: u64,
    width: usize,
) -> Vec<Line<'static>> {
    let color = app.client_color(client);
    vec![
        Line::default(),
        Line::from(Span::styled(
            rust_i18n::t!("tui.ui.overview.metric_favorite_client"),
            Style::default().fg(app.theme.text.secondary),
        )),
        centered_identity_slogan_line(client_slogan(client), color, width),
        centered_identity_usage_line(
            app,
            get_client_display_name(client),
            tokens,
            cost,
            total,
            color,
            width,
        ),
    ]
}

fn centered_identity_slogan_line(
    slogan: impl Into<Cow<'static, str>>,
    color: Color,
    width: usize,
) -> Line<'static> {
    center_line(
        Line::from(Span::styled(slogan.into(), Style::default().fg(color))),
        width,
    )
}

fn centered_identity_usage_line(
    app: &TuiModel,
    name: String,
    tokens: u64,
    cost: f64,
    total: u64,
    color: Color,
    width: usize,
) -> Line<'static> {
    center_line(
        Line::from(vec![
            Span::styled(
                name,
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!(
                    "  {} · {:.1}% · {}",
                    format_tokens(tokens),
                    share_percent(tokens, total),
                    format_cost(cost),
                ),
                Style::default().fg(app.theme.text.secondary),
            ),
        ]),
        width,
    )
}

fn append_favorite_client_block(
    lines: &mut Vec<Line<'static>>,
    mut block: Vec<Line<'static>>,
    height: usize,
) {
    if block.is_empty() || lines.len() + block.len() > height {
        return;
    }

    // Prefer a visual gap after the heading, but keep the compact section
    // intact when that extra row would otherwise hide it.
    if lines.len() + block.len() < height {
        block.insert(2, Line::default());
    }
    lines.extend(block);
}

/// Keeps the section title anchored while centering the fixed four-line empty
/// state inside the remaining body. Tiny bodies clip from the tail.
fn render_empty_fun_things(frame: &mut Frame, app: &TuiModel, area: Rect) {
    if area.is_empty() {
        return;
    }

    let title_area = Rect {
        height: area.height.min(1),
        ..area
    };
    frame.render_widget(
        Paragraph::new(section_title(
            app,
            rust_i18n::t!("tui.ui.overview.section_fun"),
        )),
        title_area,
    );

    let body = Rect {
        y: area.y.saturating_add(1),
        height: area.height.saturating_sub(1),
        ..area
    };
    let width = body.width as usize;
    let height = body.height as usize;
    let top_padding = height.saturating_sub(EMPTY_FUN_THINGS_HEIGHT) / 2;
    let mut lines = vec![Line::default(); top_padding];
    lines.extend(portraits::lines(app, ModelFamily::Unknown).map(|line| center_line(line, width)));
    lines.push(center_line(
        Line::from(Span::styled(
            rust_i18n::t!("tui.ui.overview.empty_no_data"),
            Style::default().fg(app.theme.text.secondary),
        )),
        width,
    ));
    lines.truncate(height);
    frame.render_widget(Paragraph::new(lines), body);
}

/// Left-pads a line so it centers inside the given column width.
fn center_line(line: Line<'static>, width: usize) -> Line<'static> {
    let pad = width.saturating_sub(line.width()) / 2;
    if pad == 0 {
        return line;
    }
    let mut spans = vec![Span::raw(" ".repeat(pad))];
    spans.extend(line.spans);
    Line::from(spans)
}

fn client_slogan(client: ClientId) -> Cow<'static, str> {
    match client {
        ClientId::Pi | ClientId::Claude => {
            rust_i18n::t!("tui.ui.overview.slogan_first_class_taste")
        }
        ClientId::Kimi | ClientId::Codex | ClientId::Omp | ClientId::Droid => {
            rust_i18n::t!("tui.ui.overview.slogan_elite_player")
        }
        ClientId::Antigravity | ClientId::Copilot | ClientId::Kiro | ClientId::Gemini => {
            rust_i18n::t!("tui.ui.overview.slogan_pulled_out")
        }
        ClientId::Warp => rust_i18n::t!("tui.ui.overview.slogan_taste_above"),
        _ => rust_i18n::t!("tui.ui.overview.slogan_clueless_npc"),
    }
}

/// Input health as a Core fact: a green ✓ count when everything is clean,
/// otherwise the health percentage.
fn inputs_healthy_metric_line(app: &TuiModel) -> Line<'static> {
    let inputs = total_inputs(app);
    let health = generation_health(app);
    let (value, color) = if inputs > 0 && health.clean_inputs == inputs {
        (
            rust_i18n::t!(
                "tui.ui.overview.inputs_clean_value",
                count = format_tokens_with_commas(inputs as u64)
            )
            .into_owned(),
            app.theme.status.success,
        )
    } else {
        (health_percentage(app), health_color(app))
    };
    metric_line(
        app,
        rust_i18n::t!("tui.ui.overview.metric_inputs_healthy").as_ref(),
        value,
        color,
    )
}

fn share_percent(tokens: u64, total: u64) -> f64 {
    if total > 0 {
        tokens as f64 / total as f64 * 100.0
    } else {
        0.0
    }
}

/// One fun fact at a time in the right column's top box, flipping to the
/// next with a one-line vertical roll every forty ticks.
fn render_fact_box(frame: &mut Frame, app: &TuiModel, area: Rect, data: &OverviewSummary) {
    let facts = fun_facts(app, data);
    if facts.is_empty() || area.width < 6 || area.height < 2 {
        return;
    }
    let width = area.width as usize - 2;
    let index = (app.ticker_tick as usize / 40) % facts.len();
    let phase = app.ticker_tick % 40;
    let current = split_cells(&facts[index], width);
    let next = split_cells(&facts[(index + 1) % facts.len()], width);
    let (first, second) = match phase {
        38 => (current.1, next.0),
        39 => (next.0, next.1),
        _ => (current.0, current.1),
    };
    let lines = vec![
        Line::from(vec![
            Span::styled("▸ ", Style::default().fg(app.theme.chrome.focus)),
            Span::styled(first, Style::default().fg(app.theme.text.secondary)),
        ]),
        Line::from(Span::styled(
            format!("  {second}"),
            Style::default().fg(app.theme.text.secondary),
        )),
    ];
    frame.render_widget(Paragraph::new(lines), area);
}

/// Splits a fact into at most two display-cell lines (CJK counts double).
fn split_cells(text: &str, width: usize) -> (String, String) {
    let mut first = String::new();
    let mut second = String::new();
    let mut first_width = 0;
    let mut second_width = 0;
    let mut filling_second = false;
    for ch in text.chars() {
        let cell = char_width(ch);
        if !filling_second && first_width + cell <= width {
            first.push(ch);
            first_width += cell;
        } else {
            filling_second = true;
            if second_width + cell > width {
                break;
            }
            second.push(ch);
            second_width += cell;
        }
    }
    (first, second)
}

fn fun_facts(app: &TuiModel, data: &OverviewSummary) -> Vec<String> {
    let mut facts = Vec::new();
    let total = data.tokens.total();
    if total >= 1_000_000 {
        facts.push(
            rust_i18n::t!(
                "tui.ui.overview.fact_shakespeare",
                tokens = format_tokens(total),
                count = format_tokens_with_commas(total / 1_100_000)
            )
            .into_owned(),
        );
    }
    let cost = app.usage().total_cost;
    if cost >= 1.0 {
        facts.push(
            rust_i18n::t!(
                "tui.ui.overview.fact_fried_chicken",
                cost = format_cost(cost),
                count = format_tokens_with_commas((cost / 1.7) as u64)
            )
            .into_owned(),
        );
        facts.push(
            rust_i18n::t!(
                "tui.ui.overview.fact_bubble_tea",
                count = format_tokens_with_commas((cost / 3.0) as u64)
            )
            .into_owned(),
        );
    }
    if total > 0 {
        if data.cache_rate.reaches(80) {
            facts.push(
                rust_i18n::t!("tui.ui.overview.fact_cache_frugal", rate = data.cache_rate)
                    .into_owned(),
            );
        } else if !data.cache_rate.reaches(50) {
            facts.push(
                rust_i18n::t!(
                    "tui.ui.overview.fact_cache_wasteful",
                    rate = data.cache_rate
                )
                .into_owned(),
            );
        }
    }
    if data.active_days >= 7 {
        facts.push(
            rust_i18n::t!("tui.ui.overview.fact_active_days", days = data.active_days).into_owned(),
        );
    }
    if data.model_count >= 5 {
        facts.push(
            rust_i18n::t!(
                "tui.ui.overview.fact_models_harem",
                models = data.model_count,
                count = data.model_count
            )
            .into_owned(),
        );
    }
    if data.peak_daily_tokens > 0 {
        facts.push(
            rust_i18n::t!(
                "tui.ui.overview.fact_peak_day",
                tokens = format_tokens(data.peak_daily_tokens)
            )
            .into_owned(),
        );
    }
    let streak = app.usage().current_streak;
    if streak >= 3 {
        facts.push(rust_i18n::t!("tui.ui.overview.fact_streak", streak = streak).into_owned());
    }
    facts
}

fn render_divider(frame: &mut Frame, app: &TuiModel, area: Rect) {
    let divider = Line::from(Span::styled(
        "│",
        Style::default().fg(app.theme.chrome.border),
    ));
    frame.render_widget(Paragraph::new(vec![divider; area.height as usize]), area);
}

fn render_right(frame: &mut Frame, app: &TuiModel, area: Rect, data: &OverviewSummary) {
    // Roast facts sit on top so the Achievements title lines up with the
    // Core column's Fact title row.
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(2),
            Constraint::Min(0),
        ])
        .split(area);
    frame.render_widget(
        Paragraph::new(section_title(
            app,
            rust_i18n::t!("tui.ui.overview.section_roast"),
        )),
        rows[0],
    );
    render_fact_box(frame, app, rows[2], data);

    let items = achievements::build(
        app.usage().current_streak,
        data.tokens.total(),
        data.cache_rate,
        data.model_count,
        data.client_count,
    );
    let mut lines = achievements::lines(&app.theme, &items);
    lines.truncate(rows[3].height as usize);
    frame.render_widget(Paragraph::new(lines), rows[3]);
}

fn section_title(app: &TuiModel, title: Cow<'static, str>) -> Line<'static> {
    Line::from(Span::styled(
        title,
        Style::default()
            .fg(app.theme.text.primary)
            .add_modifier(Modifier::BOLD),
    ))
}

fn separator_line(app: &TuiModel, width: usize) -> Line<'static> {
    Line::from(Span::styled(
        "-".repeat(width),
        Style::default().fg(app.theme.text.secondary),
    ))
}

fn left_lines(
    app: &TuiModel,
    data: &OverviewSummary,
    width: usize,
    height: usize,
) -> Vec<Line<'static>> {
    let favorite_model = data
        .favorite_model
        .as_ref()
        .map(|favorite| favorite.id.as_str())
        .unwrap_or("—");
    let favorite_client = data
        .favorite_client
        .as_ref()
        .map(|favorite| get_client_display_name(favorite.client))
        .unwrap_or_else(|| "—".to_string());
    let favorite_width = width.saturating_sub(METRIC_LABEL_WIDTH).clamp(1, 28);

    let groups = [
        vec![
            metric_line(
                app,
                rust_i18n::t!("tui.ui.overview.metric_total_tokens").as_ref(),
                format_tokens(app.usage().total_tokens),
                app.theme.metrics.tokens,
            ),
            metric_line(
                app,
                rust_i18n::t!("tui.ui.overview.metric_peak_daily_tokens").as_ref(),
                format_tokens(data.peak_daily_tokens),
                app.theme.metrics.tokens,
            ),
        ],
        vec![
            metric_line(
                app,
                rust_i18n::t!("tui.ui.overview.metric_total_cost").as_ref(),
                format_cost(app.usage().total_cost),
                app.theme.metrics.cost,
            ),
            metric_line(
                app,
                rust_i18n::t!("tui.ui.overview.metric_peak_daily_cost").as_ref(),
                format_cost(data.peak_daily_cost),
                app.theme.metrics.cost,
            ),
        ],
        vec![
            metric_line(
                app,
                rust_i18n::t!("tui.ui.overview.metric_input_data").as_ref(),
                format_bytes(app.total_input_bytes()),
                app.theme.text.primary,
            ),
            // The narrow fallback omits input health; Active Days keeps this
            // group paired with Input Data.
            metric_line(
                app,
                rust_i18n::t!("tui.ui.overview.metric_active_days").as_ref(),
                data.active_days.to_string(),
                app.theme.metrics.total,
            ),
        ],
        vec![
            metric_line(
                app,
                rust_i18n::t!("tui.ui.overview.metric_models_eaten").as_ref(),
                data.model_count.to_string(),
                app.theme.metrics.total,
            ),
            metric_line(
                app,
                rust_i18n::t!("tui.ui.overview.metric_favorite_model").as_ref(),
                truncate(favorite_model, favorite_width),
                app.model_color(favorite_model),
            ),
        ],
        vec![
            metric_line(
                app,
                rust_i18n::t!("tui.ui.overview.metric_clients_used").as_ref(),
                data.client_count.to_string(),
                app.theme.metrics.total,
            ),
            metric_line(
                app,
                rust_i18n::t!("tui.ui.overview.metric_favorite_client").as_ref(),
                truncate(&favorite_client, favorite_width),
                data.favorite_client
                    .as_ref()
                    .map(|favorite| app.client_color(favorite.client))
                    .unwrap_or(app.theme.text.primary),
            ),
        ],
    ];

    let separator = separator_line(app, width);
    let mut separators = Vec::new();
    let mut lines = Vec::new();
    for (index, group) in groups.into_iter().enumerate() {
        if index > 0 {
            separators.push(lines.len());
            lines.push(separator.clone());
        }
        lines.extend(group);
    }
    // Overflow policy: drop separator lines bottom-up (never metric rows),
    // then clip the tail, so every metric row survives while height >= 10.
    while lines.len() > height {
        if let Some(position) = separators.pop() {
            lines.remove(position);
        } else {
            break;
        }
    }
    lines.truncate(height);
    lines
}

fn metric_line(app: &TuiModel, label: &str, value: String, color: Color) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            pad_right(label, METRIC_LABEL_WIDTH),
            Style::default().fg(app.theme.text.secondary),
        ),
        Span::styled(value, Style::default().fg(color)),
    ])
}

fn health_percentage(app: &TuiModel) -> String {
    let total = total_inputs(app);
    let health = generation_health(app);
    if total == 0 {
        "—".to_string()
    } else if health.clean_inputs == total {
        "100%".to_string()
    } else {
        format!("{:.2}%", health.clean_inputs as f64 / total as f64 * 100.0)
    }
}

fn health_color(app: &TuiModel) -> Color {
    let total = total_inputs(app);
    let health = generation_health(app);
    if total == 0 {
        app.theme.text.secondary
    } else {
        let ratio = health.clean_inputs as f64 / total as f64;
        if ratio >= 0.99 {
            app.theme.status.success
        } else if ratio >= 0.95 {
            app.theme.status.warning
        } else {
            app.theme.status.danger
        }
    }
}

fn total_inputs(app: &TuiModel) -> usize {
    let health = generation_health(app);
    health
        .clean_inputs
        .saturating_add(health.degraded_inputs)
        .saturating_add(health.partial_inputs())
        .saturating_add(health.failed_inputs())
}

fn generation_health(app: &TuiModel) -> &tokenx_engine::input_health::HealthSummary {
    app.generation_health()
        .expect("overview health requires an installed generation")
}

fn truncate(value: &str, max_chars: usize) -> String {
    truncate_with_ellipsis(value, max_chars)
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, HashSet};

    use super::*;
    use crate::tui::data::{
        DailyClientInfo, DailyModelInfo, DailyUsage, UsageProjection, UsageTokenBreakdown,
    };
    use crate::tui::model::TuiConfig;
    use chrono::NaiveDate;
    use ratatui::{backend::TestBackend, Terminal};
    use tokenx_engine::{ClientId, SessionUsage};

    fn make_app(width: u16) -> TuiModel {
        make_app_with_theme(width, "blue")
    }

    fn make_app_with_theme(width: u16, theme: &str) -> TuiModel {
        let config = TuiConfig {
            theme: Some(theme.parse().unwrap()),
            refresh: 0,
            no_refresh: false,
            client_universe: tokenx_engine::ClientUniverse::all(),
            initial_tab: None,
            effective_date: chrono::NaiveDate::from_ymd_opt(2026, 7, 26).unwrap(),
        };
        let mut app = TuiModel::new_for_test(config).unwrap();
        app.install_generation_fixture(
            tokenx_engine::FrozenUsageIndex::new(),
            Vec::new(),
            Default::default(),
        );
        app.terminal_width = width;
        app
    }

    fn install_favorite_client(app: &mut TuiModel, client: ClientId) {
        let tokens = UsageTokenBreakdown {
            input: 10_000,
            ..UsageTokenBreakdown::default()
        };
        let model = DailyModelInfo {
            provider: "openai".into(),
            model_id: "gpt-5.4".into(),
            display_name: "gpt-5.4".into(),
            workspace_key: None,
            workspace_label: None,
            tokens: tokens.clone(),
            cost: 1.0,
            messages: 1,
        };
        let client_usage = DailyClientInfo {
            tokens: tokens.clone(),
            cost: 1.0,
            models: vec![model],
        };
        let day = DailyUsage {
            date: NaiveDate::from_ymd_opt(2026, 7, 16).unwrap(),
            tokens,
            cost: 1.0,
            client_breakdown: BTreeMap::from([(client, client_usage)]),
            message_count: 1,
            turn_count: 1,
        };
        app.update_data(UsageProjection {
            daily: vec![day],
            ..UsageProjection::default()
        });
    }

    #[test]
    fn favorite_model_family_identity_uses_one_brand_color() {
        let width = 60;
        let height = 30;
        let mut app = make_app(width);
        install_favorite_client(&mut app, ClientId::Omp);
        let expected = portraits::family_color(&app, ModelFamily::Gpt);
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();

        terminal
            .draw(|frame| {
                render_fun_things(frame, &app, frame.area(), app.overview_summary());
            })
            .unwrap();

        for (role, row_text, cell_text) in [
            ("portrait", "¬", "¬"),
            ("slogan", "always come back", "always"),
            ("family name", "gpt  10K", "gpt"),
        ] {
            let (x, y) = buffer_text_cell(&terminal, row_text, cell_text);
            assert_eq!(
                terminal.backend().buffer().cell((x, y)).unwrap().fg,
                expected,
                "{role} must use the favorite model family color"
            );
        }
    }

    fn buffer_lines(terminal: &Terminal<TestBackend>) -> Vec<String> {
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
            .collect()
    }

    fn buffer_text_cell(
        terminal: &Terminal<TestBackend>,
        row_text: &str,
        cell_text: &str,
    ) -> (u16, u16) {
        let lines = buffer_lines(terminal);
        let (y, row) = lines
            .iter()
            .enumerate()
            .find(|(_, row)| row.contains(row_text))
            .unwrap_or_else(|| {
                panic!(
                    "expected row {row_text:?} should render:\n{}",
                    lines.join("\n")
                )
            });
        let x = row
            .find(cell_text)
            .expect("expected cell text should render");
        (x as u16, y as u16)
    }

    fn render_snapshot(frame: &mut Frame, app: &mut TuiModel, area: Rect) {
        let state = crate::tui::page_state::PageStates::default();
        let presentation = crate::tui::presentation::Presentation::for_view(app, &state);
        let actions = ActionSet::for_view(app, &state, presentation);
        let mut artifacts = RenderArtifacts::default();
        render(frame, app, &state, &mut artifacts, area, None, &actions);
    }

    fn line_width(line: &Line<'_>) -> usize {
        line.spans
            .iter()
            .map(|span| terminal_width(span.content.as_ref()))
            .sum()
    }

    fn line_text(line: &Line<'_>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    }

    #[test]
    fn fact_split_uses_terminal_display_width() {
        let (first, second) = split_cells("a\u{301}中b", 2);

        assert_eq!(first, "a\u{301}");
        assert_eq!(second, "中");
        assert!(terminal_width(first.as_str()) <= 2);
        assert!(terminal_width(second.as_str()) <= 2);
    }

    #[test]
    fn roast_facts_use_the_authoritative_current_streak() {
        let mut app = make_app(120);
        app.usage_mut_for_test().current_streak = 3;

        let facts = fun_facts(&app, &OverviewSummary::default());

        assert!(facts.iter().any(|fact| fact.contains("3-day streak")));
    }

    #[test]
    fn empty_fun_state_centers_its_fixed_block_inside_the_body() {
        let width = 40;
        let height = 13;
        let app = make_app(width);
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();

        terminal
            .draw(|frame| render_fun_things(frame, &app, frame.area(), &OverviewSummary::default()))
            .unwrap();

        let lines = buffer_lines(&terminal);
        assert_eq!(lines[0].trim(), "Fun");

        let body_height = height as usize - 1;
        let first_block_row = 1 + (body_height - EMPTY_FUN_THINGS_HEIGHT) / 2;
        let mut expected = portraits::lines(&app, ModelFamily::Unknown)
            .map(|line| line_text(&center_line(line, width as usize)))
            .to_vec();
        expected.push(line_text(&center_line(
            Line::from(Span::styled(
                "no data yet",
                Style::default().fg(app.theme.text.secondary),
            )),
            width as usize,
        )));

        assert!(lines[1..first_block_row]
            .iter()
            .all(|line| line.trim().is_empty()));
        for (offset, expected_line) in expected.iter().enumerate() {
            let rendered = &lines[first_block_row + offset];
            assert!(rendered.starts_with(expected_line), "{rendered:?}");
        }
        assert!(lines[first_block_row + EMPTY_FUN_THINGS_HEIGHT..]
            .iter()
            .all(|line| line.trim().is_empty()));
    }

    #[test]
    fn favorite_client_heading_uses_extra_space_without_sacrificing_content() {
        let block = || {
            vec![
                Line::default(),
                Line::from("Favorite Client"),
                Line::from("slogan"),
                Line::from("stats"),
            ]
        };

        let mut spacious = vec![Line::from("prior"); 11];
        append_favorite_client_block(&mut spacious, block(), 16);
        let spacious_tail = spacious[11..].iter().map(line_text).collect::<Vec<_>>();
        assert_eq!(
            spacious_tail,
            ["", "Favorite Client", "", "slogan", "stats"]
        );

        let mut compact = vec![Line::from("prior"); 11];
        append_favorite_client_block(&mut compact, block(), 15);
        let compact_tail = compact[11..].iter().map(line_text).collect::<Vec<_>>();
        assert_eq!(compact_tail, ["", "Favorite Client", "slogan", "stats"]);
    }

    #[test]
    fn favorite_client_slogan_and_name_share_client_color() {
        let width = 60;
        let height = 30;
        let mut app = make_app(width);
        install_favorite_client(&mut app, ClientId::Omp);
        let expected = app.client_color(ClientId::Omp);
        assert_ne!(expected, app.theme.text.primary);
        assert_ne!(expected, app.theme.visualization.artwork);
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();

        terminal
            .draw(|frame| {
                render_fun_things(frame, &app, frame.area(), app.overview_summary());
            })
            .unwrap();

        for (role, row_text, cell_text) in
            [("slogan", "Elite", "Elite"), ("client name", "OMP", "OMP")]
        {
            let (x, y) = buffer_text_cell(&terminal, row_text, cell_text);
            assert_eq!(
                terminal.backend().buffer().cell((x, y)).unwrap().fg,
                expected,
                "{role} must use the favorite client color"
            );
        }
    }

    #[test]
    fn narrow_favorite_client_metric_uses_app_client_color() {
        let width = 30;
        let height = 50;
        let mut app = make_app(width);
        install_favorite_client(&mut app, ClientId::Codex);
        let expected = app.client_color(ClientId::Codex);
        assert_ne!(expected, app.theme.text.primary);
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();

        terminal
            .draw(|frame| render_snapshot(frame, &mut app, frame.area()))
            .unwrap();

        let (x, y) = buffer_text_cell(&terminal, "Favorite Client", "Codex");
        assert_eq!(
            terminal.backend().buffer().cell((x, y)).unwrap().fg,
            expected
        );
    }

    #[test]
    fn core_facts_keep_input_health_and_data_size_as_the_last_rows() {
        let width = 60;
        let height = 20;
        let app = make_app(width);
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();

        terminal
            .draw(|frame| render_core(frame, &app, frame.area(), &OverviewSummary::default()))
            .unwrap();

        let lines = buffer_lines(&terminal);
        let expected = [
            "Active Days",
            "Sessions Scanned",
            "Cache Rate",
            "Models Eaten",
            "Clients Used",
            "Inputs Healthy",
            "Data Size",
        ];
        let rendered = lines
            .iter()
            .filter_map(|line| {
                expected
                    .iter()
                    .find(|label| line.contains(**label))
                    .copied()
            })
            .collect::<Vec<_>>();

        assert_eq!(rendered, expected);
        assert_eq!(
            &rendered[rendered.len() - 2..],
            ["Inputs Healthy", "Data Size"]
        );

        let clients_row = lines
            .iter()
            .position(|line| line.contains("Clients Used"))
            .unwrap();
        let inputs_row = lines
            .iter()
            .position(|line| line.contains("Inputs Healthy"))
            .unwrap();
        let data_size_row = lines
            .iter()
            .position(|line| line.contains("Data Size"))
            .unwrap();
        assert_eq!(inputs_row, clients_row + 2);
        assert!(lines[clients_row + 1].trim().is_empty());
        assert_eq!(data_size_row, inputs_row + 1);
    }

    #[test]
    fn core_facts_drop_the_diagnostic_gap_before_clipping_metrics() {
        let width = 60;
        let height = 13;
        let app = make_app(width);
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();

        terminal
            .draw(|frame| render_core(frame, &app, frame.area(), &OverviewSummary::default()))
            .unwrap();

        let lines = buffer_lines(&terminal);
        let clients_row = lines
            .iter()
            .position(|line| line.contains("Clients Used"))
            .unwrap();
        let inputs_row = lines
            .iter()
            .position(|line| line.contains("Inputs Healthy"))
            .unwrap();
        let data_size_row = lines
            .iter()
            .position(|line| line.contains("Data Size"))
            .unwrap();

        assert_eq!(inputs_row, clients_row + 1);
        assert_eq!(data_size_row, inputs_row + 1);
    }

    #[test]
    fn overview_render_clears_the_replaced_dashboard_before_drawing_snapshot() {
        let width = 120;
        let height = 30;
        let mut app = make_app(width);
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();

        terminal
            .draw(|frame| {
                let area = frame.area();
                let stale = vec![Line::from("X".repeat(width as usize)); height as usize];
                frame.render_widget(Paragraph::new(stale), area);
                render_snapshot(frame, &mut app, area);
            })
            .unwrap();

        let screen = buffer_lines(&terminal).join("\n");
        assert!(screen.contains("Snapshot"));
        assert!(!screen.contains('X'), "stale dashboard symbols remained");
        assert!(!screen.contains("Token Profile"));
        assert!(!screen.contains("Agent profiles"));
        assert!(screen.contains("Active Days"));
    }

    #[test]
    fn wide_snapshot_separates_sections_with_vertical_dividers() {
        let width = 120;
        let height = 30;
        let mut app = make_app(width);
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();

        terminal
            .draw(|frame| render_snapshot(frame, &mut app, frame.area()))
            .unwrap();

        let lines = buffer_lines(&terminal);
        let metric_row = lines
            .iter()
            .find(|line| line.contains("Data Size"))
            .expect("snapshot metric row should render");
        assert_eq!(metric_row.matches('│').count(), 4, "{metric_row}");
        // The Fact block lives in the middle column, right of the second divider.
        let dividers: Vec<usize> = metric_row
            .char_indices()
            .filter(|(_, glyph)| *glyph == '│')
            .map(|(index, _)| index)
            .collect();
        let metric_offset = metric_row
            .find("Data Size")
            .expect("metric label should render");
        assert!(metric_offset > dividers[1], "{metric_row}");
    }

    #[test]
    fn two_column_snapshot_shows_fun_things_and_core_without_donut() {
        let width = 90;
        let height = 30;
        let mut app = make_app(width);
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();

        terminal
            .draw(|frame| render_snapshot(frame, &mut app, frame.area()))
            .unwrap();

        let lines = buffer_lines(&terminal);
        let screen = lines.join("\n");
        assert!(screen.contains("Fun"), "{screen}");
        assert!(screen.contains("Core"), "{screen}");
        assert!(
            !screen.contains("Achievements"),
            "two columns drop the ladder column: {screen}"
        );
        assert!(
            !screen.contains("total"),
            "the donut center label must be gone: {screen}"
        );
        let core_row = lines
            .iter()
            .find(|line| line.contains("Data Size"))
            .expect("core metric row should render");
        assert_eq!(core_row.matches('│').count(), 3, "{core_row}");
    }

    #[test]
    fn one_column_snapshot_shows_only_core() {
        let width = 60;
        let height = 30;
        let mut app = make_app(width);
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();

        terminal
            .draw(|frame| render_snapshot(frame, &mut app, frame.area()))
            .unwrap();

        let lines = buffer_lines(&terminal);
        let screen = lines.join("\n");
        assert!(screen.contains("Core"), "{screen}");
        assert!(screen.contains("Data Size"), "{screen}");
        assert!(!screen.contains("Fun"), "{screen}");
        let core_row = lines
            .iter()
            .find(|line| line.contains("Data Size"))
            .expect("core metric row should render");
        assert_eq!(core_row.matches('│').count(), 2, "{core_row}");
    }

    #[test]
    fn snapshot_session_count_follows_the_selected_clients() {
        let universe =
            tokenx_engine::ClientUniverse::new([ClientId::Claude, ClientId::Codex]).unwrap();
        let config = TuiConfig {
            theme: Some(crate::theme::ThemeName::Blue),
            refresh: 0,
            no_refresh: false,
            client_universe: universe,
            initial_tab: None,
            effective_date: chrono::NaiveDate::from_ymd_opt(2026, 7, 26).unwrap(),
        };
        let mut app = TuiModel::new_for_test(config).unwrap();
        app.terminal_width = 60;
        let main_session = |client: ClientId, session_id: &str| SessionUsage {
            is_main_session: true,
            ..SessionUsage::new(client, session_id)
        };
        app.install_generation_fixture(
            tokenx_engine::FrozenUsageIndex::new(),
            vec![
                main_session(ClientId::Claude, "claude-main"),
                main_session(ClientId::Codex, "codex-main-1"),
                main_session(ClientId::Codex, "codex-main-2"),
            ],
            Default::default(),
        );

        assert_eq!(app.session_snapshot().client_summaries().len(), 2);
        assert_eq!(app.overview_summary().main_session_count, 3);

        app.set_selected_clients_for_test(HashSet::from([ClientId::Claude]));
        app.update_data(crate::tui::data::UsageProjection::default());

        assert_eq!(app.session_snapshot().client_summaries().len(), 2);
        assert_eq!(app.overview_summary().main_session_count, 1);

        let mut terminal = Terminal::new(TestBackend::new(60, 20)).unwrap();
        terminal
            .draw(|frame| render_core(frame, &app, frame.area(), app.overview_summary()))
            .unwrap();
        let sessions_row = buffer_lines(&terminal)
            .into_iter()
            .find(|line| line.contains("Sessions"))
            .expect("Core should render its Sessions metric");
        assert_eq!(sessions_row.split_whitespace().last(), Some("1"));
    }

    #[test]
    fn narrow_snapshot_keeps_the_stacked_text_fallback() {
        let width = 30;
        let height = 50;
        let mut app = make_app(width);
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();

        terminal
            .draw(|frame| render_snapshot(frame, &mut app, frame.area()))
            .unwrap();

        let lines = buffer_lines(&terminal);
        let screen = lines.join("\n");
        assert!(screen.contains("Total Token"), "{screen}");
        assert!(
            !screen.contains("Inputs Healthy"),
            "text fallback drops the Inputs Healthy fact: {screen}"
        );
        assert!(
            !screen.contains('●'),
            "text fallback has no donut glyphs: {screen}"
        );
        let metric_row = lines
            .iter()
            .find(|line| line.contains("Total Token"))
            .expect("snapshot metric row should render");
        assert_eq!(metric_row.matches('│').count(), 2, "{metric_row}");
    }

    #[test]
    fn inputs_healthy_fact_is_compact_when_all_inputs_are_clean() {
        let width = 200;
        let height = 50;
        let mut app = make_app_with_theme(width, "dusk");
        app.install_generation_fixture_with_health(
            tokenx_engine::FrozenUsageIndex::new(),
            Vec::new(),
            Default::default(),
            tokenx_engine::input_health::HealthSummary {
                clean_inputs: 100,
                ..Default::default()
            },
        );
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();

        terminal
            .draw(|frame| render_snapshot(frame, &mut app, frame.area()))
            .unwrap();

        let screen = buffer_lines(&terminal).join("\n");
        assert!(screen.contains("Inputs Healthy"), "{screen}");
        assert!(screen.contains("✓ 100 clean"), "{screen}");
        assert!(
            !screen.contains("Degraded"),
            "clean inputs earn the one-liner, not the legend: {screen}"
        );
    }

    #[test]
    fn left_metrics_pair_active_days_with_input_data() {
        let app = make_app(120);
        let data = OverviewSummary::default();
        let lines = left_lines(&app, &data, 54, 14);
        let text = lines.iter().map(line_text).collect::<Vec<_>>();

        assert_eq!(text.len(), 14);
        for index in [2, 5, 8, 11] {
            assert_eq!(text[index], "-".repeat(54), "separator at {index}");
        }
        assert!(text[0].starts_with("Total Token"));
        assert!(text[6].starts_with("Input Data"));
        assert!(text[7].starts_with("Active Days"));
        assert!(text[9].starts_with("Models Eaten"));
        assert!(text[10].starts_with("Favorite Model"));
        assert!(text[12].starts_with("Clients Used"));
        assert!(text[13].starts_with("Favorite Client"));
        assert!(
            text.iter().all(|line| !line.contains("Inputs Healthy")),
            "the narrow fallback omits the input-health fact"
        );
    }

    #[test]
    fn left_metrics_drop_separators_before_metric_rows_when_space_is_tight() {
        let app = make_app(120);
        let data = OverviewSummary::default();

        let lines = left_lines(&app, &data, 54, 12);
        let text = lines.iter().map(line_text).collect::<Vec<_>>();
        assert_eq!(text.len(), 12);
        assert_eq!(text[2], "-".repeat(54));
        assert_eq!(text[5], "-".repeat(54));
        assert_eq!(
            text.iter().filter(|line| line.starts_with('-')).count(),
            2,
            "separators should be dropped bottom-up first"
        );
        assert!(text.iter().any(|line| line.starts_with("Total Token")));
        assert!(text.iter().any(|line| line.starts_with("Favorite Client")));

        let lines = left_lines(&app, &data, 54, 10);
        let text = lines.iter().map(line_text).collect::<Vec<_>>();
        assert_eq!(text.len(), 10);
        assert!(text.iter().all(|line| !line.starts_with('-')));
        assert!(text.last().unwrap().starts_with("Favorite Client"));

        let lines = left_lines(&app, &data, 54, 8);
        let text = lines.iter().map(line_text).collect::<Vec<_>>();
        assert_eq!(text.len(), 8);
        assert!(text[0].starts_with("Total Token"));
        assert!(text.iter().any(|line| line.starts_with("Active Days")));
        assert!(text.iter().all(|line| !line.contains("Favorite Client")));
    }

    #[test]
    fn wide_snapshot_lines_fit_their_columns() {
        let app = make_app(120);
        let data = OverviewSummary::default();

        assert!(left_lines(&app, &data, 29, 16)
            .iter()
            .all(|line| line_width(line) <= 29));
    }

    #[test]
    fn snapshot_renders_without_panicking_across_layouts() {
        for (width, height) in [
            (120, 30),
            (200, 50),
            (100, 24),
            (90, 24),
            (60, 24),
            (30, 24),
        ] {
            let mut app = make_app(width);
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal
                .draw(|frame| render_snapshot(frame, &mut app, frame.area()))
                .unwrap();
        }
    }

    #[test]
    fn wide_snapshots_show_all_four_section_titles() {
        for (width, height) in [(120, 30), (200, 50)] {
            let mut app = make_app(width);
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal
                .draw(|frame| render_snapshot(frame, &mut app, frame.area()))
                .unwrap();

            let screen = buffer_lines(&terminal).join("\n");
            for title in ["Fun", "Core", "Roast", "Achievements"] {
                assert!(screen.contains(title), "missing {title}: {screen}");
            }
            assert!(screen.contains("[■_■]"), "fallback portrait: {screen}");
        }
    }
}
