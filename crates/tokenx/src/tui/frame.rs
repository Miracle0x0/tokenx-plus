//! Coherent TUI frame lifecycle.
//!
//! The event loop owns one [`TuiFrame`] and no longer coordinates product
//! state, page state, capability classification, reconciliation, and
//! rendering as separate protocols. This module is the sole ordering
//! authority for those operations.

use crossterm::event::{KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use ratatui::Frame;

use super::actions::ActionSet;
use super::intent::Intent;
use super::model::{KeyEventOutcome, TuiModel};
use super::page_state::PageStates;
use super::presentation::Presentation;
use super::render_artifacts::RenderArtifacts;
use super::ui;

pub(crate) struct TuiFrame {
    model: TuiModel,
    pages: PageStates,
    artifacts: RenderArtifacts,
}

impl TuiFrame {
    pub(crate) fn new(model: TuiModel) -> Self {
        Self {
            model,
            pages: PageStates::default(),
            artifacts: RenderArtifacts::default(),
        }
    }

    pub(crate) fn model(&self) -> &TuiModel {
        &self.model
    }

    pub(crate) fn model_mut(&mut self) -> &mut TuiModel {
        &mut self.model
    }

    #[cfg(test)]
    pub(crate) fn pages(&self) -> &PageStates {
        &self.pages
    }

    #[cfg(test)]
    pub(crate) fn pages_mut(&mut self) -> &mut PageStates {
        &mut self.pages
    }

    #[cfg(test)]
    pub(crate) fn parts_mut(&mut self) -> (&mut TuiModel, &mut PageStates) {
        (&mut self.model, &mut self.pages)
    }

    pub(crate) fn render(&mut self, frame: &mut Frame) {
        let area = frame.area();
        self.model.handle_resize(area.width, area.height);
        let mut artifacts = RenderArtifacts::default();
        ui::render_with_state(frame, &self.model, &self.pages, &mut artifacts);
        self.model.install_render_measurements(&artifacts);
        self.pages.install_render_measurements(&artifacts);
        self.artifacts = artifacts;
    }

    pub(crate) fn reconcile_generation(&mut self) {
        self.pages.reconcile_session_snapshot(&self.model);
    }

    pub(crate) fn handle_key(&mut self, key: KeyEvent) -> KeyEventOutcome {
        let outcome = self.transition_key(key);
        self.flush_effects();
        outcome
    }

    fn transition_key(&mut self, key: KeyEvent) -> KeyEventOutcome {
        let Some(intent) = Intent::from_key(self.model.current_tab, key) else {
            if self.model.dialog_stack.is_active() {
                return self.model.handle_dialog_key(key);
            }
            return KeyEventOutcome::Continue;
        };

        if intent == Intent::Interrupt {
            return self.model.apply_intent(intent);
        }

        if self.model.dialog_stack.is_active() {
            let outcome = self.model.handle_dialog_key(key);
            if !self.model.dialog_stack.is_active() {
                self.reconcile_generation();
            }
            return outcome;
        }

        self.dispatch_intent(intent)
    }

    fn dispatch_intent(&mut self, intent: Intent) -> KeyEventOutcome {
        // The same capability set drives both advertised shortcuts and
        // dispatch, so an empty table cannot accept a decorative command.
        let presentation = Presentation::for_view(&self.model, &self.pages);
        let actions = ActionSet::for_view(&self.model, &self.pages, presentation);
        if intent
            .action()
            .is_some_and(|action| !actions.contains(action))
        {
            return KeyEventOutcome::Continue;
        }

        if self.pages.handle_intent(&mut self.model, intent) {
            return KeyEventOutcome::Continue;
        }

        let outcome = self.model.apply_intent(intent);
        if !self.model.dialog_stack.is_active() {
            self.reconcile_generation();
        }
        outcome
    }

    pub(crate) fn handle_mouse(&mut self, event: MouseEvent) {
        self.transition_mouse(event);
        self.flush_effects();
    }

    fn transition_mouse(&mut self, event: MouseEvent) {
        if self.model.dialog_stack.is_active() {
            self.model
                .handle_dialog_mouse(event, self.artifacts.dialog_rect());
            if !self.model.dialog_stack.is_active() {
                self.reconcile_generation();
            }
            return;
        }

        let intent = match event.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                self.artifacts.intent_at(event.column, event.row)
            }
            MouseEventKind::ScrollUp => Some(Intent::Move(super::interaction::MoveCommand::Up)),
            MouseEventKind::ScrollDown => Some(Intent::Move(super::interaction::MoveCommand::Down)),
            _ => None,
        };
        if let Some(intent) = intent {
            let _ = self.dispatch_intent(intent);
        }
    }

    pub(crate) fn on_tick(&mut self) {
        self.model.on_tick();
        self.flush_effects();
    }

    pub(crate) fn flush_effects(&mut self) {
        super::effect::execute_pending(&mut self.model);
    }

    #[cfg(test)]
    pub(crate) fn artifacts(&self) -> &RenderArtifacts {
        &self.artifacts
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::KeyModifiers;
    use ratatui::{backend::TestBackend, Terminal};

    use super::*;
    use crate::settings::Settings;
    use crate::tui::model::{Tab, TuiConfig};

    fn frame() -> TuiFrame {
        let mut model = TuiModel::new_for_test_with_settings(
            TuiConfig {
                theme: Some(crate::theme::ThemeName::Blue),
                refresh: 0,
                no_refresh: true,
                client_universe: tokenx_engine::ClientUniverse::all(),
                initial_tab: Some(Tab::Overview),
                effective_date: chrono::NaiveDate::from_ymd_opt(2026, 7, 26).unwrap(),
            },
            Settings::default(),
        )
        .unwrap();
        model.install_generation_fixture(
            tokenx_engine::FrozenUsageIndex::new(),
            Vec::new(),
            Default::default(),
        );
        TuiFrame::new(model)
    }

    #[test]
    fn rendered_tab_hit_target_dispatches_through_the_frame() {
        let mut tui = frame();
        let mut terminal = Terminal::new(TestBackend::new(120, 30)).unwrap();
        terminal.draw(|frame| tui.render(frame)).unwrap();

        let target = tui
            .artifacts()
            .hit_targets()
            .iter()
            .find(|target| target.intent == Intent::SelectTab(Tab::Models))
            .copied()
            .expect("rendered Models tab must install a hit target");
        tui.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: target.rect.x + target.rect.width / 2,
            row: target.rect.y,
            modifiers: KeyModifiers::NONE,
        });

        assert_eq!(tui.model().current_tab, Tab::Models);
    }

    #[test]
    fn click_outside_the_last_rendered_frame_is_a_noop() {
        let mut tui = frame();
        let mut terminal = Terminal::new(TestBackend::new(120, 30)).unwrap();
        terminal.draw(|frame| tui.render(frame)).unwrap();

        tui.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 119,
            row: 29,
            modifiers: KeyModifiers::NONE,
        });

        assert_eq!(tui.model().current_tab, Tab::Overview);
    }

    #[test]
    fn dialog_mouse_routing_uses_the_rectangle_from_the_rendered_frame() {
        let mut tui = frame();
        let mut terminal = Terminal::new(TestBackend::new(120, 30)).unwrap();
        tui.handle_key(KeyEvent::new(
            crossterm::event::KeyCode::Char('s'),
            KeyModifiers::NONE,
        ));
        assert!(tui.model().dialog_stack.is_active());

        terminal.draw(|frame| tui.render(frame)).unwrap();
        let dialog = tui
            .artifacts()
            .dialog_rect()
            .expect("active rendered dialog must publish its rectangle");
        let column = dialog.x.saturating_sub(1);
        let row = dialog.y.saturating_sub(1);
        tui.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column,
            row,
            modifiers: KeyModifiers::NONE,
        });

        assert!(!tui.model().dialog_stack.is_active());
    }
}
