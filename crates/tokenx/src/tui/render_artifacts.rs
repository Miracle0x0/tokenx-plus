//! Ephemeral output produced by one render pass.
//!
//! Hit targets describe pixels from the frame that was actually drawn. They
//! are replaced atomically after rendering and never become durable product
//! state, which prevents stale geometry from leaking across resize or page
//! transitions.

use ratatui::layout::Rect;

use super::intent::Intent;
use super::interaction::{ListInteraction, TextViewport};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TextMeasurement {
    pub(crate) viewport: TextViewport,
    pub(crate) total_lines: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct HitTarget {
    pub(crate) rect: Rect,
    pub(crate) intent: Intent,
}

#[derive(Debug, Default)]
pub(crate) struct RenderArtifacts {
    hit_targets: Vec<HitTarget>,
    dialog_rect: Option<Rect>,
    main_list: Option<ListInteraction>,
    daily_profile: Option<TextMeasurement>,
    hourly_profile: Option<TextMeasurement>,
    subscription: Option<TextMeasurement>,
    session_clients: Option<ListInteraction>,
    session_details: Option<ListInteraction>,
}

impl RenderArtifacts {
    pub(crate) fn add_hit_target(&mut self, rect: Rect, intent: Intent) {
        if !rect.is_empty() {
            self.hit_targets.push(HitTarget { rect, intent });
        }
    }

    pub(crate) fn intent_at(&self, column: u16, row: u16) -> Option<Intent> {
        self.hit_targets
            .iter()
            .find(|target| target.rect.contains((column, row).into()))
            .map(|target| target.intent)
    }

    pub(crate) fn set_dialog_rect(&mut self, rect: Option<Rect>) {
        self.dialog_rect = rect;
    }

    pub(crate) fn dialog_rect(&self) -> Option<Rect> {
        self.dialog_rect
    }

    pub(crate) fn measure_main_list(
        &mut self,
        current: ListInteraction,
        visible: usize,
        len: usize,
    ) -> ListInteraction {
        let mut measured = self.main_list.unwrap_or(current);
        measured.set_visible(visible, len);
        self.main_list = Some(measured);
        measured
    }

    pub(crate) fn main_list(&self) -> Option<ListInteraction> {
        self.main_list
    }

    pub(crate) fn measure_daily_profile(
        &mut self,
        current: TextViewport,
        visible: usize,
        total_lines: usize,
    ) -> TextViewport {
        measure_text(&mut self.daily_profile, current, visible, total_lines)
    }

    pub(crate) fn daily_profile(&self) -> Option<TextMeasurement> {
        self.daily_profile
    }

    pub(crate) fn measure_hourly_profile(
        &mut self,
        current: TextViewport,
        visible: usize,
        total_lines: usize,
    ) -> TextViewport {
        measure_text(&mut self.hourly_profile, current, visible, total_lines)
    }

    pub(crate) fn hourly_profile(&self) -> Option<TextMeasurement> {
        self.hourly_profile
    }

    pub(crate) fn measure_subscription(
        &mut self,
        current: TextViewport,
        visible: usize,
        total_lines: usize,
    ) -> TextViewport {
        measure_text(&mut self.subscription, current, visible, total_lines)
    }

    pub(crate) fn subscription(&self) -> Option<TextMeasurement> {
        self.subscription
    }

    pub(crate) fn measure_session_clients(
        &mut self,
        current: ListInteraction,
        visible: usize,
        len: usize,
    ) -> ListInteraction {
        measure_list(&mut self.session_clients, current, visible, len)
    }

    pub(crate) fn session_clients(&self) -> Option<ListInteraction> {
        self.session_clients
    }

    pub(crate) fn measure_session_details(
        &mut self,
        current: ListInteraction,
        visible: usize,
        len: usize,
    ) -> ListInteraction {
        measure_list(&mut self.session_details, current, visible, len)
    }

    pub(crate) fn session_details(&self) -> Option<ListInteraction> {
        self.session_details
    }

    #[cfg(test)]
    pub(crate) fn hit_targets(&self) -> &[HitTarget] {
        &self.hit_targets
    }
}

fn measure_text(
    slot: &mut Option<TextMeasurement>,
    current: TextViewport,
    visible: usize,
    total_lines: usize,
) -> TextViewport {
    let mut viewport = slot.map_or(current, |measurement| measurement.viewport);
    viewport.set_visible(visible, total_lines);
    *slot = Some(TextMeasurement {
        viewport,
        total_lines,
    });
    viewport
}

fn measure_list(
    slot: &mut Option<ListInteraction>,
    current: ListInteraction,
    visible: usize,
    len: usize,
) -> ListInteraction {
    let mut interaction = slot.unwrap_or(current);
    interaction.set_visible(visible, len);
    *slot = Some(interaction);
    interaction
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::model::{SortField, Tab};

    #[test]
    fn hit_targets_resolve_only_inside_the_drawn_rectangle() {
        let mut artifacts = RenderArtifacts::default();
        artifacts.add_hit_target(
            Rect::new(10, 5, 3, 2),
            Intent::SelectGraphCell { week: 2, day: 3 },
        );

        assert_eq!(
            artifacts.intent_at(11, 6),
            Some(Intent::SelectGraphCell { week: 2, day: 3 })
        );
        assert_eq!(artifacts.intent_at(13, 6), None);
        assert_eq!(artifacts.intent_at(11, 7), None);
    }

    #[test]
    fn zero_sized_and_stale_targets_do_not_survive_a_new_frame() {
        let mut previous = RenderArtifacts::default();
        previous.add_hit_target(Rect::new(0, 0, 5, 1), Intent::SelectTab(Tab::Models));
        previous.add_hit_target(Rect::new(5, 0, 0, 1), Intent::Sort(SortField::Tokens));
        assert_eq!(previous.hit_targets().len(), 1);

        let next = RenderArtifacts::default();
        assert_eq!(next.intent_at(1, 0), None);
    }
}
