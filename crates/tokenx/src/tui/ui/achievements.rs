//! Achievement ladders for the Overview snapshot: five permanent ladders,
//! each with five tiers plus a roast title for below the first tier.

use std::borrow::Cow;

use ratatui::prelude::*;

use crate::terminal_text::width;
use crate::tui::data::CacheRate;
use crate::tui::themes::Theme;

/// Tier names are i18n keys resolved at render time; the third tuple field is
/// the locale-independent threshold display ("7", "0.1B", "50%").
struct TierSet {
    roast: &'static str,
    tiers: [(u64, &'static str, &'static str); 5],
}

const STREAK: TierSet = TierSet {
    roast: "tui.ui.achievements.streak.roast",
    tiers: [
        (7, "tui.ui.achievements.streak.tier1", "7"),
        (15, "tui.ui.achievements.streak.tier2", "15"),
        (30, "tui.ui.achievements.streak.tier3", "30"),
        (90, "tui.ui.achievements.streak.tier4", "90"),
        (180, "tui.ui.achievements.streak.tier5", "180"),
    ],
};
const TOKENS: TierSet = TierSet {
    roast: "tui.ui.achievements.tokens.roast",
    tiers: [
        (100_000_000, "tui.ui.achievements.tokens.tier1", "0.1B"),
        (1_000_000_000, "tui.ui.achievements.tokens.tier2", "1B"),
        (10_000_000_000, "tui.ui.achievements.tokens.tier3", "10B"),
        (100_000_000_000, "tui.ui.achievements.tokens.tier4", "100B"),
        (1_000_000_000_000, "tui.ui.achievements.tokens.tier5", "1T"),
    ],
};
const CACHE: TierSet = TierSet {
    roast: "tui.ui.achievements.cache.roast",
    tiers: [
        (50, "tui.ui.achievements.cache.tier1", "50%"),
        (80, "tui.ui.achievements.cache.tier2", "80%"),
        (90, "tui.ui.achievements.cache.tier3", "90%"),
        (95, "tui.ui.achievements.cache.tier4", "95%"),
        (99, "tui.ui.achievements.cache.tier5", "99%"),
    ],
};
const MODELS: TierSet = TierSet {
    roast: "tui.ui.achievements.models.roast",
    tiers: [
        (10, "tui.ui.achievements.models.tier1", "10"),
        (20, "tui.ui.achievements.models.tier2", "20"),
        (30, "tui.ui.achievements.models.tier3", "30"),
        (100, "tui.ui.achievements.models.tier4", "100"),
        (200, "tui.ui.achievements.models.tier5", "200"),
    ],
};
const CLIENTS: TierSet = TierSet {
    roast: "tui.ui.achievements.clients.roast",
    tiers: [
        (1, "tui.ui.achievements.clients.tier1", "1"),
        (5, "tui.ui.achievements.clients.tier2", "5"),
        (10, "tui.ui.achievements.clients.tier3", "10"),
        (15, "tui.ui.achievements.clients.tier4", "15"),
        (20, "tui.ui.achievements.clients.tier5", "20"),
    ],
};

const TITLE_WIDTH: usize = 10;

pub(super) struct Achievement {
    title: Cow<'static, str>,
    ladder: [&'static str; 5],
    /// Tier index 0..=4, or -1 when below the first tier.
    current: i8,
}

fn rank(set: &TierSet, value: u64) -> Achievement {
    rank_when(set, |threshold| value >= threshold)
}

fn rank_cache(set: &TierSet, rate: CacheRate) -> Achievement {
    rank_when(set, |threshold| rate.reaches(threshold))
}

fn rank_when(set: &TierSet, reached: impl Fn(u64) -> bool) -> Achievement {
    let mut current: i8 = -1;
    for (index, (threshold, _, _)) in set.tiers.iter().enumerate() {
        if reached(*threshold) {
            current = index as i8;
        }
    }
    let title = if current < 0 {
        rust_i18n::t!(set.roast)
    } else {
        rust_i18n::t!(set.tiers[current as usize].1)
    };
    Achievement {
        title,
        ladder: set.tiers.map(|(_, _, display)| display),
        current,
    }
}

pub(super) fn build(
    current_streak: u32,
    total_tokens: u64,
    cache_rate: CacheRate,
    models: usize,
    clients: usize,
) -> Vec<Achievement> {
    vec![
        rank(&STREAK, current_streak as u64),
        rank(&TOKENS, total_tokens),
        rank_cache(&CACHE, cache_rate),
        rank(&MODELS, models as u64),
        rank(&CLIENTS, clients as u64),
    ]
}

pub(super) fn lines(theme: &Theme, achievements: &[Achievement]) -> Vec<Line<'static>> {
    let mut lines = Vec::with_capacity(achievements.len() + 2);
    lines.push(Line::from(Span::styled(
        rust_i18n::t!("tui.ui.achievements.title"),
        Style::default()
            .fg(theme.text.primary)
            .add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::default());
    lines.extend(
        achievements
            .iter()
            .map(|achievement| ladder_line(theme, achievement)),
    );
    lines
}

fn ladder_line(theme: &Theme, achievement: &Achievement) -> Line<'static> {
    let locked = achievement.current < 0;
    let title_style = Style::default()
        .fg(theme.text.primary)
        .add_modifier(Modifier::BOLD);
    let title_pad = TITLE_WIDTH.saturating_sub(text_width(&achievement.title));

    let mut spans = vec![
        Span::styled(achievement.title.to_string(), title_style),
        Span::raw(" ".repeat(title_pad + 1)),
    ];
    for (index, display) in achievement.ladder.iter().enumerate() {
        let tier = index as i8;
        if tier == achievement.current {
            spans.push(Span::styled(
                format!("[{display}]"),
                Style::default()
                    .fg(theme.status.success)
                    .add_modifier(Modifier::BOLD),
            ));
        } else if !locked && tier < achievement.current {
            spans.push(Span::styled(
                display.to_string(),
                Style::default().fg(theme.status.success),
            ));
        } else {
            spans.push(Span::styled(
                display.to_string(),
                Style::default().fg(theme.text.secondary),
            ));
        }
        spans.push(Span::raw(" "));
    }
    Line::from(spans)
}

/// Display width limited to what the ladders need (CJK counts double).
fn text_width(text: &str) -> usize {
    width(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::themes::ThemeName;

    fn theme() -> Theme {
        Theme::from_name(ThemeName::Blue)
    }

    fn en(key: &'static str) -> Cow<'static, str> {
        rust_i18n::t!(key, locale = "en")
    }

    #[test]
    fn rank_picks_the_highest_reached_tier_or_roast() {
        assert_eq!(
            rank(&STREAK, 0).title,
            en("tui.ui.achievements.streak.roast")
        );
        assert_eq!(rank(&STREAK, 0).current, -1);
        assert_eq!(
            rank(&STREAK, 7).title,
            en("tui.ui.achievements.streak.tier1")
        );
        assert_eq!(
            rank(&STREAK, 179).title,
            en("tui.ui.achievements.streak.tier4")
        );
        assert_eq!(
            rank(&STREAK, 180).title,
            en("tui.ui.achievements.streak.tier5")
        );
        assert_eq!(
            rank(&TOKENS, 12_000_000_000).title,
            en("tui.ui.achievements.tokens.tier3")
        );
        assert_eq!(
            rank(&MODELS, 67).title,
            en("tui.ui.achievements.models.tier3")
        );
        assert_eq!(
            rank(&CACHE, 91).title,
            en("tui.ui.achievements.cache.tier3")
        );
        assert_eq!(
            rank(&CLIENTS, 15).title,
            en("tui.ui.achievements.clients.tier4")
        );
        assert_eq!(
            rank(&CLIENTS, 20).title,
            en("tui.ui.achievements.clients.tier5")
        );
    }

    #[test]
    fn build_uses_the_authoritative_current_streak() {
        let achievements = build(30, 0, CacheRate::default(), 0, 0);

        assert_eq!(
            achievements[0].title,
            en("tui.ui.achievements.streak.tier3")
        );
        assert_eq!(achievements[0].current, 2);
    }

    #[test]
    fn cache_tier_uses_the_same_tenth_percent_as_the_display() {
        let rounded_to_fifty = build(0, 0, CacheRate::from_tokens(4_996, 10_000), 0, 0);
        let still_below_fifty = build(0, 0, CacheRate::from_tokens(4_994, 10_000), 0, 0);

        assert_eq!(
            rounded_to_fifty[2].title,
            en("tui.ui.achievements.cache.tier1")
        );
        assert_eq!(rounded_to_fifty[2].current, 0);
        assert_eq!(
            still_below_fifty[2].title,
            en("tui.ui.achievements.cache.roast")
        );
        assert_eq!(still_below_fifty[2].current, -1);
    }

    #[test]
    fn every_below_threshold_achievement_keeps_its_roast_title() {
        let achievements = build(0, 0, CacheRate::default(), 0, 0);

        assert_eq!(
            achievements
                .iter()
                .map(|achievement| (achievement.title.clone(), achievement.current))
                .collect::<Vec<_>>(),
            vec![
                (en("tui.ui.achievements.streak.roast"), -1),
                (en("tui.ui.achievements.tokens.roast"), -1),
                (en("tui.ui.achievements.cache.roast"), -1),
                (en("tui.ui.achievements.models.roast"), -1),
                (en("tui.ui.achievements.clients.roast"), -1),
            ]
        );
    }

    #[test]
    fn colors_encode_ladder_progress_without_highlighting_locked_titles() {
        let theme = theme();
        let locked = rank(&STREAK, 0);
        let unlocked = rank(&STREAK, 15);
        let locked_line = ladder_line(&theme, &locked);
        let unlocked_line = ladder_line(&theme, &unlocked);

        assert_eq!(locked_line.spans[0].style, unlocked_line.spans[0].style);
        assert_eq!(locked_line.spans[0].style.fg, Some(theme.text.primary));
        assert!(locked_line.spans[0]
            .style
            .add_modifier
            .contains(Modifier::BOLD));

        for tier_span in locked_line.spans.iter().skip(2).step_by(2) {
            assert_eq!(tier_span.style.fg, Some(theme.text.secondary));
            assert!(!tier_span.style.add_modifier.contains(Modifier::BOLD));
        }

        assert_eq!(unlocked_line.spans[2].style.fg, Some(theme.status.success));
        assert!(!unlocked_line.spans[2]
            .style
            .add_modifier
            .contains(Modifier::BOLD));
        assert_eq!(unlocked_line.spans[4].content.as_ref(), "[15]");
        assert_eq!(unlocked_line.spans[4].style.fg, Some(theme.status.success));
        assert!(unlocked_line.spans[4]
            .style
            .add_modifier
            .contains(Modifier::BOLD));
        for tier_span in unlocked_line.spans.iter().skip(6).step_by(2) {
            assert_eq!(tier_span.style.fg, Some(theme.text.secondary));
        }
    }
}
