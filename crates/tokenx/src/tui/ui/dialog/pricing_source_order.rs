use super::{DialogContent, DialogResult, UiCommand};
use crate::tui::interaction::{ListInteraction, MoveCommand, WrapMode};
use crate::tui::themes::Theme;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::Style,
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph},
    Frame,
};
use tokenx_engine::pricing::SourceOrder;

pub struct PricingSourceOrderDialog {
    draft: SourceOrder,
    cursor: usize,
}

impl PricingSourceOrderDialog {
    pub fn new(order: SourceOrder) -> Self {
        Self {
            draft: order,
            cursor: 0,
        }
    }
    fn move_cursor(&mut self, command: MoveCommand) {
        let mut interaction = ListInteraction {
            selected: self.cursor,
            visible: 3,
            ..Default::default()
        };
        let _ = interaction.apply_move(command, 3, WrapMode::Wrap);
        self.cursor = interaction.selected;
    }
    fn swap(&mut self, delta: isize) {
        let target = self.cursor as isize + delta;
        if !(0..3).contains(&target) {
            return;
        }
        self.draft.swap(self.cursor, target as usize);
        self.cursor = target as usize;
    }
}

impl DialogContent for PricingSourceOrderDialog {
    fn desired_size(&self, viewport: Rect) -> (u16, u16) {
        (
            64.min(viewport.width.saturating_sub(4)),
            14.min(viewport.height.saturating_sub(4)),
        )
    }
    fn render(&self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let block = Block::default()
            .title(rust_i18n::t!("tui.ui.dialog.pricing_sources.title"))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.chrome.focus));
        frame.render_widget(block, area);
        let inner = Block::default().borders(Borders::ALL).inner(area);
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Length(4),
                Constraint::Min(3),
                Constraint::Length(2),
            ])
            .split(inner);
        frame.render_widget(
            Paragraph::new(rust_i18n::t!("tui.ui.dialog.pricing_sources.priority")),
            chunks[0],
        );
        frame.render_widget(
            Paragraph::new(rust_i18n::t!("tui.ui.dialog.pricing_sources.deepseek")),
            chunks[1],
        );
        frame.render_widget(
            Paragraph::new(rust_i18n::t!("tui.ui.dialog.pricing_sources.description")),
            chunks[2],
        );
        let rows: Vec<ListItem> = self
            .draft
            .sources()
            .iter()
            .enumerate()
            .map(|(i, source)| {
                let line = Line::from(vec![Span::styled(
                    format!("{}  {}", i + 1, source.label()),
                    if i == self.cursor {
                        theme.selection_style()
                    } else {
                        Style::default().fg(theme.text.primary)
                    },
                )]);
                ListItem::new(line)
            })
            .collect();
        let list = List::new(rows);
        frame.render_widget(
            list,
            Rect::new(chunks[3].x, chunks[3].y, inner.width, chunks[3].height),
        );
        frame.render_widget(
            Paragraph::new(rust_i18n::t!("tui.ui.dialog.pricing_sources.hint")),
            Rect::new(chunks[4].x, chunks[4].y, inner.width, chunks[4].height),
        );
    }
    fn handle_key(&mut self, key: KeyEvent) -> DialogResult {
        match key.code {
            KeyCode::Up => {
                self.move_cursor(MoveCommand::Up);
                DialogResult::Handled
            }
            KeyCode::Down => {
                self.move_cursor(MoveCommand::Down);
                DialogResult::Handled
            }
            KeyCode::Char('k') => {
                self.swap(-1);
                DialogResult::Handled
            }
            KeyCode::Char('j') => {
                self.swap(1);
                DialogResult::Handled
            }
            KeyCode::Char('r') => {
                self.draft = SourceOrder::default();
                self.cursor = 0;
                DialogResult::Handled
            }
            KeyCode::Enter => DialogResult::Submit(UiCommand::PricingSourceOrder(self.draft)),
            _ => DialogResult::Ignored("unhandled key"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, crossterm::event::KeyModifiers::NONE)
    }

    #[test]
    fn moving_and_submitting_returns_the_reordered_sources() {
        let mut dialog = PricingSourceOrderDialog::new(SourceOrder::default());
        assert_eq!(
            dialog.handle_key(key(KeyCode::Char('j'))),
            DialogResult::Handled
        );
        let result = dialog.handle_key(key(KeyCode::Enter));
        let DialogResult::Submit(UiCommand::PricingSourceOrder(order)) = result else {
            panic!("expected pricing source order submission");
        };
        assert_eq!(
            order.sources()[0],
            tokenx_engine::pricing::CatalogSource::Openrouter
        );
        assert_eq!(
            order.sources()[1],
            tokenx_engine::pricing::CatalogSource::Litellm
        );
    }

    #[test]
    fn reset_restores_default_order() {
        let mut dialog = PricingSourceOrderDialog::new(SourceOrder::default());
        dialog.handle_key(key(KeyCode::Char('j')));
        dialog.handle_key(key(KeyCode::Char('r')));
        let DialogResult::Submit(UiCommand::PricingSourceOrder(order)) =
            dialog.handle_key(key(KeyCode::Enter))
        else {
            panic!("expected pricing source order submission");
        };
        assert_eq!(order, SourceOrder::default());
    }
}
