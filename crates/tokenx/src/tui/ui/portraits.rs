//! Kaomoji portraits for the Overview snapshot's favorite model family:
//! one original artwork per family, painted in the family's brand color.

use std::borrow::Cow;

use ratatui::prelude::*;
use unicode_width::UnicodeWidthStr;

use crate::tui::model::TuiModel;
use crate::tui::model_family::ModelFamily;

pub(super) fn display_name(family: ModelFamily) -> &'static str {
    match family {
        ModelFamily::Gpt => "gpt",
        ModelFamily::Claude => "claude",
        ModelFamily::Gemini => "gemini",
        ModelFamily::Xai => "xai",
        ModelFamily::Glm => "glm",
        ModelFamily::Deepseek => "deepseek",
        ModelFamily::Qwen => "qwen",
        ModelFamily::Kimi => "kimi",
        ModelFamily::Minimax => "minimax",
        ModelFamily::Mimo => "mimo",
        ModelFamily::Mistral => "mistral",
        ModelFamily::Unknown => "???",
    }
}

pub(super) fn slogan(family: ModelFamily) -> &'static str {
    let key = match family {
        ModelFamily::Gpt => "tui.ui.portraits.slogan.gpt",
        ModelFamily::Claude => "tui.ui.portraits.slogan.claude",
        ModelFamily::Gemini => "tui.ui.portraits.slogan.gemini",
        ModelFamily::Xai => "tui.ui.portraits.slogan.xai",
        ModelFamily::Mimo => "tui.ui.portraits.slogan.mimo",
        ModelFamily::Minimax => "tui.ui.portraits.slogan.minimax",
        ModelFamily::Qwen => "tui.ui.portraits.slogan.qwen",
        ModelFamily::Kimi => "tui.ui.portraits.slogan.kimi",
        ModelFamily::Glm => "tui.ui.portraits.slogan.glm",
        ModelFamily::Deepseek => "tui.ui.portraits.slogan.deepseek",
        ModelFamily::Mistral => "tui.ui.portraits.slogan.mistral",
        ModelFamily::Unknown => "tui.ui.portraits.slogan.unknown",
    };
    // Configured translations are borrowed straight from the static backend;
    // the owned arm only fires when a key is missing from every locale.
    match rust_i18n::t!(key) {
        Cow::Borrowed(text) => text,
        Cow::Owned(text) => Box::leak(text.into_boxed_str()),
    }
}

/// Fixed brand color per family (logo primary colors), adapted through the
/// shared identity-color path for surface contrast.
pub(super) fn family_color(app: &TuiModel, family: ModelFamily) -> Color {
    app.family_color(family)
}

/// Overview card artwork uses one fixed three-row visual contract.
pub(super) const PORTRAIT_HEIGHT: usize = 3;
type Portrait = [&'static str; PORTRAIT_HEIGHT];

const GPT: Portrait = ["     ╲", "  (¬‿¬)╮", "   ⁄|~|⁄"];
const CLAUDE: Portrait = ["   ╭─ ✦ ─╮", "  (˶ᵔ ᵕ ᵔ˶)", "    /| |\\"];
const GEMINI: Portrait = ["  ✦    ✦", "  (◕‿◕)✦", "   /||\\"];
const XAI: Portrait = ["    𝕏", "  (¬‿¬)✕", "   /|\\"];
const GLM: Portrait = ["   ___", "  (⌐■_■)▤", "   /|  |\\"];
const DEEPSEEK: Portrait = ["  ～～～", " (｡•́︿•̀｡)", "   ～|～"];
const QWEN: Portrait = ["   ☁", "  (｡•̀ᴗ•́｡)☁", "   /|\\"];
const KIMI: Portrait = ["   ☾", "  (｡･ω･｡)☾", "   /|\\"];
const MINIMAX: Portrait = ["   /\\  /\\", "  (｡•̀ᴗ•́)◆", "   /|  |\\"];
const MIMO: Portrait = ["   ___", "  (｡•ω•｡)¤", "   /|  |\\"];
const MISTRAL: Portrait = ["  ≋≋≋", " (•̀ᴗ•́)≋", "   /|\\"];
const UNKNOWN: Portrait = ["  [■_■]", "  (•_•)", "   /|\\"];
const ROW_PADDING: &str = "                ";

pub(super) fn portrait(family: ModelFamily) -> [&'static str; PORTRAIT_HEIGHT] {
    match family {
        ModelFamily::Gpt => GPT,
        ModelFamily::Claude => CLAUDE,
        ModelFamily::Gemini => GEMINI,
        ModelFamily::Xai => XAI,
        ModelFamily::Glm => GLM,
        ModelFamily::Deepseek => DEEPSEEK,
        ModelFamily::Qwen => QWEN,
        ModelFamily::Kimi => KIMI,
        ModelFamily::Minimax => MINIMAX,
        ModelFamily::Mimo => MIMO,
        ModelFamily::Mistral => MISTRAL,
        ModelFamily::Unknown => UNKNOWN,
    }
}

/// Every line is padded on the right to the family block's width so the
/// artwork's authored left-edge alignment survives per-line centering
/// (left-padding each line independently was the misalignment bug).
pub(super) fn lines(app: &TuiModel, family: ModelFamily) -> [Line<'static>; PORTRAIT_HEIGHT] {
    styled_lines(family, family_color(app, family))
}

fn styled_lines(family: ModelFamily, color: Color) -> [Line<'static>; PORTRAIT_HEIGHT] {
    let art = portrait(family);
    let block_width = art
        .iter()
        .map(|row| UnicodeWidthStr::width(*row))
        .max()
        .unwrap_or(0);
    art.map(|row| {
        let padding_width = block_width - UnicodeWidthStr::width(row);
        Line::from(vec![
            Span::styled(row, Style::default().fg(color)),
            Span::raw(&ROW_PADDING[..padding_width]),
        ])
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn portrait_lines_have_fixed_height_and_equal_display_width() {
        for family in [
            ModelFamily::Gpt,
            ModelFamily::Claude,
            ModelFamily::Gemini,
            ModelFamily::Xai,
            ModelFamily::Glm,
            ModelFamily::Deepseek,
            ModelFamily::Qwen,
            ModelFamily::Kimi,
            ModelFamily::Minimax,
            ModelFamily::Mimo,
            ModelFamily::Mistral,
            ModelFamily::Unknown,
        ] {
            let art = portrait(family);
            let width = art
                .iter()
                .map(|row| UnicodeWidthStr::width(*row))
                .max()
                .unwrap();
            assert!(width <= 16, "portrait too wide for the column: {width}");

            let lines = styled_lines(family, Color::White);
            assert_eq!(lines.len(), PORTRAIT_HEIGHT);
            assert!(lines.iter().all(|line| line.width() == width));
            assert!(
                lines.iter().all(|line| line.spans.len() == 2),
                "each static row should render as one styled span plus padding"
            );
        }

        assert!(
            portrait(ModelFamily::Qwen)
                .iter()
                .any(|row| row.chars().count() != UnicodeWidthStr::width(*row)),
            "fixture must retain a combining-mark row that exercises display width"
        );
    }
}
