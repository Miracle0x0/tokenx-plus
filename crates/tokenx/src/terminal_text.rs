//! Terminal-cell-aware text measurement and fitting.
//!
//! Renderers must use terminal cell width rather than Rust character or byte
//! counts. Keeping the policy here makes CJK, combining marks, and other
//! variable-width text follow one invariant across the TUI.

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// Return the number of terminal cells occupied by `text`.
pub(crate) fn width(text: &str) -> usize {
    UnicodeWidthStr::width(text)
}

/// Return the terminal-cell width of one Unicode scalar value.
pub(crate) fn char_width(ch: char) -> usize {
    UnicodeWidthChar::width(ch).unwrap_or(0)
}

/// Saturating `u16` form of [`width`] for Ratatui geometry.
pub(crate) fn width_u16(text: &str) -> u16 {
    width(text).min(u16::MAX as usize) as u16
}

/// Truncate without splitting a scalar value so the result fits `max_width`.
pub(crate) fn truncate(text: &str, max_width: usize) -> String {
    if width(text) <= max_width {
        return text.to_string();
    }

    let mut used = 0usize;
    text.chars()
        .take_while(|ch| {
            let next = used.saturating_add(char_width(*ch));
            if next > max_width {
                false
            } else {
                used = next;
                true
            }
        })
        .collect()
}

/// Truncate to `max_width`, reserving one cell for an ellipsis when needed.
pub(crate) fn truncate_with_ellipsis(text: &str, max_width: usize) -> String {
    truncate_with_suffix(text, max_width, "…")
}

/// Truncate to `max_width`, appending `suffix` when it fits.
pub(crate) fn truncate_with_suffix(text: &str, max_width: usize, suffix: &str) -> String {
    if width(text) <= max_width {
        return text.to_string();
    }
    if max_width == 0 {
        return String::new();
    }

    let suffix_width = width(suffix);
    if max_width <= suffix_width {
        return truncate(text, max_width);
    }

    let mut fitted = truncate(text, max_width - suffix_width);
    fitted.push_str(suffix);
    fitted
}

/// Right-pad to `target_width` terminal cells without truncating.
pub(crate) fn pad_right(text: &str, target_width: usize) -> String {
    let padding = target_width.saturating_sub(width(text));
    format!("{text}{}", " ".repeat(padding))
}

/// Left-pad to `target_width` terminal cells without truncating.
pub(crate) fn pad_left(text: &str, target_width: usize) -> String {
    let padding = target_width.saturating_sub(width(text));
    format!("{}{text}", " ".repeat(padding))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn measures_ascii_cjk_and_combining_marks_in_cells() {
        assert_eq!(width("abc"), 3);
        assert_eq!(width("中文"), 4);
        assert_eq!(width("e\u{301}"), 1);
    }

    #[test]
    fn truncation_and_padding_share_the_cell_width_invariant() {
        assert_eq!(truncate("中ab", 3), "中a");
        assert_eq!(truncate_with_ellipsis("中文ab", 4), "中…");
        assert_eq!(width(&truncate_with_ellipsis("中文ab", 4)), 3);
        assert_eq!(truncate_with_suffix("中文ab", 5, "..."), "中...");
        assert_eq!(pad_right("中", 4), "中  ");
        assert_eq!(pad_left("中", 4), "  中");
    }

    #[test]
    fn zero_width_budget_is_empty_even_when_truncating() {
        assert_eq!(truncate("中", 0), "");
        assert_eq!(truncate_with_ellipsis("中", 0), "");
    }
}
