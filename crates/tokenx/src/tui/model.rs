//! TUI product model.
//!
//! This module owns durable product and shell state. Frame sequencing, page
//! interaction state, rendering measurements, and effect execution live in
//! their dedicated modules.

mod export;
mod rows;

use rows::RenderOrderCache;

use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::time::{Duration, Instant};

use anyhow::Result;
use chrono::NaiveDate;
use crossterm::event::{KeyEvent, MouseEvent};
use tokenx_engine::{
    pricing::PricingStatus, ClientId, ClientSelection, ClientUniverse, Generation, UsageQuery,
};

use ratatui::{layout::Rect, style::Color};

use super::data::{OverviewSummary, PeriodKind, PeriodUsage, UsageModelEntry, UsageProjection};
use super::display_labels::{group_by_label, theme_label};
use super::effect::{EffectOutcome, TuiEffect};
use super::generation_controller::{RefreshControl, RefreshRequest, RefreshStatus};
use super::intent::Intent;
use super::interaction::{InteractionOutcome, ListInteraction, MoveCommand, WrapMode};
pub use super::local_usage::PeriodDetailSelection;
use super::local_usage::{
    DetailSelections, InstalledGeneration, LocalUsageState, LocalUsageStatus, PreparedProjection,
};
use super::model_family::ModelFamily;
use super::session_data::SessionSnapshot;
use super::themes::{Theme, ThemeName};
use super::ui::dialog::{
    ClientPickerDialog, DialogResult, DialogStack, PricingSourceOrderDialog, UiCommand,
};
use crate::date_display::format_period_label;
use crate::i18n::Language;
use crate::product_paths::ProductPaths;
use crate::settings::Settings;
use crate::subscription::{
    FetchRequest, ProviderId, SubscriptionBatch, SubscriptionInstall, SubscriptionOutput,
    SubscriptionPoll, SubscriptionState,
};

/// Configuration for TUI initialization
pub struct TuiConfig {
    pub theme: Option<ThemeName>,
    pub refresh: u64,
    pub no_refresh: bool,
    pub no_spinner: bool,
    pub client_universe: ClientUniverse,
    pub initial_tab: Option<Tab>,
    pub effective_date: NaiveDate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TuiExit {
    Quit,
    Interrupted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KeyEventOutcome {
    Continue,
    Exit(TuiExit),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, clap::ValueEnum)]
pub enum Tab {
    Overview,
    Subscription,
    Models,
    Monthly,
    Weekly,
    Daily,
    Hourly,
    Stats,
    Agents,
    Sessions,
}

impl Tab {
    pub fn all() -> &'static [Tab] {
        &[
            Tab::Overview,
            Tab::Subscription,
            Tab::Models,
            Tab::Monthly,
            Tab::Weekly,
            Tab::Daily,
            Tab::Hourly,
            Tab::Stats,
            Tab::Agents,
            Tab::Sessions,
        ]
    }

    pub fn as_str(&self) -> Cow<'static, str> {
        self.labels().0
    }

    pub fn short_name(&self) -> Cow<'static, str> {
        self.labels().1
    }

    fn labels(self) -> (Cow<'static, str>, Cow<'static, str>) {
        match self {
            Tab::Overview => (
                rust_i18n::t!("tui.model.tab.overview"),
                rust_i18n::t!("tui.model.tab.overview_short"),
            ),
            Tab::Subscription => (
                rust_i18n::t!("tui.model.tab.subscription"),
                rust_i18n::t!("tui.model.tab.subscription_short"),
            ),
            Tab::Models => (
                rust_i18n::t!("tui.model.tab.models"),
                rust_i18n::t!("tui.model.tab.models_short"),
            ),
            Tab::Monthly => (
                rust_i18n::t!("tui.model.tab.monthly"),
                rust_i18n::t!("tui.model.tab.monthly_short"),
            ),
            Tab::Weekly => (
                rust_i18n::t!("tui.model.tab.weekly"),
                rust_i18n::t!("tui.model.tab.weekly_short"),
            ),
            Tab::Daily => (
                rust_i18n::t!("tui.model.tab.daily"),
                rust_i18n::t!("tui.model.tab.daily_short"),
            ),
            Tab::Hourly => (
                rust_i18n::t!("tui.model.tab.hourly"),
                rust_i18n::t!("tui.model.tab.hourly_short"),
            ),
            Tab::Stats => (
                rust_i18n::t!("tui.model.tab.stats"),
                rust_i18n::t!("tui.model.tab.stats_short"),
            ),
            Tab::Agents => (
                rust_i18n::t!("tui.model.tab.agents"),
                rust_i18n::t!("tui.model.tab.agents_short"),
            ),
            Tab::Sessions => (
                rust_i18n::t!("tui.model.tab.sessions"),
                rust_i18n::t!("tui.model.tab.sessions_short"),
            ),
        }
    }

    /// Whether this tab projects the installed local generation.
    /// Subscription has its own remote fetch lifecycle and must remain
    /// usable while local input acquisition is cold-loading or has failed.
    pub(crate) fn depends_on_local_generation(self) -> bool {
        self != Tab::Subscription
    }

    pub fn next(self) -> Tab {
        match self {
            Tab::Overview => Tab::Subscription,
            Tab::Subscription => Tab::Models,
            Tab::Models => Tab::Monthly,
            Tab::Monthly => Tab::Weekly,
            Tab::Weekly => Tab::Daily,
            Tab::Daily => Tab::Hourly,
            Tab::Hourly => Tab::Stats,
            Tab::Stats => Tab::Agents,
            Tab::Agents => Tab::Sessions,
            Tab::Sessions => Tab::Overview,
        }
    }

    pub fn prev(self) -> Tab {
        match self {
            Tab::Overview => Tab::Sessions,
            Tab::Subscription => Tab::Overview,
            Tab::Models => Tab::Subscription,
            Tab::Monthly => Tab::Models,
            Tab::Weekly => Tab::Monthly,
            Tab::Daily => Tab::Weekly,
            Tab::Hourly => Tab::Daily,
            Tab::Stats => Tab::Hourly,
            Tab::Agents => Tab::Stats,
            Tab::Sessions => Tab::Agents,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ChartGranularity {
    #[default]
    Daily,
    Hourly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortField {
    Cost,
    Tokens,
    Date,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HourlyViewMode {
    #[default]
    Table,
    Profile,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum StatsViewMode {
    #[default]
    Graph,
    Pie,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum StatusMessageKind {
    #[default]
    General,
    Generation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum StatusTone {
    #[default]
    Info,
    Success,
    Warning,
    Danger,
}

fn pricing_warning(status: PricingStatus) -> Option<Cow<'static, str>> {
    match status {
        PricingStatus::Available => None,
        PricingStatus::AvailableWithWarnings => {
            Some(rust_i18n::t!("tui.model.pricing.available_with_warnings"))
        }
        PricingStatus::CachedFallback => Some(rust_i18n::t!("tui.model.pricing.cached_fallback")),
        PricingStatus::Unavailable => Some(rust_i18n::t!("tui.model.pricing.unavailable")),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortDirection {
    Ascending,
    Descending,
}

fn sort_field_label(field: SortField) -> Cow<'static, str> {
    match field {
        SortField::Cost => rust_i18n::t!("tui.model.sort.field.cost"),
        SortField::Tokens => rust_i18n::t!("tui.model.sort.field.tokens"),
        SortField::Date => rust_i18n::t!("tui.model.sort.field.date"),
    }
}

fn sort_direction_label(direction: SortDirection) -> Cow<'static, str> {
    match direction {
        SortDirection::Ascending => rust_i18n::t!("tui.model.sort.direction.ascending"),
        SortDirection::Descending => rust_i18n::t!("tui.model.sort.direction.descending"),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum DetailSortContextKind {
    Models,
    Daily,
    Period,
}

/// Stable owner for a list interaction.
///
/// Detail views deliberately get their own identity instead of borrowing a
/// process-wide `selected_index`/`scroll_offset` pair. Switching pages or
/// entering detail mode therefore selects another state slot rather than
/// running a save/restore protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum ListContext {
    Tab(Tab),
    ModelDetail,
    DailyDetail,
    MonthlyDetail,
    WeeklyDetail,
}

#[derive(Debug, Clone, Copy, Default)]
struct DetailSortContext {
    list_sort_before_detail: Option<(SortField, SortDirection)>,
    detail_sort_state: Option<(SortField, SortDirection)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelDetailSelection {
    pub model: String,
    pub client: Option<ClientId>,
}

enum ModelDetailClientUpdate {
    Inactive,
    Ready(Vec<UsageModelEntry>),
    MissingSelection,
}

pub struct TuiModel {
    pub(crate) offline: bool,
    pub current_tab: Tab,
    pub theme: Theme,
    pub settings: Settings,
    language: Language,
    product_paths: ProductPaths,
    local_usage: LocalUsageState,

    pub sort_field: SortField,
    pub sort_direction: SortDirection,
    tab_sort_state: HashMap<Tab, (SortField, SortDirection)>,
    list_interactions: HashMap<ListContext, ListInteraction>,
    pub selected_daily_detail_date: Option<NaiveDate>,
    pub selected_period_detail: Option<PeriodDetailSelection>,
    pub selected_model_detail: Option<ModelDetailSelection>,
    model_detail_models: Option<Vec<UsageModelEntry>>,
    detail_sort_contexts: HashMap<DetailSortContextKind, DetailSortContext>,
    usage_revision: u64,
    render_order_cache: RefCell<RenderOrderCache>,

    pub selected_graph_cell: Option<(usize, usize)>,
    stats_auto_select_today_pending: bool,

    refresh_status: RefreshStatus,
    refresh_requests: VecDeque<RefreshRequest>,
    refresh_controls: VecDeque<RefreshControl>,
    effects: VecDeque<TuiEffect>,

    pub status_message: Option<String>,
    pub status_message_time: Option<Instant>,
    status_message_kind: StatusMessageKind,
    status_message_tone: StatusTone,
    generation_cache_warning: Option<String>,
    pricing_status: PricingStatus,
    pub subscription_status_message: Option<String>,
    pub subscription_status_message_time: Option<Instant>,
    subscription_status_message_tone: StatusTone,

    pub terminal_width: u16,
    pub terminal_height: u16,

    pub(crate) no_spinner: bool,
    pub spinner_frame: usize,

    /// Monotonic tick counter driving the Overview fun-fact ticker (spinner
    /// frames wrap too fast to scroll a long string).
    pub(crate) ticker_tick: u32,

    pub dialog_stack: DialogStack,

    subscription: SubscriptionState,
}

impl TuiModel {
    #[cfg(test)]
    fn test_product_paths() -> ProductPaths {
        use std::sync::atomic::{AtomicU64, Ordering};

        static NEXT_ROOT: AtomicU64 = AtomicU64::new(0);
        ProductPaths::at(std::env::temp_dir().join(format!(
            "tokenx-app-test-{}-{}",
            std::process::id(),
            NEXT_ROOT.fetch_add(1, Ordering::Relaxed)
        )))
    }

    #[cfg(test)]
    pub(crate) fn new_for_test(config: TuiConfig) -> Result<Self> {
        Self::new_for_test_with_settings(config, Settings::default())
    }

    #[cfg(test)]
    pub(crate) fn new_for_test_with_settings(
        config: TuiConfig,
        settings: Settings,
    ) -> Result<Self> {
        Self::new(config, settings, Self::test_product_paths(), false)
    }

    #[cfg(test)]
    pub(crate) fn new_for_test_with_settings_and_paths(
        config: TuiConfig,
        settings: Settings,
        product_paths: ProductPaths,
    ) -> Result<Self> {
        Self::new(config, settings, product_paths, false)
    }

    pub(crate) fn new(
        config: TuiConfig,
        settings: Settings,
        product_paths: ProductPaths,
        offline: bool,
    ) -> Result<Self> {
        let theme_name = config.theme.unwrap_or(settings.color_palette);
        let theme = Theme::from_name(theme_name);
        let language = crate::i18n::active_language();

        let client_universe = config.client_universe.clone();
        let effective_date = config.effective_date;

        let auto_refresh_interval = if config.refresh > 0 {
            Duration::from_secs(config.refresh)
        } else {
            settings.configured_auto_refresh_interval()
        };

        let auto_refresh = if config.no_refresh {
            false
        } else {
            config.refresh > 0 || settings.auto_refresh_enabled
        };
        let subscription_enabled = settings.subscription.enabled;
        let local_usage = LocalUsageState::new(UsageQuery::full(
            &client_universe,
            tokenx_engine::GroupBy::default(),
            effective_date,
        ));
        let dialog_stack = DialogStack::new(theme.clone());
        let requested_tab = config.initial_tab.unwrap_or(Tab::Overview);
        if !Self::tab_visible(&settings, requested_tab) {
            anyhow::bail!(rust_i18n::t!(
                "tui.model.error.tab_disabled",
                tab = requested_tab.as_str().to_ascii_lowercase()
            ));
        }
        let current_tab = requested_tab;
        let (sort_field, sort_direction) = Self::default_sort_for_tab(current_tab);
        let subscription = if subscription_enabled {
            SubscriptionState::new(
                settings.subscription.providers.clone(),
                crate::subscription::cache::load(&product_paths.subscription_cache_file()),
            )
        } else {
            SubscriptionState::disabled()
        };

        let mut list_interactions = HashMap::new();
        list_interactions.insert(
            ListContext::Tab(current_tab),
            ListInteraction {
                visible: 20,
                ..ListInteraction::default()
            },
        );

        let mut app = Self {
            offline,
            current_tab,
            theme,
            settings,
            language,
            product_paths,
            local_usage,
            sort_field,
            sort_direction,
            tab_sort_state: HashMap::new(),
            list_interactions,
            selected_daily_detail_date: None,
            selected_period_detail: None,
            selected_model_detail: None,
            model_detail_models: None,
            detail_sort_contexts: HashMap::new(),
            usage_revision: 0,
            render_order_cache: RefCell::new(RenderOrderCache::default()),
            selected_graph_cell: None,
            stats_auto_select_today_pending: current_tab == Tab::Stats,
            refresh_status: RefreshStatus::new(auto_refresh, auto_refresh_interval, Duration::ZERO),
            refresh_requests: VecDeque::new(),
            refresh_controls: VecDeque::new(),
            effects: VecDeque::new(),
            status_message: None,
            status_message_time: None,
            status_message_kind: StatusMessageKind::General,
            status_message_tone: StatusTone::Info,
            generation_cache_warning: None,
            pricing_status: PricingStatus::Available,
            subscription_status_message: None,
            subscription_status_message_time: None,
            subscription_status_message_tone: StatusTone::Info,
            terminal_width: 80,
            terminal_height: 24,
            no_spinner: config.no_spinner,
            spinner_frame: 0,
            ticker_tick: 0,
            dialog_stack,
            subscription,
        };
        app.try_auto_select_stats_today();
        app.maybe_fetch_subscription_on_entry();
        Ok(app)
    }

    pub(crate) fn is_background_loading(&self) -> bool {
        self.refresh_status.loading()
    }

    pub(crate) fn language(&self) -> Language {
        self.language
    }

    pub(crate) fn background_load_elapsed(&self) -> Option<Duration> {
        self.refresh_status.loading_elapsed()
    }

    pub fn has_installed_generation(&self) -> bool {
        self.installed_generation().is_some()
    }

    pub(crate) fn installed_generation(&self) -> Option<&InstalledGeneration> {
        self.local_usage.installed()
    }

    pub(crate) fn generation_health(&self) -> Option<&tokenx_engine::input_health::HealthSummary> {
        self.installed_generation()
            .map(InstalledGeneration::generation)
            .map(Generation::health)
    }

    pub(crate) fn local_usage_status(&self) -> LocalUsageStatus<'_> {
        self.local_usage.status()
    }

    pub(crate) fn is_cold_loading(&self) -> bool {
        self.is_background_loading() && !self.has_installed_generation()
    }

    pub(crate) fn is_cold_failed(&self) -> bool {
        matches!(self.local_usage.status(), LocalUsageStatus::Failed { .. })
    }

    pub(crate) fn overview_summary(&self) -> &OverviewSummary {
        self.require_installed_generation().overview()
    }

    pub(crate) fn usage(&self) -> &UsageProjection {
        self.require_installed_generation().view()
    }

    pub(crate) fn period_usage(&self, kind: PeriodKind) -> &[PeriodUsage] {
        self.require_installed_generation().periods(kind)
    }

    pub(crate) fn detail_selections(&self) -> DetailSelections {
        DetailSelections {
            daily: self.selected_daily_detail_date,
            period: self.selected_period_detail,
        }
    }

    pub(crate) fn session_snapshot(&self) -> &SessionSnapshot {
        self.require_installed_generation().sessions()
    }

    pub(crate) fn total_input_bytes(&self) -> u64 {
        self.require_installed_generation()
            .generation()
            .input_footprint()
            .total_bytes()
            .expect("validated input footprint must fit in u64")
    }

    fn require_installed_generation(&self) -> &InstalledGeneration {
        self.installed_generation()
            .expect("local projection data requires an installed generation")
    }

    pub(crate) fn group_by(&self) -> tokenx_engine::GroupBy {
        self.local_usage.query().group_by
    }

    pub(crate) fn selected_clients(&self) -> impl ExactSizeIterator<Item = ClientId> + '_ {
        self.local_usage.query().clients.iter()
    }

    pub(crate) fn client_universe(&self) -> ClientUniverse {
        self.installed_generation()
            .map(InstalledGeneration::generation)
            .map(Generation::universe)
            .cloned()
            .unwrap_or_else(|| {
                ClientUniverse::new(self.local_usage.query().clients.iter())
                    .expect("initial TUI projection has a non-empty client universe")
            })
    }

    pub(crate) fn effective_date(&self) -> NaiveDate {
        self.local_usage.query().effective_date
    }

    pub(crate) fn current_calendar_hour(&self) -> chrono::NaiveDateTime {
        self.calendar_context().current_hour()
    }

    pub(crate) fn calendar_context(&self) -> tokenx_engine::CalendarContext {
        *self
            .require_installed_generation()
            .generation()
            .acquisition_config()
            .calendar()
    }

    pub fn has_enabled_subscription_providers(&self) -> bool {
        !self.subscription.enabled().is_empty()
    }

    pub(crate) fn subscription_outputs(&self) -> &[SubscriptionOutput] {
        self.subscription.outputs()
    }

    pub(crate) fn subscription_errors(&self) -> &[crate::subscription::SubscriptionError] {
        self.subscription.errors()
    }

    pub(crate) fn has_subscription_fetch_history(&self) -> bool {
        self.subscription.has_fetch_history()
    }

    pub(crate) fn last_subscription_check(&self) -> Option<Instant> {
        self.subscription.last_checked()
    }

    pub(crate) fn refresh_status(&self) -> RefreshStatus {
        self.refresh_status
    }

    pub(crate) fn auto_refresh_enabled(&self) -> bool {
        self.refresh_status.automatic()
    }

    pub(crate) fn auto_refresh_interval(&self) -> Duration {
        self.refresh_status.interval()
    }

    pub(crate) fn last_refresh_elapsed(&self) -> Duration {
        self.refresh_status.elapsed()
    }

    pub(crate) fn set_refresh_status(&mut self, status: RefreshStatus) {
        self.refresh_status = status;
    }

    #[cfg(test)]
    pub(crate) fn set_refresh_status_for_test(
        &mut self,
        automatic: bool,
        interval: Duration,
        last_checked: Instant,
    ) {
        self.refresh_status = RefreshStatus::new(automatic, interval, last_checked.elapsed());
    }

    #[cfg(test)]
    pub(crate) fn set_refresh_loading_for_test(&mut self, loading: bool) {
        self.refresh_status.set_loading(loading);
    }

    pub(crate) fn take_refresh_requests(&mut self) -> Vec<RefreshRequest> {
        self.refresh_requests.drain(..).collect()
    }

    pub(crate) fn take_refresh_controls(&mut self) -> Vec<RefreshControl> {
        self.refresh_controls.drain(..).collect()
    }

    pub(crate) fn take_effects(&mut self) -> Vec<TuiEffect> {
        self.effects.drain(..).collect()
    }

    pub(crate) fn apply_effect_outcome(&mut self, outcome: EffectOutcome) {
        match outcome {
            EffectOutcome::SettingsPersisted {
                success_message,
                result,
            } => match result {
                Ok(()) => self.set_status(&success_message),
                Err(error) => self.set_status_with_tone(
                    &rust_i18n::t!(
                        "tui.model.status.settings_save_failed",
                        message = success_message,
                        error = error
                    ),
                    StatusTone::Danger,
                ),
            },
            EffectOutcome::TextCopied { result } => match result {
                Ok(()) => {
                    self.set_status_with_tone(
                        &rust_i18n::t!("tui.model.status.copied"),
                        StatusTone::Success,
                    );
                }
                Err(_) => self.set_status_with_tone(
                    &rust_i18n::t!("tui.model.status.copy_failed"),
                    StatusTone::Danger,
                ),
            },
            EffectOutcome::ExportWritten { path, result } => match result {
                Ok(()) => self.set_status_with_tone(
                    &rust_i18n::t!("tui.model.status.exported", path = path.display()),
                    StatusTone::Success,
                ),
                Err(error) => self.set_status_with_tone(
                    &rust_i18n::t!("tui.model.status.export_failed", error = error),
                    StatusTone::Danger,
                ),
            },
            EffectOutcome::SubscriptionCachePersisted { result: Ok(()) } => {}
            EffectOutcome::SubscriptionCachePersisted { result: Err(error) } => {
                self.subscription.record_cache_failure(error);
                self.set_subscription_status_with_tone(
                    &rust_i18n::t!("tui.model.subscription.loaded_with_errors"),
                    StatusTone::Warning,
                );
            }
        }
    }

    pub(crate) fn persist_refresh_policy(
        &mut self,
        automatic: bool,
        interval: Duration,
        message: String,
    ) {
        self.settings.auto_refresh_enabled = automatic;
        self.settings.auto_refresh_ms = interval.as_millis() as u64;
        self.queue_settings_persistence(message);
    }

    fn queue_settings_persistence(&mut self, success_message: String) {
        self.effects.push_back(TuiEffect::PersistSettings {
            settings: self.settings.clone(),
            paths: self.product_paths.clone(),
            success_message,
        });
    }

    fn handle_dialog_result(&mut self, result: DialogResult) {
        if let DialogResult::Submit(command) = result {
            self.apply_ui_command(command);
        }
    }

    fn apply_ui_command(&mut self, command: UiCommand) {
        match command {
            UiCommand::ProjectClients(clients) => self.apply_projection(clients, self.group_by()),
            UiCommand::ProjectGroupBy(group_by) => {
                self.apply_projection(self.selected_clients().collect(), group_by)
            }
            UiCommand::PricingSourceOrder(order) => {
                if self.settings.pricing_source_order != order {
                    self.settings.pricing_source_order = order;
                    self.refresh_requests.push_back(RefreshRequest::Manual);
                    self.queue_settings_persistence(
                        rust_i18n::t!("tui.model.status.pricing_source_order_changed").into_owned(),
                    );
                }
            }
        }
    }

    fn apply_projection(
        &mut self,
        selected_clients: HashSet<ClientId>,
        group_by: tokenx_engine::GroupBy,
    ) {
        let current_clients = self.selected_clients().collect::<HashSet<_>>();
        let client_changed = selected_clients != current_clients;
        let group_changed = group_by != self.group_by();
        if !client_changed && !group_changed {
            return;
        }
        if selected_clients.is_empty()
            || selected_clients
                .iter()
                .any(|client| !self.client_universe().contains(*client))
        {
            self.set_status_with_tone(
                &rust_i18n::t!("tui.model.status.client_selection_outside_universe"),
                StatusTone::Danger,
            );
            return;
        }
        if !self.has_installed_generation() {
            self.set_status_with_tone(
                &rust_i18n::t!("tui.model.status.local_data_not_loaded"),
                StatusTone::Warning,
            );
            return;
        }

        let query = UsageQuery {
            clients: match ClientSelection::new(selected_clients.iter().copied()) {
                Ok(clients) => clients,
                Err(error) => {
                    self.set_status_with_tone(
                        &rust_i18n::t!(
                            "tui.model.status.client_projection_failed",
                            error = format!("{error:#}")
                        ),
                        StatusTone::Danger,
                    );
                    return;
                }
            },
            group_by,
            effective_date: self.effective_date(),
        };
        let projection = match self
            .local_usage
            .prepare_projection(query, self.detail_selections())
        {
            Ok(projection) => projection,
            Err(error) => {
                let operation = if client_changed && !group_changed {
                    rust_i18n::t!("tui.model.status.client_projection")
                } else {
                    rust_i18n::t!("tui.model.status.group_by_projection")
                };
                let diagnostic = rust_i18n::t!(
                    "tui.model.status.projection_failed",
                    operation = operation,
                    error = format!("{error:#}")
                );
                self.set_status_with_tone(&diagnostic, StatusTone::Danger);
                return;
            }
        };
        let detail_update = if client_changed && !group_changed {
            match self.model_detail_update_for_clients(projection.view(), &selected_clients) {
                Ok(update) => update,
                Err(error) => {
                    self.set_status_with_tone(
                        &rust_i18n::t!(
                            "tui.model.status.client_projection_model_details_failed",
                            error = format!("{error:#}")
                        ),
                        StatusTone::Danger,
                    );
                    return;
                }
            }
        } else {
            ModelDetailClientUpdate::Inactive
        };

        let client_status = if group_changed {
            self.clear_model_detail_state(client_changed);
            None
        } else if client_changed {
            Some(match detail_update {
                ModelDetailClientUpdate::Ready(models) => {
                    self.model_detail_models = Some(models);
                    (
                        &rust_i18n::t!("tui.model.status.clients_filtered_details_updated"),
                        StatusTone::Success,
                    )
                }
                ModelDetailClientUpdate::MissingSelection => {
                    self.clear_model_detail_state(true);
                    (
                        &rust_i18n::t!("tui.model.status.selected_model_unavailable"),
                        StatusTone::Warning,
                    )
                }
                ModelDetailClientUpdate::Inactive => {
                    self.clear_model_detail_state(true);
                    (
                        &rust_i18n::t!("tui.model.status.clients_filtered"),
                        StatusTone::Success,
                    )
                }
            })
        } else {
            None
        };
        self.install_projected_data(projection);
        if let Some((status, tone)) = client_status {
            self.set_generation_status_with_tone(status, tone);
        } else {
            self.set_generation_status_with_tone(
                &rust_i18n::t!(
                    "tui.model.status.regrouped",
                    group_by = group_by_label(group_by)
                ),
                StatusTone::Success,
            );
        }
    }

    fn install_projected_data(&mut self, projection: PreparedProjection) {
        self.replace_usage_projection(projection, false);
        crate::acquisition::trim_allocator();
    }

    fn graph_cell_for_date(&self, date: NaiveDate) -> Option<(usize, usize)> {
        self.installed_generation()?
            .view()
            .graph
            .weeks
            .iter()
            .enumerate()
            .find_map(|(week_idx, week)| {
                week.iter()
                    .position(|day| day.as_ref().is_some_and(|day| day.date == date))
                    .map(|day_idx| (week_idx, day_idx))
            })
    }

    fn graph_date_for_cell(&self, (week_idx, day_idx): (usize, usize)) -> Option<NaiveDate> {
        self.installed_generation()?
            .view()
            .graph
            .weeks
            .get(week_idx)?
            .get(day_idx)?
            .as_ref()
            .map(|day| day.date)
    }

    fn request_stats_today_selection(&mut self) {
        self.selected_graph_cell = None;
        self.stats_auto_select_today_pending = true;
        self.try_auto_select_stats_today();
    }

    fn try_auto_select_stats_today(&mut self) {
        if !self.stats_auto_select_today_pending || self.current_tab != Tab::Stats {
            return;
        }

        if let Some(cell) = self.graph_cell_for_date(self.effective_date()) {
            self.selected_graph_cell = Some(cell);
            self.stats_auto_select_today_pending = false;
        }
    }

    #[cfg(test)]
    pub fn update_data(&mut self, data: UsageProjection) {
        self.ensure_test_generation();
        self.replace_usage_data_for_test(data, true);
        crate::acquisition::trim_allocator();
    }

    #[cfg(test)]
    fn update_projected_data(&mut self, data: UsageProjection) {
        self.ensure_test_generation();
        self.replace_usage_data_for_test(data, false);
        crate::acquisition::trim_allocator();
    }

    fn capture_usage_selection(&mut self, mark_refresh: bool) -> (bool, Option<NaiveDate>) {
        if mark_refresh {
            self.clear_model_detail_state(true);
        }
        let had_graph_selection = self.selected_graph_cell.is_some();
        let selected_graph_date = self
            .selected_graph_cell
            .and_then(|cell| self.graph_date_for_cell(cell));
        (had_graph_selection, selected_graph_date)
    }

    fn reconcile_usage_selection(
        &mut self,
        had_graph_selection: bool,
        selected_graph_date: Option<NaiveDate>,
    ) {
        if had_graph_selection {
            self.selected_graph_cell =
                selected_graph_date.and_then(|date| self.graph_cell_for_date(date));
        }
        self.try_auto_select_stats_today();

        // Exit Daily-detail mode if the refresh dropped the day we were
        // viewing; otherwise `daily_detail_rows()` would return
        // empty while the user is still nominally in detail mode.
        if let Some(date) = self.selected_daily_detail_date {
            if !self.usage().daily.iter().any(|day| day.date == date) {
                self.leave_daily_detail_sort_context();
                self.selected_daily_detail_date = None;
                self.set_current_list_interaction(self.stored_list_interaction(Tab::Daily));
            }
        }
        if let Some(selection) = self.selected_period_detail {
            let period_still_exists = self.period_usage(selection.kind).iter().any(|period| {
                period.start_date == selection.start_date && period.end_date == selection.end_date
            });
            if !period_still_exists {
                let tab = Self::period_tab(selection.kind);
                self.leave_period_detail_sort_context();
                self.selected_period_detail = None;
                self.set_current_list_interaction(self.stored_list_interaction(tab));
            }
        }

        self.clamp_selection();
    }

    fn replace_usage_projection(&mut self, projection: PreparedProjection, mark_refresh: bool) {
        let (had_graph_selection, selected_graph_date) = self.capture_usage_selection(mark_refresh);
        self.local_usage.install_projection(projection);
        self.bump_usage_revision();
        self.reconcile_usage_selection(had_graph_selection, selected_graph_date);
    }

    #[cfg(test)]
    fn replace_usage_data_for_test(&mut self, data: UsageProjection, mark_refresh: bool) {
        let (had_graph_selection, selected_graph_date) = self.capture_usage_selection(mark_refresh);
        self.local_usage
            .replace_view_for_test(data, self.detail_selections());
        self.bump_usage_revision();
        self.reconcile_usage_selection(had_graph_selection, selected_graph_date);
    }

    fn bump_usage_revision(&mut self) {
        self.usage_revision = self.usage_revision.wrapping_add(1);
        *self.render_order_cache.get_mut() = RenderOrderCache::default();
    }

    fn project_generation(
        &self,
        group_by: tokenx_engine::GroupBy,
        selected_clients: &HashSet<ClientId>,
    ) -> Result<UsageProjection> {
        let clients = ClientSelection::new(selected_clients.iter().copied())?;
        self.local_usage.project_view(&UsageQuery {
            clients,
            group_by,
            effective_date: self.effective_date(),
        })
    }

    #[cfg(test)]
    pub(crate) fn install_generation(&mut self, generation: Generation) -> Result<()> {
        if generation.universe() != &self.client_universe() {
            anyhow::bail!("generation client universe does not match TUI acquisition universe");
        }
        let installed = InstalledGeneration::new(
            std::sync::Arc::new(generation),
            self.local_usage.query().clone(),
            self.detail_selections(),
        )?;
        self.install_prepared_generation(Box::new(installed))
    }

    pub(crate) fn generation_query(&self) -> tokenx_engine::UsageQuery {
        self.local_usage.query().clone()
    }

    pub(crate) fn install_prepared_generation(
        &mut self,
        mut installed: Box<InstalledGeneration>,
    ) -> Result<()> {
        let generation = installed.generation();
        if generation.universe() != &self.client_universe() {
            anyhow::bail!("generation client universe does not match TUI acquisition universe");
        }

        let pricing_status = generation.pricing_status();
        let (had_graph_selection, selected_graph_date) = self.capture_usage_selection(true);
        installed.reconcile_projection(self.local_usage.query(), self.detail_selections())?;
        self.local_usage.install_prepared(installed);
        self.bump_usage_revision();
        self.pricing_status = pricing_status;
        self.reconcile_usage_selection(had_graph_selection, selected_graph_date);
        Ok(())
    }

    pub(crate) fn rebind_pricing_diagnostics(
        &mut self,
        diagnostics: tokenx_engine::pricing::PricingDiagnostics,
    ) {
        let status = PricingStatus::from_diagnostics(&diagnostics);
        if self.local_usage.rebind_pricing_diagnostics(diagnostics) {
            self.pricing_status = status;
        }
    }

    #[cfg(test)]
    pub(crate) fn install_generation_fixture(
        &mut self,
        usage_index: tokenx_engine::FrozenUsageIndex,
        sessions: Vec<tokenx_engine::SessionUsage>,
        input_footprint: tokenx_engine::InputFootprint,
    ) {
        self.install_generation_fixture_with_health(
            usage_index,
            sessions,
            input_footprint,
            tokenx_engine::input_health::HealthSummary::default(),
        );
    }

    #[cfg(test)]
    pub(crate) fn install_generation_fixture_with_health(
        &mut self,
        usage_index: tokenx_engine::FrozenUsageIndex,
        sessions: Vec<tokenx_engine::SessionUsage>,
        input_footprint: tokenx_engine::InputFootprint,
        health: tokenx_engine::input_health::HealthSummary,
    ) {
        let generation = super::generation_fixture_with_health(
            self.client_universe().iter(),
            usage_index,
            sessions,
            input_footprint,
            health,
        );
        self.install_generation(generation)
            .expect("test generation installs");
    }

    #[cfg(test)]
    pub(crate) fn install_generation_fixture_with_pricing_diagnostics(
        &mut self,
        diagnostics: tokenx_engine::pricing::PricingDiagnostics,
    ) {
        let generation = super::generation_fixture_with_health_and_pricing(
            self.client_universe().iter(),
            tokenx_engine::FrozenUsageIndex::new(),
            Vec::new(),
            tokenx_engine::InputFootprint::default(),
            tokenx_engine::input_health::HealthSummary::default(),
            diagnostics,
        );
        self.install_generation(generation)
            .expect("test generation installs");
    }

    #[cfg(test)]
    fn ensure_test_generation(&mut self) {
        if !self.has_installed_generation() {
            self.install_generation_fixture(
                tokenx_engine::FrozenUsageIndex::new(),
                Vec::new(),
                tokenx_engine::InputFootprint::default(),
            );
        }
    }

    #[cfg(test)]
    pub(crate) fn usage_mut_for_test(&mut self) -> super::local_usage::UsageProjectionMut<'_> {
        self.ensure_test_generation();
        self.bump_usage_revision();
        self.local_usage.view_mut()
    }

    #[cfg(test)]
    pub(crate) fn replace_session_snapshot_for_test(&mut self, snapshot: SessionSnapshot) {
        self.ensure_test_generation();
        self.local_usage.replace_sessions_for_test(snapshot);
        self.bump_usage_revision();
    }

    #[cfg(test)]
    pub(crate) fn set_selected_clients_for_test(&mut self, clients: HashSet<ClientId>) {
        let query = UsageQuery {
            clients: ClientSelection::new(clients).expect("test client selection is non-empty"),
            group_by: self.group_by(),
            effective_date: self.effective_date(),
        };
        self.local_usage.set_query_for_test(query);
        self.bump_usage_revision();
    }

    #[cfg(test)]
    pub(crate) fn set_group_by_for_test(&mut self, group_by: tokenx_engine::GroupBy) {
        let query = UsageQuery {
            clients: self.local_usage.query().clients.clone(),
            group_by,
            effective_date: self.effective_date(),
        };
        self.local_usage.set_query_for_test(query);
        self.bump_usage_revision();
    }

    #[cfg(test)]
    pub(crate) fn generation_for_test(&self) -> Option<&Generation> {
        self.installed_generation()
            .map(InstalledGeneration::generation)
    }

    pub(crate) fn fail_local_usage_load(&mut self, diagnostic: String) {
        self.local_usage.fail_acquisition(diagnostic);
    }

    #[cfg(test)]
    pub(crate) fn fail_refresh_for_test(&mut self, diagnostic: impl Into<String>) {
        self.set_refresh_loading_for_test(false);
        self.fail_local_usage_load(diagnostic.into());
    }

    pub fn model_color(&self, model_id: &str) -> Color {
        self.theme
            .model_identity_color(ModelFamily::from_model_id(model_id))
    }

    pub(crate) fn family_color(&self, family: ModelFamily) -> Color {
        self.theme.model_identity_color(family)
    }

    pub fn client_color(&self, client: ClientId) -> Color {
        self.theme.client_identity_color(Some(client))
    }

    pub(crate) fn set_generation_cache_warning(&mut self, warning: Option<String>) {
        self.generation_cache_warning = warning;
    }

    pub(crate) fn generation_cache_warning(&self) -> Option<&str> {
        self.generation_cache_warning.as_deref()
    }

    pub(crate) fn pricing_warning(&self) -> Option<Cow<'static, str>> {
        pricing_warning(self.pricing_status)
    }

    pub fn on_tick(&mut self) {
        self.on_tick_for_date(self.effective_date());
    }

    pub(super) fn on_tick_for_date(&mut self, current_date: NaiveDate) {
        self.spinner_frame = (self.spinner_frame + 1) % 20;
        self.ticker_tick = self.ticker_tick.wrapping_add(1);
        self.advance_effective_date(current_date);

        if let Some(status_time) = self.status_message_time {
            if status_time.elapsed() > Duration::from_secs(3) {
                self.status_message = None;
                self.status_message_time = None;
                self.status_message_kind = StatusMessageKind::General;
                self.status_message_tone = StatusTone::Info;
            }
        }
        if let Some(status_time) = self.subscription_status_message_time {
            if status_time.elapsed() > Duration::from_secs(3) {
                self.subscription_status_message = None;
                self.subscription_status_message_time = None;
                self.subscription_status_message_tone = StatusTone::Info;
            }
        }

        self.poll_subscription();
    }

    pub(super) fn poll_subscription(&mut self) {
        match self.subscription.poll() {
            SubscriptionPoll::Batch(batch) => {
                if self.install_subscription_batch(batch) {
                    self.effects.push_back(TuiEffect::PersistSubscriptionCache {
                        path: self.product_paths.subscription_cache_file(),
                        outputs: self.subscription.outputs().to_vec(),
                    });
                }
            }
            SubscriptionPoll::Disconnected => {
                self.subscription.install_disconnected();
                self.set_subscription_status_with_tone(
                    &rust_i18n::t!("tui.model.subscription.fetch_failed"),
                    StatusTone::Danger,
                );
            }
            SubscriptionPoll::Pending => {}
        }
    }

    pub(super) fn advance_effective_date(&mut self, current_date: NaiveDate) {
        let previous_date = self.effective_date();
        if previous_date == current_date {
            return;
        }

        let selected_previous_today = self
            .selected_graph_cell
            .and_then(|cell| self.graph_date_for_cell(cell))
            .is_some_and(|date| date == previous_date);
        let query = UsageQuery {
            clients: self.local_usage.query().clients.clone(),
            group_by: self.local_usage.query().group_by,
            effective_date: current_date,
        };

        if self.has_installed_generation() {
            match self
                .local_usage
                .prepare_projection(query, self.detail_selections())
            {
                Ok(projection) => self.replace_usage_projection(projection, false),
                Err(error) => {
                    self.set_generation_status_with_tone(
                        &rust_i18n::t!(
                            "tui.model.status.advance_calendar_failed",
                            error = format!("{error:#}")
                        ),
                        StatusTone::Danger,
                    );
                    return;
                }
            }
        } else {
            self.local_usage.replace_uninstalled_query(query);
        }

        if selected_previous_today {
            self.request_stats_today_selection();
        }
    }

    fn install_subscription_batch(&mut self, batch: SubscriptionBatch) -> bool {
        match self.subscription.install(batch) {
            SubscriptionInstall::Loaded => {
                self.set_subscription_status_with_tone(
                    &rust_i18n::t!("tui.model.subscription.loaded"),
                    StatusTone::Success,
                );
            }
            SubscriptionInstall::LoadedWithErrors => self.set_subscription_status_with_tone(
                &rust_i18n::t!("tui.model.subscription.loaded_with_errors"),
                StatusTone::Warning,
            ),
            SubscriptionInstall::Empty => self.set_subscription_status_with_tone(
                &rust_i18n::t!("tui.model.subscription.no_data"),
                StatusTone::Warning,
            ),
            SubscriptionInstall::Failed => {
                self.set_subscription_status_with_tone(
                    &rust_i18n::t!("tui.model.subscription.fetch_failed"),
                    StatusTone::Danger,
                );
            }
        }
        !self.subscription.outputs().is_empty()
    }

    pub(crate) fn handle_dialog_key(&mut self, key: KeyEvent) -> KeyEventOutcome {
        debug_assert!(self.dialog_stack.is_active());
        let result = self.dialog_stack.handle_key(key);
        self.handle_dialog_result(result);
        KeyEventOutcome::Continue
    }

    pub(crate) fn apply_intent(&mut self, intent: Intent) -> KeyEventOutcome {
        match intent {
            Intent::Interrupt => return KeyEventOutcome::Exit(TuiExit::Interrupted),
            Intent::Quit => return KeyEventOutcome::Exit(TuiExit::Quit),
            Intent::NextTab => {
                let next = self.next_visible_tab();
                self.switch_tab(next);
            }
            Intent::PreviousTab => {
                let prev = self.prev_visible_tab();
                self.switch_tab(prev);
            }
            Intent::SelectTab(tab) => self.switch_tab(tab),
            Intent::Move(command) => {
                self.apply_list_move(command);
            }
            Intent::Sort(field) => self.set_sort(field),
            Intent::Theme => self.cycle_theme(),
            Intent::PricingSources => self.open_pricing_source_order_picker(),
            Intent::Language => self.cycle_language(),
            Intent::RefreshLocal if self.current_tab != Tab::Subscription => {
                self.refresh_requests.push_back(RefreshRequest::Manual);
                let msg = if self.is_background_loading() {
                    rust_i18n::t!("tui.model.status.refresh_queued")
                } else {
                    rust_i18n::t!("tui.model.status.refresh_requested")
                };
                self.set_status(&msg);
            }
            Intent::ToggleAutoRefresh if self.current_tab != Tab::Subscription => {
                self.refresh_controls
                    .push_back(RefreshControl::ToggleAutomatic);
            }
            Intent::IncreaseRefreshInterval if self.current_tab != Tab::Subscription => {
                self.refresh_controls
                    .push_back(RefreshControl::IncreaseInterval);
            }
            Intent::DecreaseRefreshInterval if self.current_tab != Tab::Subscription => {
                self.refresh_controls
                    .push_back(RefreshControl::DecreaseInterval);
            }
            Intent::Copy => self.request_copy_selected(),
            Intent::Export if self.current_tab != Tab::Subscription => {
                self.export_to_json();
            }
            Intent::Clients => self.open_client_picker(),
            // PageStates consumes view toggles before durable model dispatch.
            Intent::ToggleView => {}
            Intent::GroupBy if self.group_by_applies_to_current_tab() => {
                self.open_group_by_picker();
            }
            Intent::RefreshSubscription if self.current_tab == Tab::Subscription => {
                self.fetch_subscription();
            }
            Intent::OpenDetails if self.current_tab == Tab::Models => {
                self.open_selected_model_detail();
            }
            Intent::OpenDetails if self.current_tab == Tab::Daily => {
                self.open_selected_daily_detail();
            }
            Intent::OpenDetails if self.current_tab == Tab::Monthly => {
                self.open_selected_period_detail(PeriodKind::Monthly);
            }
            Intent::OpenDetails if self.current_tab == Tab::Weekly => {
                self.open_selected_period_detail(PeriodKind::Weekly);
            }
            Intent::OpenDetails if self.current_tab == Tab::Stats => {
                self.handle_graph_selection();
            }
            Intent::Back if self.current_tab == Tab::Models && self.is_model_detail_active() => {
                self.close_model_detail();
            }
            Intent::Back if self.current_tab == Tab::Daily && self.is_daily_detail_active() => {
                self.close_daily_detail();
            }
            Intent::Back
                if self.current_period_kind().is_some() && self.is_period_detail_active() =>
            {
                self.close_period_detail();
            }
            Intent::Back if self.selected_graph_cell.is_some() => {
                self.selected_graph_cell = None;
                self.stats_auto_select_today_pending = false;
                self.reset_current_list_interaction();
            }
            Intent::SelectGraphCell { week, day } => {
                self.selected_graph_cell = Some((week, day));
                self.stats_auto_select_today_pending = false;
                self.reset_current_list_interaction();
            }
            _ => {}
        }
        KeyEventOutcome::Continue
    }

    #[cfg(test)]
    pub(crate) fn handle_key_event(&mut self, key: KeyEvent) -> KeyEventOutcome {
        let outcome = if let Some(Intent::Interrupt) = Intent::from_key(self.current_tab, key) {
            self.apply_intent(Intent::Interrupt)
        } else if self.dialog_stack.is_active() {
            self.handle_dialog_key(key)
        } else if let Some(intent) = Intent::from_key(self.current_tab, key) {
            self.apply_intent(intent)
        } else {
            KeyEventOutcome::Continue
        };
        super::effect::execute_pending(self);
        outcome
    }

    pub fn fetch_subscription(&mut self) {
        if self.offline {
            self.set_subscription_status_with_tone(
                &rust_i18n::t!("tui.model.subscription.offline"),
                StatusTone::Warning,
            );
            return;
        }
        match self.subscription.request_fetch() {
            FetchRequest::AlreadyFetching => {
                self.set_subscription_status(&rust_i18n::t!(
                    "tui.model.subscription.fetch_in_progress"
                ));
            }
            FetchRequest::NoProviders => self.set_subscription_status_with_tone(
                &rust_i18n::t!("tui.model.subscription.no_providers"),
                StatusTone::Warning,
            ),
            FetchRequest::Started => {
                self.set_subscription_status(&rust_i18n::t!("tui.model.subscription.fetching"));
            }
        }
    }

    pub(crate) fn take_subscription_request(
        &mut self,
    ) -> Option<(Vec<ProviderId>, std::sync::mpsc::Sender<SubscriptionBatch>)> {
        self.subscription.take_request()
    }

    #[cfg(test)]
    fn start_subscription_fetch(&mut self, rx: std::sync::mpsc::Receiver<SubscriptionBatch>) {
        self.subscription.start_fetch_for_test(rx);
        self.set_subscription_status("Fetching subscription...");
    }

    fn should_start_initial_subscription_fetch(&self) -> bool {
        self.subscription.should_start_initial_fetch(
            !self.offline
                && self.current_tab == Tab::Subscription
                && self.settings.subscription.enabled,
        )
    }

    fn maybe_fetch_subscription_on_entry(&mut self) {
        if !self.should_start_initial_subscription_fetch() {
            return;
        }
        self.fetch_subscription();
    }

    #[cfg(test)]
    pub(crate) fn set_subscription_provider_ids_for_test(&mut self, ids: Vec<ProviderId>) {
        self.subscription.set_enabled_for_test(ids);
    }

    #[cfg(test)]
    pub(crate) fn start_subscription_fetch_for_test(
        &mut self,
        rx: std::sync::mpsc::Receiver<SubscriptionBatch>,
    ) {
        self.start_subscription_fetch(rx);
    }

    #[cfg(test)]
    pub(crate) fn replace_subscription_outputs_for_test(
        &mut self,
        outputs: Vec<SubscriptionOutput>,
    ) {
        self.subscription.replace_outputs_for_test(outputs);
    }

    #[cfg(test)]
    pub(crate) fn replace_subscription_errors_for_test(
        &mut self,
        errors: Vec<crate::subscription::SubscriptionError>,
    ) {
        self.subscription.replace_errors_for_test(errors);
    }

    #[cfg(test)]
    pub(crate) fn subscription_outputs_mut_for_test(&mut self) -> &mut Vec<SubscriptionOutput> {
        self.subscription.outputs_mut_for_test()
    }

    #[cfg(test)]
    pub(crate) fn set_last_subscription_check_for_test(&mut self, checked: Option<Instant>) {
        self.subscription.set_last_checked_for_test(checked);
    }

    pub fn is_fetching_subscription(&self) -> bool {
        self.subscription.is_fetching()
    }

    pub(crate) fn subscription_fetch_elapsed(&self) -> Option<Duration> {
        self.subscription.fetch_elapsed()
    }

    pub(crate) fn enabled_subscription_provider_count(&self) -> usize {
        self.subscription.enabled().len()
    }

    pub(crate) fn handle_dialog_mouse(&mut self, event: MouseEvent, dialog_rect: Option<Rect>) {
        debug_assert!(self.dialog_stack.is_active());
        let result = self.dialog_stack.handle_mouse(event, dialog_rect);
        self.handle_dialog_result(result);
    }

    /// Cache terminal dimensions before producing the next frame. Rendered
    /// capacities are installed together from `RenderArtifacts` after draw.
    pub fn handle_resize(&mut self, width: u16, height: u16) {
        self.terminal_width = width;
        self.terminal_height = height;
    }

    #[cfg(test)]
    pub(crate) fn set_max_visible_items(&mut self, max_visible_items: usize) {
        let mut interaction = self.current_list_interaction();
        interaction.visible = max_visible_items.max(1);
        self.set_current_list_interaction(interaction);
        self.clamp_selection();
    }

    fn current_list_context(&self) -> ListContext {
        match self.current_tab {
            Tab::Models if self.is_model_detail_active() => ListContext::ModelDetail,
            Tab::Daily if self.is_daily_detail_active() => ListContext::DailyDetail,
            Tab::Monthly if self.is_period_detail_active_for_kind(PeriodKind::Monthly) => {
                ListContext::MonthlyDetail
            }
            Tab::Weekly if self.is_period_detail_active_for_kind(PeriodKind::Weekly) => {
                ListContext::WeeklyDetail
            }
            tab => ListContext::Tab(tab),
        }
    }

    fn default_list_interaction() -> ListInteraction {
        ListInteraction {
            visible: 20,
            ..ListInteraction::default()
        }
    }

    fn current_list_interaction(&self) -> ListInteraction {
        self.list_interactions
            .get(&self.current_list_context())
            .copied()
            .unwrap_or_else(Self::default_list_interaction)
    }

    pub(crate) fn list_interaction_for_render(&self) -> ListInteraction {
        self.current_list_interaction()
    }

    pub(crate) fn install_render_measurements(
        &mut self,
        artifacts: &super::render_artifacts::RenderArtifacts,
    ) {
        if let Some(interaction) = artifacts.main_list() {
            self.set_current_list_interaction(interaction);
        }
    }

    fn set_current_list_interaction(&mut self, interaction: ListInteraction) {
        self.list_interactions.insert(
            self.current_list_context(),
            ListInteraction {
                visible: interaction.visible.max(1),
                ..interaction
            },
        );
    }

    pub(crate) fn selected_index(&self) -> usize {
        self.current_list_interaction().selected
    }

    #[cfg(test)]
    pub(crate) fn scroll_offset(&self) -> usize {
        self.current_list_interaction().scroll
    }

    pub(crate) fn max_visible_items(&self) -> usize {
        self.current_list_interaction().visible
    }

    pub(crate) fn set_selected_index(&mut self, selected: usize) {
        let mut interaction = self.current_list_interaction();
        interaction.selected = selected;
        self.set_current_list_interaction(interaction);
    }

    pub(crate) fn set_scroll_offset(&mut self, scroll: usize) {
        let mut interaction = self.current_list_interaction();
        interaction.scroll = scroll;
        self.set_current_list_interaction(interaction);
    }

    fn stored_list_interaction(&self, tab: Tab) -> ListInteraction {
        self.list_interactions
            .get(&ListContext::Tab(tab))
            .copied()
            .unwrap_or_else(Self::default_list_interaction)
    }

    fn restore_current_list_interaction(&mut self) {
        self.clamp_selection();
    }

    pub(crate) fn reset_current_list_interaction(&mut self) {
        let visible = self.max_visible_items();
        self.set_current_list_interaction(ListInteraction {
            visible,
            ..ListInteraction::default()
        });
    }

    /// Clamp selection and scroll offset to valid bounds after data/resize changes.
    fn clamp_selection(&mut self) {
        let len = self.current_list_len();
        let mut interaction = self.current_list_interaction();
        interaction.set_visible(self.max_visible_items(), len);
        self.set_current_list_interaction(interaction);
    }

    fn switch_tab(&mut self, target: Tab) {
        if !self.is_tab_visible(target) {
            return;
        }

        let entering_stats = target == Tab::Stats && self.current_tab != Tab::Stats;
        let was_model_detail = self.current_tab == Tab::Models && self.is_model_detail_active();
        let was_daily_detail = self.current_tab == Tab::Daily && self.is_daily_detail_active();
        let was_period_detail = self.is_period_detail_active();
        self.persist_current_sort();

        self.current_tab = target;
        if was_model_detail {
            self.selected_model_detail = None;
            self.clear_detail_sort_context(DetailSortContextKind::Models);
        }
        if target != Tab::Daily || was_daily_detail {
            self.selected_daily_detail_date = None;
            self.clear_detail_sort_context(DetailSortContextKind::Daily);
        }
        if was_period_detail {
            self.selected_period_detail = None;
            self.clear_detail_sort_context(DetailSortContextKind::Period);
        }
        if target == Tab::Stats {
            // Re-clicking the already-active Stats tab must not discard a
            // manually chosen day; auto-select today only on tab entry.
            if entering_stats {
                self.request_stats_today_selection();
            }
        } else {
            self.selected_graph_cell = None;
            self.stats_auto_select_today_pending = false;
        }

        let (field, dir) = self
            .tab_sort_state
            .get(&target)
            .copied()
            .unwrap_or_else(|| Self::default_sort_for_tab(target));
        self.sort_field = field;
        self.sort_direction = dir;
        self.restore_current_list_interaction();
        self.maybe_fetch_subscription_on_entry();
    }

    fn default_sort_for_tab(tab: Tab) -> (SortField, SortDirection) {
        match tab {
            Tab::Models => (SortField::Tokens, SortDirection::Descending),
            Tab::Monthly | Tab::Weekly | Tab::Daily | Tab::Hourly => {
                (SortField::Date, SortDirection::Descending)
            }
            Tab::Overview | Tab::Subscription | Tab::Stats | Tab::Agents | Tab::Sessions => {
                (SortField::Cost, SortDirection::Descending)
            }
        }
    }

    fn default_sort_for_daily_detail() -> (SortField, SortDirection) {
        (SortField::Tokens, SortDirection::Descending)
    }

    fn default_sort_for_model_detail() -> (SortField, SortDirection) {
        (SortField::Tokens, SortDirection::Descending)
    }

    fn default_sort_for_period_detail() -> (SortField, SortDirection) {
        (SortField::Tokens, SortDirection::Descending)
    }

    pub(crate) fn tab_visible(settings: &Settings, tab: Tab) -> bool {
        match tab {
            Tab::Subscription => settings.subscription.enabled,
            _ => true,
        }
    }

    fn period_tab(kind: PeriodKind) -> Tab {
        match kind {
            PeriodKind::Monthly => Tab::Monthly,
            PeriodKind::Weekly => Tab::Weekly,
        }
    }

    fn current_period_kind(&self) -> Option<PeriodKind> {
        match self.current_tab {
            Tab::Monthly => Some(PeriodKind::Monthly),
            Tab::Weekly => Some(PeriodKind::Weekly),
            _ => None,
        }
    }

    pub(crate) fn is_tab_visible(&self, tab: Tab) -> bool {
        Self::tab_visible(&self.settings, tab)
    }

    fn next_visible_tab(&self) -> Tab {
        let mut candidate = self.current_tab.next();
        while !self.is_tab_visible(candidate) && candidate != self.current_tab {
            candidate = candidate.next();
        }
        candidate
    }

    fn prev_visible_tab(&self) -> Tab {
        let mut candidate = self.current_tab.prev();
        while !self.is_tab_visible(candidate) && candidate != self.current_tab {
            candidate = candidate.prev();
        }
        candidate
    }

    fn persist_current_sort(&mut self) {
        let current_sort = (self.sort_field, self.sort_direction);
        if self.current_tab == Tab::Models && self.is_model_detail_active() {
            let context = self
                .detail_sort_contexts
                .entry(DetailSortContextKind::Models)
                .or_default();
            context.detail_sort_state = Some(current_sort);
            let model_sort = self
                .detail_sort_contexts
                .get(&DetailSortContextKind::Models)
                .and_then(|context| context.list_sort_before_detail)
                .unwrap_or_else(|| Self::default_sort_for_tab(Tab::Models));
            self.tab_sort_state.insert(Tab::Models, model_sort);
            return;
        }
        if self.current_tab == Tab::Daily && self.is_daily_detail_active() {
            let context = self
                .detail_sort_contexts
                .entry(DetailSortContextKind::Daily)
                .or_default();
            context.detail_sort_state = Some(current_sort);
            let daily_sort = self
                .detail_sort_contexts
                .get(&DetailSortContextKind::Daily)
                .and_then(|context| context.list_sort_before_detail)
                .unwrap_or_else(|| Self::default_sort_for_tab(Tab::Daily));
            self.tab_sort_state.insert(Tab::Daily, daily_sort);
            return;
        }
        if let Some(selection) = self.selected_period_detail {
            let context = self
                .detail_sort_contexts
                .entry(DetailSortContextKind::Period)
                .or_default();
            context.detail_sort_state = Some(current_sort);
            let tab = Self::period_tab(selection.kind);
            let period_sort = self
                .detail_sort_contexts
                .get(&DetailSortContextKind::Period)
                .and_then(|context| context.list_sort_before_detail)
                .unwrap_or_else(|| Self::default_sort_for_tab(tab));
            self.tab_sort_state.insert(tab, period_sort);
            return;
        }

        self.tab_sort_state.insert(self.current_tab, current_sort);
    }

    fn enter_detail_sort_context(
        &mut self,
        kind: DetailSortContextKind,
        default_detail_sort: fn() -> (SortField, SortDirection),
    ) {
        let list_sort = (self.sort_field, self.sort_direction);
        let context = self.detail_sort_contexts.entry(kind).or_default();
        context.list_sort_before_detail = Some(list_sort);
        let (field, direction) = self
            .detail_sort_contexts
            .get(&kind)
            .and_then(|context| context.detail_sort_state)
            .unwrap_or_else(default_detail_sort);
        self.sort_field = field;
        self.sort_direction = direction;
    }

    fn leave_detail_sort_context(&mut self, kind: DetailSortContextKind, tab: Tab) {
        let detail_sort = (self.sort_field, self.sort_direction);
        let saved_tab_sort = self.tab_sort_state.get(&tab).copied();
        let context = self.detail_sort_contexts.entry(kind).or_default();
        context.detail_sort_state = Some(detail_sort);
        let list_sort = context
            .list_sort_before_detail
            .take()
            .or(saved_tab_sort)
            .unwrap_or_else(|| Self::default_sort_for_tab(tab));
        self.sort_field = list_sort.0;
        self.sort_direction = list_sort.1;
        self.tab_sort_state.insert(tab, list_sort);
    }

    fn clear_detail_sort_context(&mut self, kind: DetailSortContextKind) {
        if let Some(context) = self.detail_sort_contexts.get_mut(&kind) {
            context.list_sort_before_detail = None;
        }
    }

    fn enter_daily_detail_sort_context(&mut self) {
        self.enter_detail_sort_context(
            DetailSortContextKind::Daily,
            Self::default_sort_for_daily_detail,
        );
    }

    fn enter_model_detail_sort_context(&mut self) {
        self.enter_detail_sort_context(
            DetailSortContextKind::Models,
            Self::default_sort_for_model_detail,
        );
    }

    fn leave_model_detail_sort_context(&mut self) {
        self.leave_detail_sort_context(DetailSortContextKind::Models, Tab::Models);
    }

    fn leave_daily_detail_sort_context(&mut self) {
        self.leave_detail_sort_context(DetailSortContextKind::Daily, Tab::Daily);
    }

    fn enter_period_detail_sort_context(&mut self) {
        self.enter_detail_sort_context(
            DetailSortContextKind::Period,
            Self::default_sort_for_period_detail,
        );
    }

    fn leave_period_detail_sort_context(&mut self) {
        let tab = self
            .selected_period_detail
            .map(|selection| Self::period_tab(selection.kind))
            .unwrap_or(self.current_tab);
        self.leave_detail_sort_context(DetailSortContextKind::Period, tab);
    }

    #[cfg(test)]
    fn move_selection_up(&mut self) {
        self.apply_list_move(MoveCommand::Up);
    }

    #[cfg(test)]
    fn move_selection_down(&mut self) {
        self.apply_list_move(MoveCommand::Down);
    }

    fn apply_list_move(&mut self, command: MoveCommand) -> InteractionOutcome {
        let len = self.current_list_len();
        let wrap = if self.current_tab == Tab::Stats {
            WrapMode::Clamp
        } else {
            WrapMode::Wrap
        };
        let mut interaction = self.current_list_interaction();
        let outcome = interaction.apply_move(command, len, wrap);
        self.set_current_list_interaction(interaction);
        outcome
    }

    pub(crate) fn current_list_len(&self) -> usize {
        if self.current_tab.depends_on_local_generation() && !self.has_installed_generation() {
            return 0;
        }

        match self.current_tab {
            Tab::Overview => self.usage().models.len(),
            Tab::Models if self.is_model_detail_active() => self.model_row_count(),
            Tab::Models => self.usage().models.len(),
            Tab::Agents => self.usage().agents.len(),
            Tab::Daily if self.is_daily_detail_active() => self.daily_detail_row_count(),
            Tab::Monthly if self.is_period_detail_active_for_kind(PeriodKind::Monthly) => {
                self.period_detail_row_count()
            }
            Tab::Weekly if self.is_period_detail_active_for_kind(PeriodKind::Weekly) => {
                self.period_detail_row_count()
            }
            Tab::Monthly => self.period_usage(PeriodKind::Monthly).len(),
            Tab::Weekly => self.period_usage(PeriodKind::Weekly).len(),
            Tab::Daily => self.usage().daily.len(),
            Tab::Hourly => self.usage().hourly.len(),
            Tab::Stats => 0,
            Tab::Subscription => self
                .subscription_outputs()
                .iter()
                .map(|u| u.metrics.len())
                .sum(),
            Tab::Sessions => 0,
        }
    }

    fn set_sort(&mut self, field: SortField) {
        if self.sort_field == field {
            self.sort_direction = match self.sort_direction {
                SortDirection::Ascending => SortDirection::Descending,
                SortDirection::Descending => SortDirection::Ascending,
            };
        } else {
            self.sort_field = field;
            self.sort_direction = SortDirection::Descending;
        }
        self.persist_current_sort();
        if (self.current_tab == Tab::Models && self.is_model_detail_active())
            || (self.current_tab == Tab::Daily && self.is_daily_detail_active())
            || self.is_period_detail_active()
        {
            self.set_selected_index(0);
            self.set_scroll_offset(0);
        } else {
            self.selected_graph_cell = None;
            self.reset_current_list_interaction();
        }
        self.set_status(&rust_i18n::t!(
            "tui.model.status.sorted_by",
            field = sort_field_label(self.sort_field),
            direction = sort_direction_label(self.sort_direction)
        ));
    }

    fn cycle_theme(&mut self) {
        let new_theme = self.theme.name.next();
        self.theme = Theme::from_name(new_theme);
        self.dialog_stack.set_theme(self.theme.clone());
        self.settings.set_theme(new_theme);
        self.queue_settings_persistence(
            rust_i18n::t!("tui.model.status.theme", theme = theme_label(new_theme)).into_owned(),
        );
    }

    fn cycle_language(&mut self) {
        let language = self.language.next();
        crate::i18n::set_active_language(language);
        self.prepare_language_change(language);
    }

    fn prepare_language_change(&mut self, language: Language) {
        self.language = language;
        self.settings.set_language(language);
        self.subscription_status_message = None;
        self.subscription_status_message_time = None;
        self.queue_settings_persistence(
            rust_i18n::t!(
                "tui.model.status.language",
                locale = language.as_str(),
                language = language.native_name()
            )
            .into_owned(),
        );
    }

    fn open_client_picker(&mut self) {
        if !self.has_installed_generation() {
            self.set_status_with_tone(
                &rust_i18n::t!("tui.model.status.clients_unavailable_loading"),
                StatusTone::Warning,
            );
            return;
        }
        let mut clients: Vec<ClientId> = self.client_universe().iter().collect();
        clients.sort_by_key(|client| *client as usize);
        let dialog =
            ClientPickerDialog::new(clients, self.selected_clients().collect::<HashSet<_>>());
        self.dialog_stack.show(Box::new(dialog));
    }

    pub(crate) fn is_client_selected(&self, client: ClientId) -> bool {
        self.local_usage
            .query()
            .clients
            .iter()
            .any(|selected| selected == client)
    }

    /// Group By only reshapes the group-keyed projections (ADR 0004):
    /// Models plus the Daily/Monthly/Weekly tables built from them. The
    /// picker and its footer hint apply only on those tabs.
    pub fn group_by_applies_to_current_tab(&self) -> bool {
        matches!(
            self.current_tab,
            Tab::Models | Tab::Daily | Tab::Monthly | Tab::Weekly
        )
    }

    fn open_group_by_picker(&mut self) {
        if !self.has_installed_generation() {
            self.set_status_with_tone(
                &rust_i18n::t!("tui.model.status.group_by_unavailable_loading"),
                StatusTone::Warning,
            );
            return;
        }
        use super::ui::dialog::GroupByPickerDialog;
        let dialog = GroupByPickerDialog::new(self.group_by());
        self.dialog_stack.show(Box::new(dialog));
    }

    pub fn model_details_supported(&self) -> bool {
        matches!(
            self.group_by(),
            tokenx_engine::GroupBy::Model | tokenx_engine::GroupBy::ClientModel
        )
    }

    fn open_pricing_source_order_picker(&mut self) {
        self.dialog_stack
            .show(Box::new(PricingSourceOrderDialog::new(
                self.settings.pricing_source_order,
            )));
    }

    pub fn is_model_detail_active(&self) -> bool {
        self.selected_model_detail.is_some()
    }

    fn model_detail_matches(selection: &ModelDetailSelection, model: &UsageModelEntry) -> bool {
        model.model_id.as_ref() == selection.model
            && selection
                .client
                .is_none_or(|client| model.clients.as_slice() == [client])
    }

    fn model_detail_update_for_clients(
        &mut self,
        projected_data: &UsageProjection,
        selected_clients: &HashSet<ClientId>,
    ) -> Result<ModelDetailClientUpdate> {
        let Some(selection) = self.selected_model_detail.clone() else {
            return Ok(ModelDetailClientUpdate::Inactive);
        };
        if !projected_data
            .models
            .iter()
            .any(|model| Self::model_detail_matches(&selection, model))
        {
            return Ok(ModelDetailClientUpdate::MissingSelection);
        }

        let detail_data = self.project_generation(
            tokenx_engine::GroupBy::ClientProviderModel,
            selected_clients,
        )?;
        if !detail_data
            .models
            .iter()
            .any(|model| Self::model_detail_matches(&selection, model))
        {
            anyhow::bail!("provider projection omitted the selected model");
        }

        Ok(ModelDetailClientUpdate::Ready(detail_data.models))
    }

    fn open_selected_model_detail(&mut self) {
        if self.is_model_detail_active() || !self.model_details_supported() {
            return;
        }

        let selection = self
            .model_at_sorted_index(self.selected_index())
            .map(|model| ModelDetailSelection {
                model: model.model_id.to_string(),
                client: (self.group_by() == tokenx_engine::GroupBy::ClientModel)
                    .then(|| model.clients.first().copied())
                    .flatten(),
            });
        let Some(selection) = selection else {
            return;
        };

        if self.model_detail_models.is_none() {
            let selected_clients = self.selected_clients().collect::<HashSet<_>>();
            if !self.has_installed_generation() {
                self.set_status_with_tone(
                    &rust_i18n::t!("tui.model.status.model_details_unavailable_loading"),
                    StatusTone::Warning,
                );
                return;
            }
            let detail_data = match self.project_generation(
                tokenx_engine::GroupBy::ClientProviderModel,
                &selected_clients,
            ) {
                Ok(data) => data,
                Err(error) => {
                    self.set_status_with_tone(
                        &rust_i18n::t!(
                            "tui.model.status.model_details_failed",
                            error = format!("{error:#}")
                        ),
                        StatusTone::Danger,
                    );
                    return;
                }
            };
            self.model_detail_models = Some(detail_data.models);
        }

        let has_rows = self.model_detail_models.as_deref().is_some_and(|models| {
            models
                .iter()
                .any(|model| Self::model_detail_matches(&selection, model))
        });
        if !has_rows {
            self.set_status_with_tone(
                &rust_i18n::t!("tui.model.status.no_provider_details"),
                StatusTone::Warning,
            );
            return;
        }

        self.selected_model_detail = Some(selection.clone());
        self.enter_model_detail_sort_context();
        self.set_selected_index(0);
        self.set_scroll_offset(0);
        self.set_generation_status(&rust_i18n::t!(
            "tui.model.status.viewing_provider_details",
            model = selection.model
        ));
        self.clamp_selection();
    }

    fn clear_model_detail_state(&mut self, invalidate_projection: bool) {
        if self.is_model_detail_active() {
            self.leave_model_detail_sort_context();
            self.selected_model_detail = None;
            self.set_current_list_interaction(self.stored_list_interaction(Tab::Models));
        }
        if invalidate_projection {
            self.model_detail_models = None;
        }
    }

    fn close_model_detail(&mut self) {
        let Some(selection) = self.selected_model_detail.clone() else {
            return;
        };

        self.leave_model_detail_sort_context();
        self.selected_model_detail = None;

        let restored_index = self
            .model_render_order()
            .iter()
            .position(|index| {
                self.model_at_source_index(*index)
                    .is_some_and(|model| Self::model_detail_matches(&selection, model))
            })
            .unwrap_or_else(|| self.stored_list_interaction(Tab::Models).selected);
        let model_interaction = self.stored_list_interaction(Tab::Models);
        let max_visible = model_interaction.visible.max(1);
        let viewport_still_holds = restored_index >= model_interaction.scroll
            && restored_index < model_interaction.scroll + max_visible;
        let scroll = if viewport_still_holds {
            model_interaction.scroll
        } else {
            restored_index.saturating_sub(max_visible / 2)
        };
        self.set_current_list_interaction(ListInteraction {
            selected: restored_index,
            scroll,
            visible: model_interaction.visible,
        });

        self.set_generation_status(&rust_i18n::t!(
            "tui.model.status.returned_to_model",
            model = selection.model
        ));
        self.clamp_selection();
    }

    fn open_selected_daily_detail(&mut self) {
        if self.is_daily_detail_active() {
            return;
        }

        let selected_date = self
            .daily_at_sorted_index(self.selected_index())
            .map(|day| day.date);

        if let Some(date) = selected_date {
            if let Err(error) = self.local_usage.materialize_daily_detail(date) {
                self.set_generation_status_with_tone(
                    &rust_i18n::t!(
                        "tui.model.status.daily_detail_failed",
                        error = format!("{error:#}")
                    ),
                    StatusTone::Danger,
                );
                return;
            }
            self.selected_daily_detail_date = Some(date);
            self.enter_daily_detail_sort_context();
            self.set_selected_index(0);
            self.set_scroll_offset(0);
            self.set_generation_status(&rust_i18n::t!(
                "tui.model.status.viewing_daily_details",
                date = date
            ));
            self.clamp_selection();
        }
    }

    fn close_daily_detail(&mut self) {
        let Some(detail_date) = self.selected_daily_detail_date else {
            return;
        };

        self.leave_daily_detail_sort_context();
        self.selected_daily_detail_date = None;

        // Re-anchor by date so a sort change inside detail mode still
        // restores the same day rather than the stale list index.
        let restored_index = self
            .daily_render_order()
            .iter()
            .position(|index| self.usage().daily[*index].date == detail_date)
            .unwrap_or_else(|| self.stored_list_interaction(Tab::Daily).selected);

        let daily_interaction = self.stored_list_interaction(Tab::Daily);
        let max_visible = daily_interaction.visible.max(1);
        let viewport_still_holds = restored_index >= daily_interaction.scroll
            && restored_index < daily_interaction.scroll + max_visible;
        let scroll = if viewport_still_holds {
            daily_interaction.scroll
        } else {
            restored_index.saturating_sub(max_visible / 2)
        };
        self.set_current_list_interaction(ListInteraction {
            selected: restored_index,
            scroll,
            visible: daily_interaction.visible,
        });

        self.set_generation_status(&rust_i18n::t!("tui.model.status.returned_to_daily"));
        self.clamp_selection();
    }

    fn open_selected_period_detail(&mut self, kind: PeriodKind) {
        if self.is_period_detail_active() {
            return;
        }

        let selected_period = {
            let periods = self.get_sorted_periods(kind);
            periods.get(self.selected_index()).map(|period| {
                (
                    PeriodDetailSelection {
                        kind,
                        start_date: period.start_date,
                        end_date: period.end_date,
                    },
                    format!(
                        "{} {}",
                        period.section_label,
                        format_period_label(kind, period.start_date, period.end_date, false)
                    ),
                )
            })
        };

        if let Some((selection, label)) = selected_period {
            if let Err(error) = self.local_usage.materialize_period_detail(selection) {
                self.set_generation_status_with_tone(
                    &rust_i18n::t!(
                        "tui.model.status.period_detail_failed",
                        error = format!("{error:#}")
                    ),
                    StatusTone::Danger,
                );
                return;
            }
            self.selected_period_detail = Some(selection);
            self.enter_period_detail_sort_context();
            self.set_selected_index(0);
            self.set_scroll_offset(0);
            self.set_generation_status(&rust_i18n::t!(
                "tui.model.status.viewing_period_details",
                label = label
            ));
            self.clamp_selection();
        }
    }

    fn close_period_detail(&mut self) {
        let Some(selection) = self.selected_period_detail else {
            return;
        };

        self.leave_period_detail_sort_context();
        self.selected_period_detail = None;

        let restored_index = self
            .get_sorted_periods(selection.kind)
            .iter()
            .position(|period| {
                period.start_date == selection.start_date && period.end_date == selection.end_date
            })
            .unwrap_or_else(|| {
                self.stored_list_interaction(Self::period_tab(selection.kind))
                    .selected
            });

        let period_interaction = self.stored_list_interaction(Self::period_tab(selection.kind));
        let max_visible = period_interaction.visible.max(1);
        let viewport_still_holds = restored_index >= period_interaction.scroll
            && restored_index < period_interaction.scroll + max_visible;
        let scroll = if viewport_still_holds {
            period_interaction.scroll
        } else {
            restored_index.saturating_sub(max_visible / 2)
        };
        self.set_current_list_interaction(ListInteraction {
            selected: restored_index,
            scroll,
            visible: period_interaction.visible,
        });

        let msg = match selection.kind {
            PeriodKind::Monthly => rust_i18n::t!("tui.model.status.returned_to_monthly"),
            PeriodKind::Weekly => rust_i18n::t!("tui.model.status.returned_to_weekly"),
        };
        self.set_generation_status(&msg);
        self.clamp_selection();
    }

    fn handle_graph_selection(&mut self) {
        if self.current_tab == Tab::Stats && self.selected_graph_cell.is_some() {
            self.set_status(&rust_i18n::t!("tui.model.status.press_esc_deselect"));
        }
    }

    pub fn set_status(&mut self, message: &str) {
        self.set_status_with_tone(message, StatusTone::Info);
    }

    pub(crate) fn set_status_with_tone(&mut self, message: &str, tone: StatusTone) {
        self.status_message = Some(message.to_string());
        self.status_message_time = Some(Instant::now());
        self.status_message_kind = StatusMessageKind::General;
        self.status_message_tone = tone;
    }

    pub(crate) fn set_generation_status(&mut self, message: &str) {
        self.set_generation_status_with_tone(message, StatusTone::Info);
    }

    pub(crate) fn set_generation_status_with_tone(&mut self, message: &str, tone: StatusTone) {
        self.status_message = Some(message.to_string());
        self.status_message_time = Some(Instant::now());
        self.status_message_kind = StatusMessageKind::Generation;
        self.status_message_tone = tone;
    }

    fn set_subscription_status(&mut self, message: &str) {
        self.set_subscription_status_with_tone(message, StatusTone::Info);
    }

    pub(crate) fn set_subscription_status_with_tone(&mut self, message: &str, tone: StatusTone) {
        let now = Instant::now();
        let message = message.to_string();
        self.subscription_status_message = Some(message);
        self.subscription_status_message_time = Some(now);
        self.subscription_status_message_tone = tone;
    }

    pub fn general_status_message(&self) -> Option<&str> {
        if self.status_message_kind == StatusMessageKind::General {
            self.status_message.as_deref()
        } else {
            None
        }
    }

    pub(crate) fn status_message_tone(&self) -> StatusTone {
        self.status_message_tone
    }

    pub(crate) fn subscription_status_message_tone(&self) -> StatusTone {
        self.subscription_status_message_tone
    }

    pub fn is_narrow(&self) -> bool {
        self.terminal_width < 80
    }

    pub fn is_very_narrow(&self) -> bool {
        self.terminal_width < 60
    }
}

#[cfg(test)]
mod tests;
