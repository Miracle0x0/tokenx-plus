//! Page-owned interaction state.
//!
//! Page state is deliberately separate from the durable product model so one
//! view cannot reinterpret another view's selection, scroll, or detail state.

use std::cell::RefCell;
use std::cmp::Ordering;
#[cfg(test)]
use std::ops::Range;
use std::sync::Arc;

use super::intent::Intent;
use super::interaction::{ListInteraction, MoveCommand, TextViewport, WrapMode};
use super::model::{ChartGranularity, HourlyViewMode, SortDirection, SortField, Tab, TuiModel};
use super::render_artifacts::RenderArtifacts;
use super::session_data::ClientSummary;
#[cfg(test)]
use super::session_data::SessionEntry;
use tokenx_engine::ClientId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SessionOrderKey {
    snapshot_revision: u64,
    client: ClientId,
    sort_field: SortField,
    sort_direction: SortDirection,
}

#[derive(Debug, Default)]
struct SessionOrderCache {
    key: Option<SessionOrderKey>,
    order: Arc<[usize]>,
}

#[derive(Debug, Default)]
pub(crate) struct PageStates {
    overview_granularity: ChartGranularity,
    hourly_mode: HourlyViewMode,
    hourly_profile_viewport: TextViewport,
    hourly_profile_total_lines: usize,
    subscription_viewport: TextViewport,
    subscription_total_lines: usize,
    daily_profile: bool,
    daily_profile_viewport: TextViewport,
    daily_profile_total_lines: usize,
    selected_session_client: Option<ClientId>,
    session_clients: ListInteraction,
    session_details: ListInteraction,
    session_order_cache: RefCell<SessionOrderCache>,
}

impl PageStates {
    pub(crate) fn handle_intent(&mut self, app: &mut TuiModel, intent: Intent) -> bool {
        if app.dialog_stack.is_active() {
            return false;
        }

        match (app.current_tab, intent) {
            (Tab::Overview, Intent::ToggleView) => {
                self.overview_granularity = match self.overview_granularity {
                    ChartGranularity::Daily => ChartGranularity::Hourly,
                    ChartGranularity::Hourly => ChartGranularity::Daily,
                };
                return true;
            }
            (Tab::Hourly, Intent::ToggleView) => {
                self.hourly_mode = match self.hourly_mode {
                    HourlyViewMode::Table => HourlyViewMode::Profile,
                    HourlyViewMode::Profile => HourlyViewMode::Table,
                };
                app.reset_current_list_interaction();
                self.hourly_profile_viewport.scroll = 0;
                return true;
            }
            (Tab::Subscription, Intent::Move(command)) => {
                self.subscription_viewport
                    .apply_move(command, self.subscription_total_lines);
                return true;
            }
            (Tab::Hourly, Intent::Move(command)) if self.hourly_mode == HourlyViewMode::Profile => {
                self.hourly_profile_viewport
                    .apply_move(command, self.hourly_profile_total_lines);
                return true;
            }
            _ => {}
        }

        if app.current_tab == Tab::Daily && !app.is_daily_detail_active() {
            match intent {
                Intent::ToggleView => {
                    self.daily_profile = !self.daily_profile;
                    return true;
                }
                Intent::Move(command) if self.daily_profile => {
                    self.move_daily_profile(command);
                    return true;
                }
                Intent::OpenDetails if self.daily_profile => {
                    return true;
                }
                _ => {}
            }
        }

        if app.current_tab != Tab::Sessions {
            return false;
        }

        match intent {
            Intent::Move(command) => {
                self.move_session_selection(app, command);
                true
            }
            Intent::OpenDetails if !self.session_detail_active() => {
                if let Some(client) = self.selected_client_row(app).map(|row| row.client) {
                    self.selected_session_client = Some(client);
                    self.session_details = ListInteraction::default();
                }
                true
            }
            Intent::Back if self.session_detail_active() => {
                self.selected_session_client = None;
                true
            }
            _ => false,
        }
    }

    fn move_session_selection(&mut self, app: &TuiModel, command: MoveCommand) {
        let detail_active = self.session_detail_active();
        let len = if detail_active {
            self.session_count(app)
        } else {
            self.client_count(app)
        };
        let interaction = if detail_active {
            &mut self.session_details
        } else {
            &mut self.session_clients
        };
        interaction.apply_move(command, len, WrapMode::Wrap);
    }

    fn move_daily_profile(&mut self, command: MoveCommand) {
        self.daily_profile_viewport
            .apply_move(command, self.daily_profile_total_lines);
    }

    pub(crate) fn overview_granularity(&self) -> ChartGranularity {
        self.overview_granularity
    }

    pub(crate) fn hourly_mode(&self) -> HourlyViewMode {
        self.hourly_mode
    }

    #[cfg(test)]
    pub(crate) fn set_hourly_mode_for_test(&mut self, mode: HourlyViewMode) {
        self.hourly_mode = mode;
    }

    #[cfg(test)]
    pub(crate) fn set_subscription_text_viewport(&mut self, visible: usize, total_lines: usize) {
        self.subscription_total_lines = total_lines;
        self.subscription_viewport.set_visible(visible, total_lines);
    }

    #[cfg(test)]
    pub(crate) fn subscription_text_visible_range(&self) -> Range<usize> {
        self.subscription_viewport
            .visible_range(self.subscription_total_lines)
    }

    #[cfg(test)]
    pub(crate) fn subscription_scroll(&self) -> usize {
        self.subscription_viewport.scroll
    }

    #[cfg(test)]
    pub(crate) fn set_hourly_profile_text_viewport(&mut self, visible: usize, total_lines: usize) {
        self.hourly_profile_total_lines = total_lines;
        self.hourly_profile_viewport
            .set_visible(visible, total_lines);
    }

    #[cfg(test)]
    pub(crate) fn hourly_profile_text_visible_range(&self) -> Range<usize> {
        self.hourly_profile_viewport
            .visible_range(self.hourly_profile_total_lines)
    }

    #[cfg(test)]
    pub(crate) fn hourly_profile_scroll(&self) -> usize {
        self.hourly_profile_viewport.scroll
    }

    pub(crate) fn hourly_profile_viewport(&self) -> TextViewport {
        self.hourly_profile_viewport
    }

    pub(crate) fn subscription_viewport(&self) -> TextViewport {
        self.subscription_viewport
    }

    pub(crate) fn daily_profile_viewport(&self) -> TextViewport {
        self.daily_profile_viewport
    }

    pub(crate) fn daily_profile_active(&self) -> bool {
        self.daily_profile
    }

    #[cfg(test)]
    pub(crate) fn set_daily_profile_text_viewport(&mut self, visible: usize, total_lines: usize) {
        self.daily_profile_total_lines = total_lines;
        self.daily_profile_viewport
            .set_visible(visible, total_lines);
    }

    #[cfg(test)]
    pub(crate) fn daily_profile_text_visible_range(&self) -> Range<usize> {
        self.daily_profile_viewport
            .visible_range(self.daily_profile_total_lines)
    }

    #[cfg(test)]
    pub(crate) fn daily_profile_scroll(&self) -> usize {
        self.daily_profile_viewport.scroll
    }

    pub(crate) fn session_detail_active(&self) -> bool {
        self.selected_session_client.is_some()
    }

    pub(crate) fn selected_session_client(&self) -> Option<ClientId> {
        self.selected_session_client
    }

    #[cfg(test)]
    pub(crate) fn select_session_client_for_test(&mut self, client: ClientId) {
        self.selected_session_client = Some(client);
    }

    pub(crate) fn client_count(&self, app: &TuiModel) -> usize {
        app.session_snapshot()
            .client_summaries()
            .iter()
            .filter(|summary| app.is_client_selected(summary.client))
            .count()
    }

    pub(crate) fn session_count(&self, app: &TuiModel) -> usize {
        let snapshot = app.session_snapshot();
        self.selected_session_client.map_or_else(
            || {
                snapshot
                    .client_summaries()
                    .iter()
                    .filter(|summary| app.is_client_selected(summary.client))
                    .map(|summary| summary.session_count)
                    .sum()
            },
            |client| {
                if app.is_client_selected(client) {
                    snapshot.session_count_for_client(client)
                } else {
                    0
                }
            },
        )
    }

    pub(crate) fn client_rows<'a>(&self, app: &'a TuiModel) -> Vec<&'a ClientSummary> {
        let mut rows = self.visible_client_rows(app);
        rows.sort_by(|left, right| compare_client_rows(app, left, right));
        rows
    }

    pub(crate) fn selected_client_row<'a>(&self, app: &'a TuiModel) -> Option<&'a ClientSummary> {
        let selected = self.session_clients.selected;
        let mut rows = self.visible_client_rows(app);
        if selected >= rows.len() {
            return None;
        }

        let (_, row, _) = rows.select_nth_unstable_by(selected, |left, right| {
            compare_client_rows(app, left, right)
        });
        Some(*row)
    }

    fn visible_client_rows<'a>(&self, app: &'a TuiModel) -> Vec<&'a ClientSummary> {
        app.session_snapshot()
            .client_summaries()
            .iter()
            .filter(|summary| app.is_client_selected(summary.client))
            .collect()
    }

    pub(crate) fn session_order(&self, app: &TuiModel) -> Arc<[usize]> {
        let Some(client) = self.selected_session_client else {
            return Arc::from([]);
        };
        if !app.is_client_selected(client) {
            return Arc::from([]);
        }
        let snapshot = app.session_snapshot();
        let key = SessionOrderKey {
            snapshot_revision: snapshot.revision(),
            client,
            sort_field: app.sort_field,
            sort_direction: app.sort_direction,
        };
        if self.session_order_cache.borrow().key == Some(key) {
            return Arc::clone(&self.session_order_cache.borrow().order);
        }

        let mut order = snapshot.session_indices_for_client(client).to_vec();
        order.sort_by(|left_index, right_index| {
            let left = snapshot
                .session(*left_index)
                .expect("session client index must reference the snapshot");
            let right = snapshot
                .session(*right_index)
                .expect("session client index must reference the snapshot");
            let ordering = match app.sort_field {
                SortField::Date => left.last_seen.cmp(&right.last_seen),
                SortField::Tokens => left.tokens.total().cmp(&right.tokens.total()),
                SortField::Cost => left.cost.total_cmp(&right.cost),
            };
            apply_direction(ordering, app.sort_direction)
                .then_with(|| left.session_id.cmp(&right.session_id))
        });
        let order: Arc<[usize]> = order.into();
        *self.session_order_cache.borrow_mut() = SessionOrderCache {
            key: Some(key),
            order: Arc::clone(&order),
        };
        order
    }

    #[cfg(test)]
    pub(crate) fn session_rows<'a>(&mut self, app: &'a TuiModel) -> Vec<&'a SessionEntry> {
        self.session_order(app)
            .iter()
            .map(|index| {
                app.session_snapshot()
                    .session(*index)
                    .expect("cached session index must reference the snapshot")
            })
            .collect()
    }

    pub(crate) fn reconcile_session_snapshot(&mut self, app: &TuiModel) {
        if !app.has_installed_generation() {
            self.selected_session_client = None;
            self.session_clients = ListInteraction::default();
            self.session_details = ListInteraction::default();
            self.session_order_cache = RefCell::new(SessionOrderCache::default());
            return;
        }

        if self.selected_session_client.is_some_and(|selected| {
            !app.session_snapshot()
                .client_summaries()
                .iter()
                .any(|summary| summary.client == selected)
                || !app.is_client_selected(selected)
                || app.session_snapshot().session_count_for_client(selected) == 0
        }) {
            self.selected_session_client = None;
            self.session_details = ListInteraction::default();
        }

        self.session_clients.clamp(self.client_count(app));
        self.session_details.clamp(self.session_count(app));
    }

    #[cfg(test)]
    pub(crate) fn set_client_viewport(&mut self, visible: usize, len: usize) {
        self.session_clients.set_visible(visible, len);
    }

    #[cfg(test)]
    pub(crate) fn set_detail_viewport(&mut self, visible: usize, len: usize) {
        self.session_details.set_visible(visible, len);
    }

    #[cfg(test)]
    pub(crate) fn client_selected(&self) -> usize {
        self.session_clients.selected
    }

    #[cfg(test)]
    pub(crate) fn detail_selected(&self) -> usize {
        self.session_details.selected
    }

    #[cfg(test)]
    pub(crate) fn client_scroll(&self) -> usize {
        self.session_clients.scroll
    }

    #[cfg(test)]
    pub(crate) fn detail_scroll(&self) -> usize {
        self.session_details.scroll
    }

    #[cfg(test)]
    pub(crate) fn client_visible_range(&self, len: usize) -> Range<usize> {
        self.session_clients.visible_range(len)
    }

    #[cfg(test)]
    pub(crate) fn detail_visible_range(&self, len: usize) -> Range<usize> {
        self.session_details.visible_range(len)
    }

    pub(crate) fn session_clients_interaction(&self) -> ListInteraction {
        self.session_clients
    }

    pub(crate) fn session_details_interaction(&self) -> ListInteraction {
        self.session_details
    }

    pub(crate) fn install_render_measurements(&mut self, artifacts: &RenderArtifacts) {
        if let Some(measurement) = artifacts.daily_profile() {
            self.daily_profile_viewport = measurement.viewport;
            self.daily_profile_total_lines = measurement.total_lines;
        }
        if let Some(measurement) = artifacts.hourly_profile() {
            self.hourly_profile_viewport = measurement.viewport;
            self.hourly_profile_total_lines = measurement.total_lines;
        }
        if let Some(measurement) = artifacts.subscription() {
            self.subscription_viewport = measurement.viewport;
            self.subscription_total_lines = measurement.total_lines;
        }
        if let Some(interaction) = artifacts.session_clients() {
            self.session_clients = interaction;
        }
        if let Some(interaction) = artifacts.session_details() {
            self.session_details = interaction;
        }
    }
}

fn apply_direction(ordering: Ordering, direction: SortDirection) -> Ordering {
    match direction {
        SortDirection::Ascending => ordering,
        SortDirection::Descending => ordering.reverse(),
    }
}

fn compare_client_rows(app: &TuiModel, left: &ClientSummary, right: &ClientSummary) -> Ordering {
    let ordering = match app.sort_field {
        SortField::Date => left.last_seen.cmp(&right.last_seen),
        SortField::Tokens => left.session_count.cmp(&right.session_count),
        SortField::Cost => left.space_bytes.cmp(&right.space_bytes),
    };
    apply_direction(ordering, app.sort_direction).then_with(|| left.client.cmp(&right.client))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::model::TuiConfig;
    use crate::tui::session_data::SessionSnapshot;
    use crate::tui::themes::ThemeName;
    use std::collections::HashSet;
    use tokenx_engine::InputFootprint;

    fn app_with_sessions(sessions: Vec<SessionEntry>) -> TuiModel {
        let mut app = TuiModel::new_for_test(TuiConfig {
            theme: Some(ThemeName::Blue),
            refresh: 0,
            no_refresh: true,
            client_universe: tokenx_engine::ClientUniverse::all(),
            initial_tab: Some(Tab::Sessions),
            effective_date: chrono::NaiveDate::from_ymd_opt(2026, 7, 26).unwrap(),
        })
        .unwrap();
        app.replace_session_snapshot_for_test(SessionSnapshot::new(
            sessions,
            &InputFootprint::default(),
        ));
        app.set_selected_clients_for_test(HashSet::from([ClientId::Codex]));
        app
    }

    fn session(id: &str, tokens: u64) -> SessionEntry {
        let mut session = SessionEntry::new(ClientId::Codex, id);
        session.tokens.input = tokens;
        session
    }

    #[test]
    fn session_order_cache_reuses_key_and_invalidates_sort_or_snapshot_revision() {
        let mut app = app_with_sessions(vec![session("small", 1), session("large", 10)]);
        let mut state = PageStates::default();
        state.select_session_client_for_test(ClientId::Codex);

        let first = state.session_order(&app);
        assert!(Arc::ptr_eq(&first, &state.session_order(&app)));

        app.sort_direction = SortDirection::Ascending;
        let resorted = state.session_order(&app);
        assert!(!Arc::ptr_eq(&first, &resorted));

        app.replace_session_snapshot_for_test(SessionSnapshot::new(
            vec![session("replacement", 20)],
            &InputFootprint::default(),
        ));
        let replacement = state.session_order(&app);
        assert!(!Arc::ptr_eq(&resorted, &replacement));
        assert_eq!(
            app.session_snapshot()
                .session(replacement[0])
                .unwrap()
                .session_id
                .as_ref(),
            "replacement"
        );
    }
}
