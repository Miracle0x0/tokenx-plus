use std::f64::consts::{FRAC_PI_2, TAU};

use ratatui::prelude::*;
use tui_piechart::PieSlice;

pub(super) const LEGEND_RESERVED_W: u16 = 21;

// Braille cells contain two columns and four rows of equally spaced dots.
const DOTS: [(u16, u16, u32); 8] = [
    (0, 0, 0x01),
    (0, 1, 0x02),
    (0, 2, 0x04),
    (0, 3, 0x40),
    (1, 0, 0x08),
    (1, 1, 0x10),
    (1, 2, 0x20),
    (1, 3, 0x80),
];

/// Draw the disc and its right-hand legend using one dot-space center for
/// the circular outline and the angular color boundaries.
pub(super) fn render_pie(frame: &mut Frame, area: Rect, slices: &[PieSlice<'_>], style: Style) {
    if area.is_empty() || slices.is_empty() {
        return;
    }

    let show_legend = area.width > LEGEND_RESERVED_W && area.height >= 10;
    let disc = Rect {
        width: area.width - if show_legend { LEGEND_RESERVED_W } else { 0 },
        ..area
    };
    let total: f64 = slices.iter().map(PieSlice::value).sum();
    let mut cumulative = 0.0;
    let angles: Vec<_> = slices
        .iter()
        .map(|slice| {
            cumulative += slice.value();
            cumulative / total * TAU
        })
        .collect();

    // An even character grid puts the center between four cells. No single
    // foreground color then has to represent several sectors at the junction.
    // Dot indices run from zero to dimension - 1, giving a half-dot center.
    let grid_width = disc.width / 2 * 2;
    let grid_height = disc.height / 2 * 2;
    let center_x = f64::from(grid_width) - 0.5;
    let center_y = f64::from(grid_height) * 2.0 - 0.5;
    let radius = (f64::from(grid_width).min(f64::from(grid_height) * 2.0) - 1.0).max(0.0);
    let radius_squared = radius * radius;
    let buffer = frame.buffer_mut();
    buffer.set_style(area, style);
    for y in 0..grid_height {
        for x in 0..grid_width {
            let base_x = f64::from(x) * 2.0 - center_x;
            let base_y = f64::from(y) * 4.0 - center_y;
            let mut pattern = 0;
            for (dx, dy, bit) in DOTS {
                let dx = base_x + f64::from(dx);
                let dy = base_y + f64::from(dy);
                if dx * dx + dy * dy <= radius_squared {
                    pattern |= bit;
                }
            }
            if pattern == 0 {
                continue;
            }

            // One character has one foreground color. Sample its center so
            // the sector assignment does not move with outline clipping or
            // tie-breaking between the colors of its individual dots.
            let angle = ((base_y + 1.5).atan2(base_x + 0.5) + FRAC_PI_2).rem_euclid(TAU);
            let slice = &slices[angles.partition_point(|end| *end <= angle)];
            buffer[(disc.x + x, disc.y + y)]
                .set_char(char::from_u32(0x2800 + pattern).expect("valid Braille dot mask"))
                .set_fg(slice.color());
        }
    }

    if show_legend {
        let legend = Rect::new(
            disc.right() + 1,
            area.y + 1,
            LEGEND_RESERVED_W - 1,
            area.height - 2,
        );
        for (row, slice) in (0..legend.height).step_by(2).zip(slices) {
            frame.render_widget(
                Line::styled(
                    format!("■ {} {:.1}%", slice.label(), slice.value() / total * 100.0),
                    Style::default().fg(slice.color()),
                ),
                Rect::new(legend.x, legend.y + row, legend.width, 1),
            );
        }
    }
}
