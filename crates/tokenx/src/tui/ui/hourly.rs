use chrono::{NaiveDate, NaiveDateTime};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Cell, Row, Scrollbar, ScrollbarOrientation, Table};
use std::borrow::Cow;

use super::empty_state;
use super::hourly_profile;
use super::table_layout::{
    display_width, distributed_table_area, responsive_table_layout, width_for_column,
    ResponsiveColumn, DISTRIBUTED_TABLE_FLEX, TABLE_COLUMN_SPACING,
};
use super::widgets::{
    format_cache_hit_rate, format_cost, format_cost_per_million, format_tokens,
    get_client_display_name, total_tokens_cell, truncate_display_width, viewport_scrollbar_state,
};
use crate::date_display::format_numeric_month_day;
use crate::tui::actions::ActionSet;
use crate::tui::model::{HourlyViewMode, SortDirection, SortField, TuiModel};
use crate::tui::page_state::PageStates;
use crate::tui::presentation::EmptySubject;
use crate::tui::render_artifacts::RenderArtifacts;
use tokenx_engine::ClientId;

const HOUR_WIDTH: u16 = 7;
const CLIENT_MIN_WIDTH: u16 = 8;
const CLIENT_MAX_WIDTH: u16 = 40;
const TURN_WIDTH: u16 = 6;
const MSGS_WIDTH: u16 = 6;
const NUMERIC_WIDTH: u16 = 10;
const TOTAL_WIDTH: u16 = 9;
const CACHE_RATE_WIDTH: u16 = 8;
const COST_WIDTH: u16 = 10;
const COST_PER_MILLION_WIDTH: u16 = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HourlyColumn {
    Hour,
    Client,
    Turn,
    Messages,
    Input,
    Output,
    CacheRead,
    CacheWrite,
    CacheRate,
    Total,
    Cost,
    CostPerMillion,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct HourlyTableLayout {
    columns: Vec<HourlyColumn>,
    widths: Vec<Constraint>,
}

impl HourlyTableLayout {
    fn width_for(&self, column: HourlyColumn) -> usize {
        width_for_column(&self.columns, &self.widths, column)
    }
}

pub fn render(
    frame: &mut Frame,
    app: &TuiModel,
    state: &PageStates,
    artifacts: &mut RenderArtifacts,
    area: Rect,
    empty: Option<EmptySubject>,
    actions: &ActionSet,
) {
    match state.hourly_mode() {
        HourlyViewMode::Table => render_table(frame, app, artifacts, area, empty, actions),
        HourlyViewMode::Profile => {
            hourly_profile::render(frame, app, state, artifacts, area, empty, actions)
        }
    }
}

fn hourly_column_order(column: HourlyColumn) -> u16 {
    match column {
        HourlyColumn::Hour => 0,
        HourlyColumn::Client => 10,
        HourlyColumn::Turn => 20,
        HourlyColumn::Messages => 30,
        HourlyColumn::Input => 40,
        HourlyColumn::Output => 50,
        HourlyColumn::CacheRead => 60,
        HourlyColumn::CacheWrite => 70,
        HourlyColumn::CacheRate => 80,
        HourlyColumn::Total => 90,
        HourlyColumn::Cost => 100,
        HourlyColumn::CostPerMillion => 110,
    }
}

fn hourly_table_layout(
    table_width: u16,
    has_turn_data: bool,
    client_content_width: u16,
) -> HourlyTableLayout {
    let mut columns = vec![
        ResponsiveColumn::fixed_required(
            HourlyColumn::Hour,
            hourly_column_order(HourlyColumn::Hour),
            HOUR_WIDTH,
        ),
        ResponsiveColumn::fixed_required(
            HourlyColumn::Total,
            hourly_column_order(HourlyColumn::Total),
            TOTAL_WIDTH,
        ),
        ResponsiveColumn::fixed_optional(
            HourlyColumn::Cost,
            10,
            hourly_column_order(HourlyColumn::Cost),
            COST_WIDTH,
        ),
        ResponsiveColumn::fixed_optional(
            HourlyColumn::Messages,
            40,
            hourly_column_order(HourlyColumn::Messages),
            MSGS_WIDTH,
        ),
        ResponsiveColumn::measured_atomic_optional(
            HourlyColumn::Client,
            20,
            hourly_column_order(HourlyColumn::Client),
            CLIENT_MIN_WIDTH,
            client_content_width,
            CLIENT_MAX_WIDTH,
        ),
        ResponsiveColumn::fixed_optional(
            HourlyColumn::Input,
            50,
            hourly_column_order(HourlyColumn::Input),
            NUMERIC_WIDTH,
        ),
        ResponsiveColumn::fixed_optional(
            HourlyColumn::Output,
            60,
            hourly_column_order(HourlyColumn::Output),
            NUMERIC_WIDTH,
        ),
        ResponsiveColumn::fixed_optional(
            HourlyColumn::CacheRead,
            70,
            hourly_column_order(HourlyColumn::CacheRead),
            NUMERIC_WIDTH,
        ),
        ResponsiveColumn::fixed_optional(
            HourlyColumn::CacheWrite,
            80,
            hourly_column_order(HourlyColumn::CacheWrite),
            NUMERIC_WIDTH,
        ),
        ResponsiveColumn::fixed_optional(
            HourlyColumn::CacheRate,
            90,
            hourly_column_order(HourlyColumn::CacheRate),
            CACHE_RATE_WIDTH,
        ),
        ResponsiveColumn::fixed_optional(
            HourlyColumn::CostPerMillion,
            100,
            hourly_column_order(HourlyColumn::CostPerMillion),
            COST_PER_MILLION_WIDTH,
        ),
    ];

    if has_turn_data {
        columns.push(ResponsiveColumn::fixed_optional(
            HourlyColumn::Turn,
            30,
            hourly_column_order(HourlyColumn::Turn),
            TURN_WIDTH,
        ));
    }

    let layout = responsive_table_layout(table_width, &columns);

    HourlyTableLayout {
        columns: layout.columns,
        widths: layout.widths,
    }
}

fn hourly_column_header(column: HourlyColumn) -> Cow<'static, str> {
    match column {
        HourlyColumn::Hour => rust_i18n::t!("tui.ui.hourly.header.hour"),
        HourlyColumn::Client => rust_i18n::t!("tui.ui.hourly.header.client"),
        HourlyColumn::Turn => rust_i18n::t!("tui.ui.hourly.header.turn"),
        HourlyColumn::Messages => rust_i18n::t!("tui.ui.hourly.header.messages"),
        HourlyColumn::Input => rust_i18n::t!("tui.ui.hourly.header.input"),
        HourlyColumn::Output => rust_i18n::t!("tui.ui.hourly.header.output"),
        HourlyColumn::CacheRead => rust_i18n::t!("tui.ui.hourly.header.cache_read"),
        HourlyColumn::CacheWrite => rust_i18n::t!("tui.ui.hourly.header.cache_write"),
        HourlyColumn::CacheRate => rust_i18n::t!("tui.ui.hourly.header.cache_rate"),
        HourlyColumn::Total => rust_i18n::t!("tui.ui.hourly.header.total"),
        HourlyColumn::Cost => rust_i18n::t!("tui.ui.hourly.header.cost"),
        HourlyColumn::CostPerMillion => rust_i18n::t!("tui.ui.hourly.header.cost_per_million"),
    }
}

fn hourly_column_sort_field(column: HourlyColumn) -> Option<SortField> {
    match column {
        HourlyColumn::Hour => Some(SortField::Date),
        HourlyColumn::Total => Some(SortField::Tokens),
        HourlyColumn::Cost => Some(SortField::Cost),
        HourlyColumn::CostPerMillion => None,
        _ => None,
    }
}

fn format_hour_label(datetime: NaiveDateTime) -> String {
    datetime.format("%H:00").to_string()
}

fn format_date_separator(date: NaiveDate) -> String {
    format_numeric_month_day(date)
}

fn render_table(
    frame: &mut Frame,
    app: &TuiModel,
    artifacts: &mut RenderArtifacts,
    area: Rect,
    empty: Option<EmptySubject>,
    actions: &ActionSet,
) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(app.theme.chrome.border))
        .title(Span::styled(
            rust_i18n::t!("tui.ui.hourly.title"),
            Style::default()
                .fg(app.theme.chrome.heading)
                .add_modifier(Modifier::BOLD),
        ))
        .style(app.theme.panel_style());

    let inner = block.inner(area);
    let table_area = distributed_table_area(inner);
    frame.render_widget(block, area);

    let visible_height = inner.height.saturating_sub(1) as usize;
    let interaction = artifacts.measure_main_list(
        app.list_interaction_for_render(),
        visible_height,
        app.current_list_len(),
    );
    if empty_state::render_if(frame, app, inner, empty, actions) {
        return;
    }

    let hourly_order = app.hourly_render_order();

    let ordered_hourly = || {
        hourly_order.iter().map(|index| {
            app.usage()
                .hourly
                .get(*index)
                .expect("cached hourly order must reference the current projection")
        })
    };
    let has_turn_data = ordered_hourly().any(|hour| hour.turn_count > 0);
    let client_content_width = ordered_hourly()
        .map(|hour| display_width(&hourly_client_text(hour.clients.iter())))
        .max()
        .unwrap_or(0);
    let sort_field = app.sort_field;
    let sort_direction = app.sort_direction;
    let scroll_offset = interaction.scroll;
    let selected_index = interaction.selected;
    let theme_heading = app.theme.chrome.heading;
    let theme_selection_style = app.theme.selection_style();
    let metric_input_style = app.theme.metric_input_style();
    let metric_output_style = app.theme.metric_output_style();
    let metric_cache_read_style = app.theme.metric_cache_read_style();
    let metric_cache_write_style = app.theme.metric_cache_write_style();
    let current_row_style = app.theme.current_row_style();
    let striped_row_style = app.theme.striped_row_style();
    let current_hour = app.current_calendar_hour();
    let table_layout = hourly_table_layout(table_area.width, has_turn_data, client_content_width);
    let columns = table_layout.columns.clone();

    let sort_indicator = |field: SortField| -> &'static str {
        if sort_field == field {
            match sort_direction {
                SortDirection::Ascending => " ▲",
                SortDirection::Descending => " ▼",
            }
        } else {
            ""
        }
    };

    let header = Row::new(
        columns
            .iter()
            .map(|column| {
                let h = hourly_column_header(*column);
                let indicator = hourly_column_sort_field(*column)
                    .map(sort_indicator)
                    .unwrap_or("");
                Cell::from(format!("{}{}", h, indicator))
            })
            .collect::<Vec<_>>(),
    )
    .style(
        Style::default()
            .fg(theme_heading)
            .add_modifier(Modifier::BOLD),
    )
    .height(1);

    let hourly_len = hourly_order.len();
    let start = scroll_offset.min(hourly_len);

    if start >= hourly_len {
        return;
    }

    let separator_style = Style::default()
        .fg(theme_heading)
        .bg(app.theme.surface.row_alt)
        .add_modifier(Modifier::BOLD);

    let mut rows: Vec<Row> = Vec::with_capacity(visible_height.saturating_add(1));
    let mut lines_used = 0usize;
    let mut prev_date: Option<NaiveDate> = None;
    let mut data_idx = start;

    while data_idx < hourly_len && lines_used < visible_height {
        let hour = &app.usage().hourly[hourly_order[data_idx]];
        let row_date = hour.datetime.date();

        if prev_date != Some(row_date) && lines_used + 1 < visible_height {
            let mut separator_cells = Vec::with_capacity(columns.len());
            separator_cells.push(Cell::from(format_date_separator(row_date)));
            separator_cells.extend((1..columns.len()).map(|_| Cell::from("")));
            rows.push(Row::new(separator_cells).style(separator_style).height(1));
            lines_used += 1;
        }
        prev_date = Some(row_date);

        let idx = data_idx;
        let is_selected = idx == selected_index;
        let is_striped = idx % 2 == 1;
        let is_current = hour.datetime == current_hour;

        let clients_str = hourly_client_text(hour.clients.iter());
        let hour_label = format_hour_label(hour.datetime);
        let hour_style = if is_current {
            Style::default()
                .fg(app.theme.chrome.current)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().add_modifier(Modifier::BOLD)
        };
        let turn_str = if hour.turn_count > 0 {
            hour.turn_count.to_string()
        } else {
            "\u{2014}".to_string()
        };

        let cell_for_column = |column: HourlyColumn| -> Cell {
            match column {
                HourlyColumn::Hour => Cell::from(hour_label.clone()).style(hour_style),
                HourlyColumn::Client => Cell::from(truncate_display_width(
                    &clients_str,
                    table_layout.width_for(HourlyColumn::Client),
                )),
                HourlyColumn::Turn => Cell::from(turn_str.clone()),
                HourlyColumn::Messages => Cell::from(hour.message_count.to_string()),
                HourlyColumn::Input => {
                    Cell::from(format_tokens(hour.tokens.input)).style(metric_input_style)
                }
                HourlyColumn::Output => Cell::from(format_tokens(hour.tokens.displayed_output()))
                    .style(metric_output_style),
                HourlyColumn::CacheRead => {
                    Cell::from(format_tokens(hour.tokens.cache_read)).style(metric_cache_read_style)
                }
                HourlyColumn::CacheWrite => Cell::from(format_tokens(hour.tokens.cache_write))
                    .style(metric_cache_write_style),
                HourlyColumn::CacheRate => Cell::from(format_cache_hit_rate(
                    hour.tokens.cache_read,
                    hour.tokens.input,
                    hour.tokens.cache_write,
                ))
                .style(Style::default().fg(app.theme.metrics.rate)),
                HourlyColumn::Total => total_tokens_cell(hour.tokens.total(), &app.theme),
                HourlyColumn::Cost => Cell::from(format_cost(hour.cost))
                    .style(Style::default().fg(app.theme.metrics.cost)),
                HourlyColumn::CostPerMillion => {
                    Cell::from(format_cost_per_million(hour.cost, hour.tokens.total()))
                        .style(Style::default().fg(app.theme.metrics.secondary_cost))
                }
            }
        };
        let cells: Vec<Cell> = columns
            .iter()
            .map(|column| cell_for_column(*column))
            .collect();

        let row_style = if is_selected {
            theme_selection_style
        } else if is_current {
            current_row_style
        } else if is_striped {
            striped_row_style
        } else {
            Style::default()
        };

        rows.push(Row::new(cells).style(row_style).height(1));
        lines_used += 1;
        data_idx += 1;
    }

    let data_rows_shown = data_idx - start;
    let measured = artifacts.measure_main_list(
        app.list_interaction_for_render(),
        data_rows_shown.max(1),
        app.current_list_len(),
    );

    let widths = table_layout.widths;

    let table = Table::new(rows, widths)
        .header(header)
        .column_spacing(TABLE_COLUMN_SPACING)
        .flex(DISTRIBUTED_TABLE_FLEX)
        .row_highlight_style(theme_selection_style);

    frame.render_widget(table, table_area);

    if hourly_len > visible_height {
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(Some("▲"))
            .end_symbol(Some("▼"));

        let mut scrollbar_state =
            viewport_scrollbar_state(hourly_len, measured.scroll, data_rows_shown.max(1));

        frame.render_stateful_widget(
            scrollbar,
            area.inner(Margin {
                horizontal: 0,
                vertical: 1,
            }),
            &mut scrollbar_state,
        );
    }
}

fn hourly_client_text<'a>(clients: impl Iterator<Item = &'a ClientId>) -> String {
    let mut labels: Vec<String> = clients
        .map(|client| get_client_display_name(*client))
        .collect();
    labels.sort();
    labels.join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::data::{HourlyModelInfo, HourlyUsage, UsageTokenBreakdown};
    use crate::tui::model::{Tab, TuiConfig};
    use ratatui::{backend::TestBackend, Terminal};
    use std::collections::BTreeSet;

    fn length_at(widths: &[Constraint], index: usize) -> u16 {
        match widths[index] {
            Constraint::Length(width) => width,
            other => panic!("expected Length at index {index}, got {other:?}"),
        }
    }

    fn hour(date: NaiveDate, h: u32) -> HourlyUsage {
        let mut clients = BTreeSet::new();
        clients.insert(ClientId::Claude);
        HourlyUsage {
            datetime: date.and_hms_opt(h, 0, 0).unwrap(),
            tokens: UsageTokenBreakdown::default(),
            cost: 1.0,
            clients,
            models: Vec::new(),
            message_count: 5,
            turn_count: 2,
        }
    }

    fn make_hourly_app(width: u16) -> TuiModel {
        let config = TuiConfig {
            theme: Some(crate::theme::ThemeName::Blue),
            refresh: 0,
            no_refresh: false,
            client_universe: tokenx_engine::ClientUniverse::all(),
            initial_tab: None,
            effective_date: chrono::NaiveDate::from_ymd_opt(2026, 7, 26).unwrap(),
        };
        let mut app = TuiModel::new_for_test(config).unwrap();
        app.terminal_width = width;
        app.current_tab = Tab::Hourly;
        app.sort_field = SortField::Date;
        app.sort_direction = SortDirection::Descending;
        let newer = NaiveDate::from_ymd_opt(2026, 5, 29).unwrap();
        let older = NaiveDate::from_ymd_opt(2026, 5, 28).unwrap();
        app.usage_mut_for_test().hourly = vec![
            hour(newer, 14),
            hour(newer, 13),
            hour(newer, 12),
            hour(older, 23),
            hour(older, 22),
        ];
        app
    }

    fn render_lines(app: &mut TuiModel, width: u16, height: u16) -> Vec<String> {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut state = crate::tui::page_state::PageStates::default();
        let presentation = crate::tui::presentation::Presentation::for_view(app, &state);
        let actions = ActionSet::for_view(app, &state, presentation);
        let mut artifacts = RenderArtifacts::default();
        terminal
            .draw(|frame| {
                render(
                    frame,
                    app,
                    &state,
                    &mut artifacts,
                    Rect::new(0, 0, width, height),
                    None,
                    &actions,
                )
            })
            .unwrap();
        app.install_render_measurements(&artifacts);
        state.install_render_measurements(&artifacts);
        terminal
            .backend()
            .buffer()
            .content()
            .chunks(width as usize)
            .map(|row| {
                row.iter()
                    .map(|cell| cell.symbol().to_string())
                    .collect::<String>()
            })
            .collect()
    }

    #[test]
    fn tight_hourly_layout_keeps_hour_and_total() {
        let layout = hourly_table_layout(21, false, 40);

        assert_eq!(
            layout.columns,
            vec![HourlyColumn::Hour, HourlyColumn::Total]
        );
        assert_eq!(length_at(&layout.widths, 0), HOUR_WIDTH);
    }

    #[test]
    fn hourly_layout_adds_cost_before_secondary_columns() {
        let layout = hourly_table_layout(32, false, 40);

        assert_eq!(
            layout.columns,
            vec![HourlyColumn::Hour, HourlyColumn::Total, HourlyColumn::Cost]
        );
    }

    #[test]
    fn hourly_layout_stops_at_wide_client_after_cost() {
        let layout = hourly_table_layout(44, false, 40);

        assert!(layout.columns.contains(&HourlyColumn::Cost));
        assert!(!layout.columns.contains(&HourlyColumn::Client));
        assert!(!layout.columns.contains(&HourlyColumn::Messages));
    }

    #[test]
    fn hourly_layout_does_not_skip_client_to_show_turn_or_messages() {
        let layout = hourly_table_layout(45, true, 40);

        assert_eq!(
            layout.columns,
            vec![HourlyColumn::Hour, HourlyColumn::Total, HourlyColumn::Cost]
        );
        assert!(!layout.columns.contains(&HourlyColumn::Client));
        assert!(!layout.columns.contains(&HourlyColumn::Turn));
        assert!(!layout.columns.contains(&HourlyColumn::Messages));
    }

    #[test]
    fn hourly_layout_adds_secondary_columns_before_total_and_cost() {
        let layout = hourly_table_layout(72, true, 20);

        assert_eq!(layout.columns[0], HourlyColumn::Hour);
        assert!(layout.columns.contains(&HourlyColumn::Client));
        assert!(layout.columns.contains(&HourlyColumn::Turn));
        assert_eq!(
            layout.columns[layout.columns.len() - 2],
            HourlyColumn::Total
        );
        assert_eq!(layout.columns[layout.columns.len() - 1], HourlyColumn::Cost);
    }

    #[test]
    fn hourly_layout_uses_measured_client_width_when_selected() {
        let layout = hourly_table_layout(100, true, 16);
        let client_index = layout
            .columns
            .iter()
            .position(|column| *column == HourlyColumn::Client)
            .expect("client column should fit");

        assert!(length_at(&layout.widths, client_index) > CLIENT_MIN_WIDTH);
        assert!(length_at(&layout.widths, client_index) <= 16);
    }

    #[test]
    fn hourly_label_omits_year() {
        let datetime =
            NaiveDateTime::parse_from_str("2026-03-02 18:00:00", "%Y-%m-%d %H:%M:%S").unwrap();

        assert_eq!(format_hour_label(datetime), "18:00");
    }

    #[test]
    fn header_labels_translate_to_english_and_chinese() {
        assert_eq!(
            rust_i18n::t!("tui.ui.hourly.header.hour", locale = "en"),
            "Hour"
        );
        assert_eq!(
            rust_i18n::t!("tui.ui.hourly.header.hour", locale = "zh-CN"),
            "小时"
        );
        assert_eq!(
            rust_i18n::t!("tui.ui.hourly.title", locale = "en"),
            " Hourly Usage "
        );
        assert_eq!(
            rust_i18n::t!("tui.ui.hourly.title", locale = "zh-CN"),
            " 每小时用量 "
        );
    }

    #[test]
    fn date_separator_uses_month_slash_day() {
        let date = NaiveDate::from_ymd_opt(2026, 3, 2).unwrap();

        assert_eq!(format_date_separator(date), "03/02");
    }

    #[test]
    fn compact_time_with_day_separators() {
        let mut app = make_hourly_app(120);
        let body = render_lines(&mut app, 120, 20).join("\n");

        assert!(body.contains("14:00"), "expected HH:00 bucket\n{body}");
        assert!(
            !body.contains("05-29 14:00"),
            "date must not repeat on every hourly row\n{body}"
        );
        assert!(body.contains("05/29"), "expected 05/29 separator\n{body}");
        assert!(body.contains("05/28"), "expected 05/28 separator\n{body}");
    }

    #[test]
    fn rendered_output_includes_reasoning_once() {
        let mut app = make_hourly_app(180);
        app.usage_mut_for_test().hourly.truncate(1);
        app.usage_mut_for_test().hourly[0].tokens = UsageTokenBreakdown {
            input: 100,
            output: 25,
            cache_read: 10,
            cache_write: 5,
            reasoning: 25,
        };

        let body = render_lines(&mut app, 180, 8).join("\n");

        assert!(
            body.contains("50"),
            "Output should include reasoning\n{body}"
        );
        assert!(
            body.contains("165"),
            "Total should count every bucket once\n{body}"
        );
    }

    #[test]
    fn selected_row_visible_in_single_line_viewport() {
        let mut app = make_hourly_app(120);
        app.set_scroll_offset(3);
        app.set_selected_index(3);

        let body = render_lines(&mut app, 120, 4).join("\n");

        assert!(
            body.contains("23:00"),
            "selected row must stay visible when its date separator cannot fit\n{body}"
        );
    }

    #[test]
    fn window_never_overflows_height_and_reports_data_rows() {
        let mut app = make_hourly_app(120);
        let height = 6u16;
        let lines = render_lines(&mut app, 120, height);

        assert_eq!(lines.len(), height as usize);
        assert!(app.max_visible_items() >= 1);
        assert!(app.max_visible_items() <= (height as usize).saturating_sub(3));
    }

    fn hourly_model(provider: &str, model_id: &str, tokens: u64) -> HourlyModelInfo {
        HourlyModelInfo {
            provider: provider.into(),
            model_id: model_id.into(),
            display_name: model_id.into(),
            tokens: UsageTokenBreakdown {
                input: tokens,
                ..UsageTokenBreakdown::default()
            },
            cost: 1.0,
        }
    }

    fn grouped_hour(models: Vec<(&str, HourlyModelInfo)>) -> HourlyUsage {
        let mut entry = hour(NaiveDate::from_ymd_opt(2026, 5, 29).unwrap(), 14);
        entry.tokens = UsageTokenBreakdown {
            input: 100,
            ..UsageTokenBreakdown::default()
        };
        entry.models = models.into_iter().map(|(_, model)| model).collect();
        entry
    }

    #[test]
    fn hourly_table_and_profile_are_grouping_invariant() {
        // The Hourly table and Profile read only hour-level totals (ADR
        // 0026): re-keying the per-group model buckets must not change the
        // rendered output.
        let projections = [
            // GroupBy::Model: one merged bucket per model.
            grouped_hour(vec![("gpt-5", hourly_model("openai", "gpt-5", 100))]),
            // GroupBy::ClientModel: bucketed per client.
            grouped_hour(vec![
                ("v1|claude|gpt-5", hourly_model("openai", "gpt-5", 60)),
                ("v1|codex|gpt-5", hourly_model("openai", "gpt-5", 40)),
            ]),
            // GroupBy::ClientProviderModel: bucketed per client+provider.
            grouped_hour(vec![
                (
                    "v1|claude|openai|gpt-5",
                    hourly_model("openai", "gpt-5", 60),
                ),
                ("v1|claude|azure|gpt-5", hourly_model("azure", "gpt-5", 40)),
            ]),
            // GroupBy::WorkspaceModel: bucketed per workspace.
            grouped_hour(vec![
                ("v1|claude|ws-a|gpt-5", hourly_model("openai", "gpt-5", 60)),
                ("v1|claude|ws-b|gpt-5", hourly_model("openai", "gpt-5", 40)),
            ]),
        ];

        let mut table_outputs = Vec::new();
        let mut profile_outputs = Vec::new();
        for projection in projections {
            let mut app = make_hourly_app(120);
            app.usage_mut_for_test().hourly = vec![projection];
            app.usage_mut_for_test().total_tokens = 100;
            app.usage_mut_for_test().total_cost = 1.0;
            table_outputs.push(render_lines(&mut app, 120, 20).join("\n"));
            profile_outputs.push(
                hourly_profile::build_hourly_profile_lines(&app, 120)
                    .unwrap()
                    .iter()
                    .map(|line| {
                        line.spans
                            .iter()
                            .map(|span| span.content.as_ref())
                            .collect::<String>()
                    })
                    .collect::<Vec<_>>()
                    .join("\n"),
            );
        }

        for output in &table_outputs {
            assert_eq!(output, &table_outputs[0]);
        }
        for output in &profile_outputs {
            assert_eq!(output, &profile_outputs[0]);
        }
    }
}
