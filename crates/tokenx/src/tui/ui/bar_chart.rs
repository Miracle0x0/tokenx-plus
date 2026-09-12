//! Chromeless stacked bar chart for the Overview "Token per Day" panel.
//!
//! Rendering contract (implemented in this module):
//! - Recorded bars and missing dates share the whole-cell width chosen by the
//!   caller. If the calendar interval cannot fit even at one column per day,
//!   missing dates share the remaining columns and may collapse to zero width.
//!   Any remainder is split between the left and right margins (the right gets
//!   any odd column). Recorded bars clip at the right edge if they cannot fit.
//! - The gridline, baseline, peak marker, and labels use the same centered
//!   chart area, without a y-axis gutter or title row.
//! - Rows relative to the chart area: bars occupy rows `0..h-2`, row `h-2` is a
//!   baseline of '─', row `h-1` holds the date labels.
//! - A single dotted gridline ('┄') crosses the vertical middle of the bar
//!   field; it is drawn before the bars so it only shows through empty cells.
//! - A compact `peak {max}` marker overlays the top-right of bar row 0.
//! - Date labels are anchored to the edges: first date left-aligned, last date
//!   right-aligned, middle recorded date centered under its bar when it fits.

use ratatui::prelude::*;

use super::widgets::format_tokens;
use crate::date_display::month_name;
use crate::terminal_text::{char_width, width_u16};
use crate::tui::model::TuiModel;

/// 8-level block characters for sub-cell precision (matching OpenTUI)
const BLOCKS: &[char] = &[' ', '▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

/// A single model's contribution to a bar
#[derive(Debug, Clone)]
pub struct ModelSegment {
    pub model_id: String,
    pub tokens: u64,
    pub color: Color,
}

/// Data for a single bar in the stacked chart
#[derive(Debug, Clone)]
pub struct StackedBarData {
    pub date: String,
    /// Missing dates since the preceding recorded bar; zero for the first bar.
    pub empty_days_before: usize,
    pub models: Vec<ModelSegment>,
    pub total: u64,
}

struct BarChartLayout {
    bar_width: usize,
    offsets: Vec<usize>,
    width: u16,
}

impl BarChartLayout {
    /// Place the selected series at its fixed width, compressing gaps only
    /// when the caller's one-column calendar interval exceeds the viewport.
    fn new(data: &[StackedBarData], available_width: u16, bar_width: usize) -> Self {
        let available_width = available_width as usize;
        let recorded_width = bar_width * data.len();
        let remaining_width = available_width - recorded_width.min(available_width);
        let empty_days: usize = data.iter().map(|bar| bar.empty_days_before).sum();
        let gap_width = remaining_width.min(empty_days * bar_width);
        let mut preceding_empty_days = 0;
        let mut preceding_gap_width = 0;
        let offsets = data
            .iter()
            .enumerate()
            .map(|(index, bar)| {
                if bar.empty_days_before > 0 {
                    preceding_empty_days += bar.empty_days_before;
                    preceding_gap_width = preceding_empty_days * gap_width / empty_days;
                }
                index * bar_width + preceding_gap_width
            })
            .collect();

        Self {
            bar_width,
            offsets,
            width: (recorded_width + gap_width).min(available_width) as u16,
        }
    }
}

/// Render a stacked bar chart where each bar shows model breakdown
pub fn render_stacked_bar_chart(
    frame: &mut Frame,
    app: &TuiModel,
    area: Rect,
    data: &[StackedBarData],
    bar_width: usize,
) {
    if data.is_empty() || area.height < 2 || area.width == 0 {
        return;
    }

    let layout = BarChartLayout::new(data, area.width, bar_width);
    let area = Rect {
        x: area.x + (area.width - layout.width) / 2,
        width: layout.width,
        ..area
    };
    let chart_height = area.height.saturating_sub(2) as usize;

    let max_value = data
        .iter()
        .map(|d| d.total as f64)
        .fold(0.0_f64, |a, b| a.max(b))
        .max(1.0);

    let buf = frame.buffer_mut();

    // Dotted mid gridline, drawn before the bars so they overwrite it and it
    // only shows through the empty cells above shorter bars.
    if chart_height >= 4 {
        let grid_y = area.y + (chart_height / 2) as u16;
        for x in area.x..area.x + area.width {
            buf[(x, grid_y)]
                .set_char('┄')
                .set_style(Style::default().fg(app.theme.visualization.grid));
        }
    }

    // Render bars row by row (from top to bottom visually, which is high values to low)
    for row_from_bottom in (0..chart_height).rev() {
        let row_index = chart_height - 1 - row_from_bottom;
        let y = area.y + row_index as u16;

        // Render each bar
        for (bar_data, &offset) in data.iter().zip(&layout.offsets) {
            let row_threshold = ((row_from_bottom + 1) as f64 / chart_height as f64) * max_value;
            let prev_threshold = (row_from_bottom as f64 / chart_height as f64) * max_value;
            let threshold_diff = row_threshold - prev_threshold;

            let total = bar_data.total as f64;

            // Get the character and color for this cell using stacked model logic
            let (ch, fg_color) = get_stacked_bar_content(
                bar_data,
                total,
                row_threshold,
                prev_threshold,
                threshold_diff,
                app.theme.text.secondary,
                app.theme.visualization.chart_highlight,
            );

            for x in offset..(offset + layout.bar_width).min(area.width as usize) {
                // Leave empty cells untouched so the gridline shows through.
                if ch != ' ' {
                    buf[(area.x + x as u16, y)].set_char(ch).set_fg(fg_color);
                }
            }
        }
    }

    // Baseline separating the bars from the date labels
    let baseline_y = area.y + area.height - 2;
    for x in area.x..area.x + area.width {
        buf[(x, baseline_y)]
            .set_char('─')
            .set_style(Style::default().fg(app.theme.visualization.grid));
    }

    // Compact peak marker, right-aligned on the top bar row; drawn after the
    // bars with an explicit background so it reads over any bar beneath it.
    if chart_height > 0 {
        let peak_label = rust_i18n::t!(
            "tui.ui.bar_chart.peak",
            max = compact_tokens(max_value as u64)
        )
        .into_owned();
        let peak_width = width_u16(peak_label.as_str());
        if area.width >= peak_width + 2 {
            let peak_x = area.x + area.width - peak_width;
            buf.set_string(
                peak_x,
                area.y,
                &peak_label,
                Style::default()
                    .fg(app.theme.text.secondary)
                    .bg(app.theme.surface.panel),
            );
        }
    }

    render_date_labels(buf, app, area, data, &layout);
}

/// Date labels anchored to the edges of the chart: first date left-aligned at
/// `area.x`, last date right-aligned to end at the right edge, and — unless the
/// app is very narrow — the middle recorded date centered under its bar. When
/// labels would overlap, the middle one is dropped first; if the remaining two
/// still collide, the right one is truncated from the left, keeping its tail.
fn render_date_labels(
    buf: &mut Buffer,
    app: &TuiModel,
    area: Rect,
    data: &[StackedBarData],
    layout: &BarChartLayout,
) {
    let label_y = area.y + area.height - 1;
    let is_very_narrow = app.is_very_narrow();
    let bar_count = data.len();

    let first_label = format_date_label(&data[0].date, is_very_narrow);
    let first_width = width_u16(first_label.as_str());

    let mut labels: Vec<(String, u16)> = vec![(first_label, area.x)];

    if bar_count > 1 {
        let last_label = format_date_label(&data[bar_count - 1].date, is_very_narrow);
        let last_width = width_u16(last_label.as_str());

        if !is_very_narrow && bar_count > 2 {
            let middle_label = format_date_label(&data[bar_count / 2].date, is_very_narrow);
            let middle_width = width_u16(middle_label.as_str());
            let middle_center = layout.offsets[bar_count / 2] + layout.bar_width / 2;
            if let Some(middle_x) = middle_center.checked_sub(middle_width as usize / 2) {
                // Keep labels separated while following the bar's actual position.
                if middle_x > first_width as usize
                    && middle_x + middle_width as usize + (last_width as usize)
                        < area.width as usize
                {
                    labels.push((middle_label, area.x + middle_x as u16));
                }
            }
        }

        let last_label = if first_width + last_width + 1 > area.width {
            // Still colliding without the middle label: truncate the right
            // label from the left, keeping its tail.
            let keep = area.width.saturating_sub(first_width + 1) as usize;
            truncate_left_to_width(&last_label, keep)
        } else {
            last_label
        };
        let last_width = width_u16(last_label.as_str());
        if last_width > 0 {
            labels.push((last_label, area.x + area.width - last_width));
        }
    }

    for (label, label_x) in labels {
        buf.set_string(
            label_x,
            label_y,
            &label,
            Style::default().fg(app.theme.text.secondary),
        );
    }
}

fn truncate_left_to_width(label: &str, max_width: usize) -> String {
    let mut width = 0;
    let mut kept = Vec::new();
    for ch in label.chars().rev() {
        let ch_width = char_width(ch);
        if width + ch_width > max_width {
            break;
        }
        kept.push(ch);
        width += ch_width;
    }
    kept.into_iter().rev().collect()
}

/// Format a raw `month/day` date string for the label row.
fn format_date_label(date_str: &str, is_very_narrow: bool) -> String {
    if let Some((month_str, day_str)) = date_str.split_once('/') {
        if let (Ok(month), Ok(day)) = (month_str.parse::<usize>(), day_str.parse::<u32>()) {
            if (1..=12).contains(&month) {
                return if is_very_narrow {
                    format!("{}/{}", month, day)
                } else {
                    rust_i18n::t!(
                        "tui.date.month_day",
                        month = month_name(month as u32, true),
                        day = day
                    )
                    .into_owned()
                };
            }
        }
    }
    date_str.to_string()
}

/// Compact token count for the peak marker: when the integer part has 2+
/// digits, drop the decimal ("38.2B" -> "38B", "123.4B" -> "123B"; "2.1B"
/// stays as-is).
fn compact_tokens(tokens: u64) -> String {
    let formatted = format_tokens(tokens);
    if let Some(dot) = formatted.find('.') {
        let int_digits = formatted[..dot]
            .chars()
            .filter(|c| c.is_ascii_digit())
            .count();
        if int_digits >= 2 {
            let unit = formatted[dot + 1..].trim_start_matches(|c: char| c.is_ascii_digit());
            return format!("{}{}", &formatted[..dot], unit);
        }
    }
    formatted
}

fn get_stacked_bar_content(
    bar_data: &StackedBarData,
    total: f64,
    row_threshold: f64,
    prev_threshold: f64,
    threshold_diff: f64,
    muted_color: Color,
    fallback_color: Color,
) -> (char, Color) {
    if total <= prev_threshold {
        return (' ', muted_color);
    }

    if bar_data.models.is_empty() {
        return (' ', muted_color);
    }

    // Note: Sorting happens per cell render. If performance becomes an issue,
    // consider pre-sorting the model list before calling this function.
    let mut sorted_models: Vec<&ModelSegment> = bar_data.models.iter().collect();
    sorted_models.sort_by(|a, b| a.model_id.cmp(&b.model_id));

    let row_start = prev_threshold;
    let row_end = row_threshold;

    let mut current_height: f64 = 0.0;
    let mut max_overlap: f64 = 0.0;
    let mut best_color = sorted_models
        .first()
        .map(|m| m.color)
        .unwrap_or(fallback_color);

    for model in &sorted_models {
        let m_start = current_height;
        let m_end = current_height + model.tokens as f64;
        current_height += model.tokens as f64;

        let overlap_start = m_start.max(row_start);
        let overlap_end = m_end.min(row_end);
        let overlap = (overlap_end - overlap_start).max(0.0);

        if overlap > max_overlap {
            max_overlap = overlap;
            best_color = model.color;
        }
    }

    if total >= row_threshold {
        return (BLOCKS[8], best_color);
    }

    let ratio = if threshold_diff > 0.0 {
        (total - prev_threshold) / threshold_diff
    } else {
        1.0
    };
    let block_index = (ratio * 8.0).floor().clamp(1.0, 8.0) as usize;
    (BLOCKS[block_index], best_color)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::model::TuiConfig;
    use ratatui::{backend::TestBackend, Terminal};

    fn make_app(width: u16) -> TuiModel {
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
        app
    }

    fn bar(date: &str, total: u64) -> StackedBarData {
        StackedBarData {
            date: date.to_string(),
            empty_days_before: 0,
            models: vec![ModelSegment {
                model_id: "test-model".to_string(),
                tokens: total,
                color: Color::Green,
            }],
            total,
        }
    }

    fn render_chart(
        app: &TuiModel,
        area: Rect,
        data: &[StackedBarData],
        width: u16,
        height: u16,
    ) -> Buffer {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        let day_count = data.len() + data.iter().map(|bar| bar.empty_days_before).sum::<usize>();
        let bar_width = (area.width as usize / day_count.max(1)).max(1);
        terminal
            .draw(|frame| render_stacked_bar_chart(frame, app, area, data, bar_width))
            .unwrap();
        terminal.backend().buffer().clone()
    }

    fn row_string(buf: &Buffer, y: u16) -> String {
        (0..buf.area.width)
            .map(|x| buf[(x, y)].symbol().to_string())
            .collect()
    }

    #[test]
    fn bars_have_equal_widths_with_centered_margins() {
        let app = make_app(120);
        let data: Vec<StackedBarData> = (0..7)
            .map(|i| {
                let mut bar = bar(&format!("1/{}", i + 1), 100);
                bar.models[0].color = if i % 2 == 0 {
                    Color::Green
                } else {
                    Color::Blue
                };
                bar
            })
            .collect();

        // Divisible widths, even/odd remainders, and one-column bars.
        for (width, bar_width, left_margin, right_margin) in [
            (35, 5, 0, 0),
            (39, 5, 2, 2),
            (40, 5, 2, 3),
            (7, 1, 0, 0),
            (6, 1, 0, 0),
        ] {
            let area = Rect::new(2, 1, width, 10);
            let buf = render_chart(&app, area, &data, area.right() + 2, 12);
            let chart_start = area.x + left_margin;
            let chart_end = area.right() - right_margin;
            let bar_y = area.bottom() - 3;

            for x in chart_start..chart_end {
                let bar_index = ((x - chart_start) / bar_width) as usize;
                assert_eq!(buf[(x, bar_y)].symbol(), "█", "width {width}, x {x}");
                assert_eq!(
                    buf[(x, bar_y)].fg,
                    data[bar_index].models[0].color,
                    "width {width}, x {x} belongs to bar {bar_index}"
                );
            }
            for y in area.y..area.bottom() {
                for x in (0..chart_start).chain(chart_end..buf.area.width) {
                    assert_eq!(buf[(x, y)].symbol(), " ", "width {width}, ({x}, {y})");
                }
            }
        }
    }

    #[test]
    fn missing_dates_match_bar_width_except_when_the_calendar_cannot_fit() {
        let app = make_app(120);
        for (width, gaps, expected) in [
            // Five dates at two columns each, with one unused column on the right.
            (11, vec![0, 1, 1], "██  ██  ██ "),
            (12, vec![0, 1, 3], "  █ █   █   "),
            (13, vec![0, 0, 0, 1, 0, 0, 0], "  ███ ████   "),
            // Five missing dates share two columns, keeping all recorded bars.
            (5, vec![0, 2, 3], "██  █"),
            (3, vec![0, 2, 3], "███"),
            // A long interval keeps one-column recorded bars and compresses its gap.
            (11, vec![0, 0, 1_000_000], "██        █"),
        ] {
            let data: Vec<_> = gaps
                .into_iter()
                .map(|empty_days_before| {
                    let mut bar = bar("1/5", 100);
                    bar.empty_days_before = empty_days_before;
                    bar
                })
                .collect();
            let area = Rect::new(0, 0, width, 8);
            let buf = render_chart(&app, area, &data, width, 8);
            assert_eq!(row_string(&buf, 5), expected, "width {width}");
        }
    }

    #[test]
    fn missing_dates_leave_the_gridline_visible() {
        let app = make_app(120);
        let mut data = vec![bar("1/5", 100), bar("1/7", 100), bar("1/9", 100)];
        data[1].empty_days_before = 1;
        data[2].empty_days_before = 1;
        let area = Rect::new(0, 0, 10, 8);
        let buf = render_chart(&app, area, &data, 10, 8);

        assert_eq!(row_string(&buf, 3), "██┄┄██┄┄██");
        assert_eq!(row_string(&buf, 6), "──────────");
    }

    #[test]
    fn baseline_row_is_all_horizontal_lines() {
        let app = make_app(120);
        let data = vec![bar("1/5", 100), bar("1/6", 0), bar("1/7", 50)];
        let area = Rect::new(0, 0, 30, 8);
        let buf = render_chart(&app, area, &data, 30, 8);

        let baseline_y = area.y + area.height - 2;
        for x in area.x..area.x + area.width {
            assert_eq!(buf[(x, baseline_y)].symbol(), "─");
            assert_eq!(buf[(x, baseline_y)].fg, app.theme.visualization.grid);
        }
    }

    #[test]
    fn peak_label_sits_top_right_and_compacts_the_value() {
        let app = make_app(120);
        let data = vec![bar("1/5", 38_200_000_000), bar("1/6", 100)];
        let area = Rect::new(0, 0, 30, 8);
        let buf = render_chart(&app, area, &data, 30, 8);

        let label = "peak 38B";
        let start_x = area.x + area.width - label.len() as u16;
        let row: String = row_string(&buf, area.y)
            .chars()
            .skip(start_x as usize)
            .collect();
        assert_eq!(row, label);
        assert_eq!(buf[(start_x, area.y)].fg, app.theme.text.secondary);
        assert_eq!(buf[(start_x, area.y)].bg, app.theme.surface.panel);
    }

    #[test]
    fn compact_tokens_drops_decimals_for_two_or_more_integer_digits() {
        assert_eq!(compact_tokens(38_200_000_000), "38B");
        assert_eq!(compact_tokens(123_400_000_000), "123B");
        assert_eq!(compact_tokens(2_100_000_000), "2.1B");
        assert_eq!(compact_tokens(500), "500");
    }

    #[test]
    fn mid_gridline_shows_only_through_empty_cells() {
        let app = make_app(120);
        // Left half full height, right half empty.
        let data = vec![bar("1/5", 100), bar("1/6", 0)];
        let area = Rect::new(0, 0, 20, 10);
        let buf = render_chart(&app, area, &data, 20, 10);

        let chart_height = area.height - 2;
        let grid_y = area.y + chart_height / 2;
        // Full bar overwrites the gridline on the left half.
        assert_eq!(buf[(2, grid_y)].symbol(), "█");
        // Empty bar lets the gridline show through on the right half.
        assert_eq!(buf[(15, grid_y)].symbol(), "┄");
        assert_eq!(buf[(15, grid_y)].fg, app.theme.visualization.grid);
    }

    #[test]
    fn date_labels_anchor_to_the_centered_chart_edges() {
        let app = make_app(120);
        let data = vec![bar("1/5", 10), bar("6/15", 20), bar("12/25", 30)];
        // Three 13-column bars, with one unused column on either side.
        let area = Rect::new(3, 2, 41, 8);
        let buf = render_chart(&app, area, &data, 46, 12);

        let label_y = area.y + area.height - 1;
        let row = row_string(&buf, label_y);
        let inner: String = row
            .chars()
            .skip(area.x as usize)
            .take(area.width as usize)
            .collect();
        assert!(
            inner.starts_with(" Jan 5"),
            "first date after the left margin: {inner:?}"
        );
        assert!(
            inner.ends_with("Dec 25 "),
            "last date before the right margin: {inner:?}"
        );
        assert!(inner.contains("Jun 15"), "middle date centered: {inner:?}");
        let middle_start = inner.find("Jun 15").unwrap();
        let expected = (area.width as usize - "Jun 15".len()) / 2;
        assert!(
            middle_start.abs_diff(expected) <= 1,
            "middle label roughly centered at {middle_start}, expected ~{expected}"
        );
    }

    #[test]
    fn middle_date_label_follows_its_bar_after_a_gap() {
        let app = make_app(120);
        let mut data: Vec<_> = [1, 2, 3, 9, 10, 11, 12]
            .into_iter()
            .map(|day| bar(&format!("1/{day}"), 100))
            .collect();
        data[3].empty_days_before = 5;
        let area = Rect::new(0, 0, 41, 8);
        let buf = render_chart(&app, area, &data, 41, 8);

        // Twelve dates at three columns each, with a two-column left margin.
        // Jan 9 starts at x=26; its five-column label centers at x=27.
        assert_eq!(row_string(&buf, 7).find("Jan 9"), Some(25));
    }

    #[test]
    fn very_narrow_app_renders_only_two_labels() {
        let app = make_app(50);
        let data = vec![bar("1/5", 10), bar("6/15", 20), bar("12/25", 30)];
        let area = Rect::new(0, 0, 30, 8);
        let buf = render_chart(&app, area, &data, 30, 8);

        let label_y = area.y + area.height - 1;
        let inner = row_string(&buf, label_y);
        assert!(inner.starts_with("1/5"), "first date at area.x: {inner:?}");
        assert!(
            inner.ends_with("12/25"),
            "last date at right edge: {inner:?}"
        );
        assert!(
            !inner.contains("6/15"),
            "very narrow apps drop the middle label: {inner:?}"
        );
    }

    #[test]
    fn tiny_and_empty_inputs_do_not_panic() {
        let app = make_app(120);
        let data = vec![bar("1/5", 100), bar("1/6", 50)];
        for (width, height) in [(0, 0), (1, 0), (0, 1), (1, 1), (4, 1), (3, 2), (1, 5)] {
            let _ = render_chart(&app, Rect::new(0, 0, width, height), &data, 8, 8);
        }
        let _ = render_chart(&app, Rect::new(0, 0, 8, 8), &[], 8, 8);
    }
}
