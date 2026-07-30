mod actions;
mod colors;
mod contrast;
pub mod data;
mod display_labels;
mod effect;
mod event;
mod frame;
mod generation_controller;
mod intent;
mod interaction;
mod local_usage;
mod model;
mod model_family;
mod page_state;
mod presentation;
mod render_artifacts;
mod session_data;
mod subscription_display;
mod task_supervisor;
mod themes;
mod ui;

pub use event::{Event, EventHandler};
use frame::TuiFrame;
use generation_controller::GenerationController;
use model::{KeyEventOutcome, StatusTone};
pub use model::{Tab, TuiConfig, TuiExit, TuiModel};
use task_supervisor::TaskSupervisor;

use std::io;
use std::panic;
#[cfg(unix)]
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::TryRecvError;
#[cfg(unix)]
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use crate::acquisition::acquisition_engine;
use crate::generation_cache::{load_generation_cache, CacheResult, RetryBackoff};
use anyhow::Result;
use crossterm::{
    event::{DisableMouseCapture, EnableMouseCapture},
    execute,
    terminal::{
        disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen, SetTitle,
    },
};
use ratatui::prelude::*;
use tokenx_engine::Generation;

type TuiTerminal = Terminal<CrosstermBackend<io::Stdout>>;

struct TerminalSession {
    terminal: Option<TuiTerminal>,
    active: bool,
    #[cfg(test)]
    restore_hook: Option<Box<dyn FnOnce()>>,
}

impl TerminalSession {
    fn enter() -> Result<Self> {
        enable_raw_mode()?;
        let mut session = Self {
            terminal: None,
            active: true,
            #[cfg(test)]
            restore_hook: None,
        };
        let mut stdout = io::stdout();

        let _ = execute!(stdout, SetTitle("Tokenx"));
        execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
        session.terminal = Some(Terminal::new(CrosstermBackend::new(stdout))?);

        Ok(session)
    }

    fn terminal_mut(&mut self) -> &mut TuiTerminal {
        self.terminal
            .as_mut()
            .expect("active terminal session owns a terminal")
    }

    fn restore(&mut self) {
        if !self.active {
            return;
        }
        self.active = false;

        #[cfg(test)]
        if let Some(restore_hook) = self.restore_hook.take() {
            restore_hook();
            return;
        }

        let _ = disable_raw_mode();
        if let Some(terminal) = self.terminal.as_mut() {
            let _ = execute!(
                terminal.backend_mut(),
                LeaveAlternateScreen,
                DisableMouseCapture,
                SetTitle("")
            );
            let _ = terminal.show_cursor();
        } else {
            let _ = execute!(
                io::stdout(),
                LeaveAlternateScreen,
                DisableMouseCapture,
                SetTitle("")
            );
        }
    }

    #[cfg(test)]
    fn with_restore_hook(restore_hook: impl FnOnce() + 'static) -> Self {
        Self {
            terminal: None,
            active: true,
            restore_hook: Some(Box::new(restore_hook)),
        }
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        self.restore();
    }
}

#[cfg(test)]
use tokenx_engine::{AcquisitionConfig, ClientId, ClientUniverse};

#[cfg(test)]
pub(crate) fn generation_fixture_with_health(
    clients: impl IntoIterator<Item = ClientId>,
    usage_index: tokenx_engine::FrozenUsageIndex,
    sessions: Vec<tokenx_engine::SessionUsage>,
    input_footprint: tokenx_engine::InputFootprint,
    health: tokenx_engine::input_health::HealthSummary,
) -> Generation {
    generation_fixture_with_health_and_pricing(
        clients,
        usage_index,
        sessions,
        input_footprint,
        health,
        Vec::new(),
    )
}

#[cfg(test)]
pub(crate) fn generation_fixture_with_health_and_pricing(
    clients: impl IntoIterator<Item = ClientId>,
    usage_index: tokenx_engine::FrozenUsageIndex,
    sessions: Vec<tokenx_engine::SessionUsage>,
    input_footprint: tokenx_engine::InputFootprint,
    health: tokenx_engine::input_health::HealthSummary,
    pricing_diagnostics: tokenx_engine::pricing::PricingDiagnostics,
) -> Generation {
    let universe = ClientUniverse::new(clients).expect("test generation has clients");
    let mut canonical_footprint = tokenx_engine::InputFootprint::for_clients(universe.iter());
    for (client, bytes) in input_footprint.iter() {
        if universe.contains(client) {
            canonical_footprint
                .set_bytes(client, bytes)
                .expect("test input footprint fits in u64");
        }
    }
    Generation::new(
        AcquisitionConfig::new(
            std::path::PathBuf::from("/tmp/tokenx-test-home"),
            tokenx_engine::DateRange::none(),
            universe,
            tokenx_engine::scanner::ScannerSettings::default(),
            tokenx_engine::CalendarContext::explicit("UTC").unwrap(),
            tokenx_engine::PricingContext::explicit_with_catalog("test-custom", "test-catalog"),
        )
        .expect("test acquisition is valid"),
        tokenx_engine::SourceFingerprint::from_bytes([0; 32]),
        usage_index,
        sessions,
        canonical_footprint,
        health,
        pricing_diagnostics,
    )
    .expect("test generation is coherent")
}

fn decide_initial_data(
    load_result: CacheResult,
) -> (
    Option<Generation>,
    bool,
    Option<RetryBackoff>,
    Option<String>,
) {
    match load_result {
        CacheResult::Fresh(generation) => (Some(generation), false, None, None),
        CacheResult::Stale {
            generation,
            retry_backoff,
        } => (Some(generation), true, retry_backoff, None),
        CacheResult::RetryDeferred {
            generation,
            retry_backoff,
        } => (Some(generation), false, Some(retry_backoff), None),
        CacheResult::Missing => (None, true, None, None),
        CacheResult::Failure(failure) => (
            None,
            true,
            None,
            Some(rust_i18n::t!("tui.core.cache.warning", failure = failure).into_owned()),
        ),
    }
}

fn start_requested_subscription_fetch(app: &mut TuiModel, tasks: &mut TaskSupervisor) {
    let Some((enabled, tx)) = app.take_subscription_request() else {
        return;
    };
    tasks.spawn_subscription_fetch(enabled, tx);
}

fn install_cached_generation(app: &mut TuiModel, cached_snapshot: Option<Generation>) -> bool {
    if let Some(cached) = cached_snapshot {
        match app.install_generation(cached) {
            Ok(()) => {
                app.set_generation_status_with_tone(
                    rust_i18n::t!("tui.core.status.loaded_from_cache").as_ref(),
                    StatusTone::Success,
                );
            }
            Err(error) => {
                let warning =
                    rust_i18n::t!("tui.core.cache.rejected", error = format!("{error:#}"))
                        .into_owned();
                tracing::warn!(
                    error = %error,
                    "cached generation failed TUI projection; rebuilding from source inputs"
                );
                app.set_generation_cache_warning(Some(warning.clone()));
                app.set_generation_status_with_tone(&warning, StatusTone::Warning);
                return true;
            }
        }
    }
    false
}

fn shutdown_session(tasks: &mut TaskSupervisor, terminal_session: TerminalSession) {
    tasks.signal_cancel();
    drop(terminal_session);
    tasks.drain();
}

pub fn run(runtime: tokio::runtime::Handle, plan: crate::cli::TuiPlan) -> Result<TuiExit> {
    let crate::cli::TuiPlan {
        theme,
        refresh,
        no_refresh,
        debug,
        startup:
            crate::cli::StartupSnapshot {
                paths,
                input:
                    crate::cli::ResolvedInputScope {
                        home: home_dir,
                        universe,
                        restricted: _,
                    },
                settings,
                calendar,
                pricing,
            },
        date:
            crate::cli::ResolvedDateRange {
                range: date_range,
                label: _,
                relative: relative_date_range,
                effective_date,
            },
        initial_tab,
    } = plan;

    if debug {
        let _ = tracing_subscriber::fmt()
            .with_env_filter("debug")
            .try_init();
    }
    let config = TuiConfig {
        theme,
        refresh: refresh.unwrap_or(0),
        no_refresh,
        client_universe: universe.clone(),
        initial_tab,
        effective_date,
    };

    // Single file read: load cache and check freshness in one pass.
    let acquisition = acquisition_engine(
        paths.cache_dir(),
        home_dir,
        universe,
        date_range,
        settings.scanner.clone(),
        calendar,
        pricing,
    )?;
    let (cached_snapshot, mut needs_background_load, retry_backoff, cache_startup_warning) =
        decide_initial_data(load_generation_cache(
            &paths.generation_cache_file(),
            acquisition.config(),
        ));

    let original_hook = panic::take_hook();
    let tui_thread_id = thread::current().id();
    panic::set_hook(Box::new(move |info| {
        if thread::current().id() == tui_thread_id {
            restore_terminal_best_effort();
        }
        original_hook(info);
    }));

    // Declare the task owner first so unwinding or a future early return drops
    // the terminal session before TaskSupervisor can wait for blocking work.
    let mut tasks = TaskSupervisor::new(runtime);
    let mut terminal_session = TerminalSession::enter()?;
    let mut model = TuiModel::new(config, settings, paths.clone())?;
    if let Some(warning) = cache_startup_warning {
        tracing::warn!(warning = %warning, "generation cache unavailable at TUI startup");
        model.set_generation_cache_warning(Some(warning));
    }
    needs_background_load |= install_cached_generation(&mut model, cached_snapshot);

    let mut generation_controller = GenerationController::new(
        acquisition,
        paths.generation_cache_file(),
        model.refresh_status(),
    )
    .with_relative_date_range(relative_date_range);
    generation_controller.set_retry_backoff(retry_backoff);

    if needs_background_load {
        generation_controller.request_initial_load(true);
        generation_controller.start_pending(&mut model, &mut tasks);
    }
    let mut tui_frame = TuiFrame::new(model);

    #[cfg(unix)]
    let sigcont_flag = {
        let flag = Arc::new(AtomicBool::new(false));
        if let Err(err) =
            signal_hook::flag::register(signal_hook::consts::SIGCONT, Arc::clone(&flag))
        {
            eprintln!(
                "{}",
                rust_i18n::t!("tui.core.error.sigcont_handler", error = err)
            );
        }
        flag
    };

    let mut events = EventHandler::new(Duration::from_millis(100));

    let result = run_loop_with_background(
        terminal_session.terminal_mut(),
        &mut tui_frame,
        &mut events,
        &mut tasks,
        &mut generation_controller,
        #[cfg(unix)]
        &sigcont_flag,
    );

    shutdown_session(&mut tasks, terminal_session);

    result
}

fn restore_terminal_best_effort() {
    let _ = execute!(
        io::stdout(),
        LeaveAlternateScreen,
        DisableMouseCapture,
        SetTitle("")
    );
    let _ = disable_raw_mode();
}

fn run_loop_with_background(
    terminal: &mut TuiTerminal,
    tui_frame: &mut TuiFrame,
    events: &mut EventHandler,
    tasks: &mut TaskSupervisor,
    generation_controller: &mut GenerationController,
    #[cfg(unix)] sigcont_flag: &Arc<AtomicBool>,
) -> Result<TuiExit> {
    loop {
        start_requested_subscription_fetch(tui_frame.model_mut(), tasks);
        generation_controller.consume_app_intents(tui_frame.model_mut());
        generation_controller.on_tick(tui_frame.model_mut(), std::time::Instant::now());
        generation_controller.start_pending(tui_frame.model_mut(), tasks);
        tui_frame.flush_effects();

        #[cfg(unix)]
        if sigcont_flag.swap(false, Ordering::Relaxed) {
            let _ = enable_raw_mode();
            let _ = execute!(
                terminal.backend_mut(),
                EnterAlternateScreen,
                EnableMouseCapture
            );
            let _ = terminal.clear();
        }

        terminal.draw(|frame| tui_frame.render(frame))?;

        match tasks.try_recv_acquisition() {
            Ok(completed) => {
                if generation_controller.apply_task_result(tui_frame.model_mut(), completed) {
                    tui_frame.reconcile_generation();
                }
            }
            Err(TryRecvError::Disconnected) => {
                if tui_frame.model().is_background_loading() {
                    tui_frame.model_mut().fail_local_usage_load(
                        rust_i18n::t!("tui.core.status.background_disconnected").into_owned(),
                    );
                    tui_frame.model_mut().set_generation_status_with_tone(
                        rust_i18n::t!("tui.core.status.background_disconnected_error").as_ref(),
                        StatusTone::Danger,
                    );
                }
            }
            Err(TryRecvError::Empty) => {}
        }

        match events.next()? {
            Event::Tick => {
                tui_frame.on_tick();
            }
            Event::Key(key) => {
                if let KeyEventOutcome::Exit(exit) = tui_frame.handle_key(key) {
                    return Ok(exit);
                }
            }
            Event::Mouse(mouse) => {
                tui_frame.handle_mouse(mouse);
            }
            Event::Resize(w, h) => {
                tui_frame.model_mut().handle_resize(w, h);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::generation_controller::{
        load_background_data, persist_background_load, run_acquisition_task, BackgroundLoad,
        GenerationController,
    };
    use super::*;
    use crate::theme::ThemeName;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};
    use presentation::Presentation;
    use serial_test::serial;
    use std::collections::HashSet;
    use std::ffi::OsString;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc;
    use std::sync::Arc;
    use tempfile::TempDir;
    use tokenx_engine::InputFootprint;

    struct EnvGuard {
        home: Option<OsString>,
        config_dir: Option<OsString>,
    }

    impl EnvGuard {
        fn set(home: &std::path::Path) -> Self {
            let guard = Self {
                home: std::env::var_os("HOME"),
                config_dir: std::env::var_os("TOKENX_CONFIG_DIR"),
            };
            unsafe {
                std::env::set_var("HOME", home);
                std::env::set_var("TOKENX_CONFIG_DIR", home);
            }
            guard
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            unsafe {
                match self.home.take() {
                    Some(value) => std::env::set_var("HOME", value),
                    None => std::env::remove_var("HOME"),
                }
                match self.config_dir.take() {
                    Some(value) => std::env::set_var("TOKENX_CONFIG_DIR", value),
                    None => std::env::remove_var("TOKENX_CONFIG_DIR"),
                }
            }
        }
    }

    fn app_on(tab: Tab) -> TuiModel {
        TuiModel::new_for_test_with_settings(
            TuiConfig {
                theme: Some(ThemeName::Blue),
                refresh: 0,
                no_refresh: false,
                client_universe: tokenx_engine::ClientUniverse::all(),
                initial_tab: Some(tab),
                effective_date: chrono::NaiveDate::from_ymd_opt(2026, 7, 26).unwrap(),
            },
            crate::settings::Settings::default(),
        )
        .unwrap()
    }

    fn app_on_client(tab: Tab, client: ClientId) -> TuiModel {
        TuiModel::new_for_test_with_settings(
            TuiConfig {
                theme: Some(ThemeName::Blue),
                refresh: 0,
                no_refresh: false,
                client_universe: ClientUniverse::new([client]).unwrap(),
                initial_tab: Some(tab),
                effective_date: chrono::NaiveDate::from_ymd_opt(2026, 7, 26).unwrap(),
            },
            crate::settings::Settings::default(),
        )
        .unwrap()
    }

    fn frame_on(tab: Tab) -> TuiFrame {
        TuiFrame::new(app_on(tab))
    }

    fn mouse_event(kind: MouseEventKind) -> MouseEvent {
        MouseEvent {
            kind,
            column: 1,
            row: 1,
            modifiers: KeyModifiers::NONE,
        }
    }

    #[test]
    fn sessions_mouse_wheel_is_dispatched_to_view_state() {
        let mut frame = frame_on(Tab::Sessions);
        frame.model_mut().set_selected_index(7);
        frame.handle_mouse(mouse_event(MouseEventKind::ScrollDown));

        assert_eq!(
            frame.model().selected_index(),
            7,
            "Sessions wheel input must not reach TuiModel's non-owning list state"
        );
    }

    fn frame_with_codex_session_detail() -> TuiFrame {
        let mut frame = frame_on(Tab::Sessions);
        frame.model_mut().install_generation_fixture(
            tokenx_engine::FrozenUsageIndex::new(),
            vec![tokenx_engine::SessionUsage::new(
                ClientId::Codex,
                "codex-session",
            )],
            Default::default(),
        );
        frame
            .pages_mut()
            .select_session_client_for_test(ClientId::Codex);
        assert!(frame.pages().session_detail_active());
        let (model, pages) = frame.parts_mut();
        assert_eq!(pages.session_rows(model).len(), 1);

        frame
    }

    fn open_client_picker_and_toggle_codex(frame: &mut TuiFrame) {
        assert_eq!(
            frame.handle_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE)),
            KeyEventOutcome::Continue
        );
        assert!(frame.model().dialog_stack.is_active());

        for character in "codex".chars() {
            frame.handle_key(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE));
        }
        frame.handle_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
        assert!(frame.model().dialog_stack.is_active());
        assert!(frame.pages().session_detail_active());
    }

    #[test]
    fn escape_from_client_picker_cancels_without_leaving_session_detail() {
        let mut frame = frame_with_codex_session_detail();
        let original_clients = frame.model().selected_clients().collect::<HashSet<_>>();

        open_client_picker_and_toggle_codex(&mut frame);
        assert_eq!(
            frame.model().selected_clients().collect::<HashSet<_>>(),
            original_clients
        );

        frame.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));

        assert!(!frame.model().dialog_stack.is_active());
        assert!(frame.pages().session_detail_active());
        assert_eq!(
            frame.pages().selected_session_client(),
            Some(ClientId::Codex)
        );
        assert_eq!(
            frame.model().selected_clients().collect::<HashSet<_>>(),
            original_clients
        );
        assert!(frame.model_mut().take_refresh_requests().is_empty());
    }

    #[test]
    fn applying_client_picker_exits_detail_for_deselected_client_without_scanning() {
        let mut frame = frame_with_codex_session_detail();

        open_client_picker_and_toggle_codex(&mut frame);
        frame.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        assert!(!frame.model().dialog_stack.is_active());
        assert!(!frame.pages().session_detail_active());
        assert_eq!(frame.pages().selected_session_client(), None);
        assert!(!frame.model().is_client_selected(ClientId::Codex));
        assert!(frame.model_mut().take_refresh_requests().is_empty());
    }

    #[test]
    fn daily_profile_mouse_wheel_scrolls_without_moving_the_hidden_table() {
        let mut model = app_on(Tab::Daily);
        model.install_generation_fixture(
            tokenx_engine::FrozenUsageIndex::new(),
            Vec::new(),
            Default::default(),
        );
        let tokens = crate::tui::data::UsageTokenBreakdown {
            input: 1,
            ..Default::default()
        };
        model
            .usage_mut_for_test()
            .daily
            .push(crate::tui::data::DailyUsage {
                date: chrono::NaiveDate::from_ymd_opt(2026, 7, 22).unwrap(),
                tokens: tokens.clone(),
                cost: 0.0,
                client_breakdown: std::collections::BTreeMap::from([(
                    ClientId::Codex,
                    crate::tui::data::DailyClientInfo {
                        tokens: tokens.clone(),
                        cost: 0.0,
                        models: vec![crate::tui::data::DailyModelInfo {
                            provider: "openai".into(),
                            model_id: "gpt-5".into(),
                            display_name: "gpt-5".into(),
                            workspace_key: None,
                            workspace_label: None,
                            tokens,
                            cost: 0.0,
                            messages: 1,
                        }],
                    },
                )]),
                message_count: 1,
                turn_count: 1,
            });
        let mut frame = TuiFrame::new(model);
        assert_eq!(
            frame.handle_key(KeyEvent::new(KeyCode::Char('v'), KeyModifiers::NONE)),
            KeyEventOutcome::Continue
        );
        assert!(frame.pages().daily_profile_active());
        frame.pages_mut().set_daily_profile_text_viewport(10, 14);
        frame.model_mut().set_selected_index(7);
        frame.handle_mouse(mouse_event(MouseEventKind::ScrollDown));

        assert_eq!(frame.pages().daily_profile_scroll(), 1);
        assert_eq!(
            frame.model().selected_index(),
            7,
            "Daily Profile wheel input must not mutate the hidden Daily Table selection"
        );
    }

    #[test]
    fn empty_view_consumes_row_commands_but_keeps_recovery_actions() {
        let mut model = app_on(Tab::Models);
        model.install_generation_fixture(
            tokenx_engine::FrozenUsageIndex::new(),
            Vec::new(),
            Default::default(),
        );
        let mut frame = TuiFrame::new(model);
        let original_sort = (frame.model().sort_field, frame.model().sort_direction);

        for key in [
            KeyCode::Char('d'),
            KeyCode::Enter,
            KeyCode::Char('g'),
            KeyCode::Char('y'),
        ] {
            assert_eq!(
                frame.handle_key(KeyEvent::new(key, KeyModifiers::NONE)),
                KeyEventOutcome::Continue
            );
        }

        assert_eq!(
            (frame.model().sort_field, frame.model().sort_direction),
            original_sort
        );
        assert!(!frame.model().is_model_detail_active());
        assert!(!frame.model().dialog_stack.is_active());
        assert!(frame.model_mut().take_refresh_requests().is_empty());

        frame.handle_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE));
        assert_eq!(
            frame.model_mut().take_refresh_requests(),
            vec![generation_controller::RefreshRequest::Manual]
        );

        frame.handle_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE));
        assert!(frame.model().dialog_stack.is_active());
    }

    #[test]
    #[serial]
    fn empty_agents_does_not_block_exporting_the_installed_report() {
        let temp = TempDir::new().unwrap();
        let _env = EnvGuard::set(temp.path());
        let mut model = app_on(Tab::Agents);
        model.install_generation_fixture(
            tokenx_engine::FrozenUsageIndex::new(),
            Vec::new(),
            Default::default(),
        );
        model
            .usage_mut_for_test()
            .models
            .push(crate::tui::data::UsageModelEntry {
                model_id: "gpt-5".into(),
                display_name: "gpt-5".into(),
                provider: "openai".into(),
                clients: vec![ClientId::Codex],
                workspace_key: None,
                workspace_label: None,
                tokens: crate::tui::data::UsageTokenBreakdown {
                    input: 1,
                    ..Default::default()
                },
                cost: 0.0,
                session_count: 1,
            });
        let mut frame = TuiFrame::new(model);

        assert_eq!(
            Presentation::for_view(frame.model(), frame.pages()),
            Presentation::Empty(presentation::EmptySubject::AgentBreakdown)
        );
        frame.handle_key(KeyEvent::new(KeyCode::Char('e'), KeyModifiers::NONE));

        assert!(
            frame
                .model()
                .status_message
                .as_deref()
                .is_some_and(|message| message.starts_with("Exported to ")),
            "empty Agents must not swallow generation export"
        );
    }

    #[test]
    fn zero_session_summary_cannot_open_an_empty_detail() {
        let mut model = app_on(Tab::Sessions);
        let clients = ClientUniverse::new([ClientId::Junie])
            .unwrap()
            .as_hash_set();
        model.set_selected_clients_for_test(clients);
        model.install_generation_fixture(
            tokenx_engine::FrozenUsageIndex::new(),
            Vec::new(),
            InputFootprint::from_client_bytes([(ClientId::Junie, 0)]).unwrap(),
        );
        let mut frame = TuiFrame::new(model);

        assert_eq!(frame.pages().client_count(frame.model()), 1);
        assert_eq!(frame.pages().session_count(frame.model()), 0);
        frame.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        assert!(!frame.pages().session_detail_active());
    }

    #[test]
    fn session_detail_closes_when_refresh_leaves_the_client_without_sessions() {
        let mut frame = frame_with_codex_session_detail();
        frame
            .model_mut()
            .replace_session_snapshot_for_test(session_data::SessionSnapshot::new(
                Vec::new(),
                &InputFootprint::from_client_bytes([(ClientId::Codex, 0)]).unwrap(),
            ));

        frame.reconcile_generation();

        assert!(!frame.pages().session_detail_active());
        assert_eq!(frame.pages().client_count(frame.model()), 1);
        assert_eq!(frame.pages().session_count(frame.model()), 0);
    }

    fn write_amp_input(home: &std::path::Path, input_tokens: u64) {
        write_amp_model_input(home, "claude-opus-4-7", input_tokens);
    }

    fn write_amp_model_input(home: &std::path::Path, model: &str, input_tokens: u64) {
        let directory = home.join(".local/share/amp/threads");
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(
            directory.join("T-refresh.json"),
            format!(
                r#"{{
                    "id": "refresh-thread",
                    "created": 1747800000000,
                    "messages": [{{
                        "role": "assistant",
                        "messageId": 1,
                        "usage": {{
                            "timestamp": "2026-05-21T04:00:00Z",
                            "model": "{model}",
                            "inputTokens": {input_tokens},
                            "outputTokens": 2
                        }}
                    }}]
                }}"#
            ),
        )
        .unwrap()
    }

    fn session(client: ClientId, session_id: &str) -> tokenx_engine::SessionUsage {
        tokenx_engine::SessionUsage {
            last_seen: 100,
            ..tokenx_engine::SessionUsage::new(client, session_id)
        }
    }

    fn input_bytes_for(app: &TuiModel, client: ClientId) -> Option<u64> {
        app.session_snapshot()
            .client_summaries()
            .iter()
            .find(|summary| summary.client == client)
            .map(|summary| summary.space_bytes)
    }

    fn generation_with_usage(
        client: ClientId,
        input_tokens: i64,
        session_id: &str,
        input_bytes: u64,
        signature: tokenx_engine::SourceFingerprint,
    ) -> Generation {
        let accumulator = tokenx_engine::build_usage_index(
            &[tokenx_engine::AttributedUsageRecord::new(
                client,
                "test-model",
                "test-provider",
                session_id,
                100,
                tokenx_engine::TokenBreakdown {
                    input: input_tokens,
                    ..Default::default()
                },
                0.0,
            )],
            tokenx_engine::DateRange::none(),
            tokenx_engine::CalendarContext::explicit("UTC").unwrap(),
        )
        .unwrap();
        Generation::new(
            AcquisitionConfig::new(
                std::path::PathBuf::from("/tmp/tokenx-test-home"),
                tokenx_engine::DateRange::none(),
                ClientUniverse::new([client]).unwrap(),
                tokenx_engine::scanner::ScannerSettings::default(),
                tokenx_engine::CalendarContext::explicit("UTC").unwrap(),
                tokenx_engine::PricingContext::explicit_with_catalog("test-custom", "test-catalog"),
            )
            .unwrap(),
            signature,
            accumulator,
            vec![session(client, session_id)],
            InputFootprint::from_client_bytes([(client, input_bytes)]).unwrap(),
            tokenx_engine::input_health::HealthSummary::default(),
            Vec::new(),
        )
        .unwrap()
    }

    fn loaded_generation(generation: Generation) -> BackgroundLoad {
        BackgroundLoad::Loaded {
            generation: Box::new(generation),
            cache_persistence_warning: None,
            retry_backoff: None,
        }
    }

    fn controller_for(
        app: &TuiModel,
        acquisition: tokenx_engine::AcquisitionEngine,
    ) -> GenerationController {
        GenerationController::new(
            acquisition,
            std::path::PathBuf::from("/tmp/tokenx-tui-controller-generation.bin"),
            app.refresh_status(),
        )
    }

    fn controller_for_client(app: &TuiModel, client: ClientId) -> GenerationController {
        let acquisition = acquisition_engine(
            std::path::PathBuf::from("/tmp/tokenx-tui-controller-cache"),
            std::path::PathBuf::from("/tmp/tokenx-tui-controller-test"),
            ClientUniverse::new([client]).unwrap(),
            tokenx_engine::DateRange::none(),
            tokenx_engine::scanner::ScannerSettings::default(),
            tokenx_engine::CalendarContext::explicit("UTC").unwrap(),
            crate::acquisition::test_pricing_snapshot(),
        )
        .unwrap();
        controller_for(app, acquisition)
    }

    #[test]
    fn cached_generation_install_error_warns_and_requests_rebuild() {
        let restored = Arc::new(AtomicBool::new(false));
        let restored_by_guard = Arc::clone(&restored);
        let mut app = app_on_client(Tab::Overview, ClientId::Codex);
        let cached = generation_fixture_with_health(
            [ClientId::Amp],
            tokenx_engine::FrozenUsageIndex::default(),
            Vec::new(),
            InputFootprint::default(),
            tokenx_engine::input_health::HealthSummary::default(),
        );
        let (cached, mut needs_background_load, retry_backoff, cache_warning) =
            decide_initial_data(CacheResult::Fresh(cached));
        assert!(!needs_background_load);
        assert!(retry_backoff.is_none());
        assert!(cache_warning.is_none());

        needs_background_load |= {
            let _terminal_session = TerminalSession::with_restore_hook(move || {
                restored_by_guard.store(true, Ordering::Release);
            });
            install_cached_generation(&mut app, cached)
        };

        assert!(needs_background_load);
        assert!(app
            .generation_cache_warning()
            .unwrap()
            .contains("cached generation rejected"));
        assert!(app
            .generation_cache_warning()
            .unwrap()
            .contains("generation client universe does not match"));
        assert!(app.generation_for_test().is_none());
        assert!(restored.load(Ordering::Acquire));
    }

    #[test]
    fn shutdown_restores_terminal_before_waiting_for_persistence() {
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        let mut tasks = TaskSupervisor::new(runtime.handle().clone());
        let persistence_gate = tasks.persistence_gate_for_test();
        let (gate_held_tx, gate_held_rx) = mpsc::channel();
        let (restored_tx, restored_rx) = mpsc::channel();
        let gate_holder = std::thread::spawn(move || {
            let _persistence_guard = persistence_gate
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            gate_held_tx
                .send(())
                .expect("test receiver remains available");
            restored_rx
                .recv_timeout(Duration::from_secs(1))
                .expect("terminal restoration must precede persistence quiescence");
        });
        gate_held_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("persistence gate holder started");

        let restored = Arc::new(AtomicBool::new(false));
        let restored_by_guard = Arc::clone(&restored);
        let terminal_session = TerminalSession::with_restore_hook(move || {
            restored_by_guard.store(true, Ordering::Release);
            let _ = restored_tx.send(());
        });

        shutdown_session(&mut tasks, terminal_session);

        gate_holder.join().expect("persistence gate holder");
        assert!(restored.load(Ordering::Acquire));
    }

    #[test]
    #[serial]
    fn fresh_unified_snapshot_renders_all_tabs_without_background_load() {
        let signature = tokenx_engine::SourceFingerprint::from_bytes([1; 32]);
        let generation = generation_with_usage(ClientId::Amp, 1, "cached-session", 512, signature);

        let (cached_data, needs_background_load, retry_backoff, cache_warning) =
            decide_initial_data(CacheResult::Fresh(generation));

        let cached_data = cached_data.expect("fresh cache must remain immediately visible");
        assert!(!needs_background_load);
        assert!(retry_backoff.is_none());
        assert!(cache_warning.is_none());
        assert_eq!(
            cached_data.sessions()[0].session_id.as_ref(),
            "cached-session"
        );
        assert_eq!(cached_data.input_footprint().bytes_for(ClientId::Amp), 512);
    }

    #[test]
    #[serial]
    fn stale_unified_snapshot_renders_immediately_and_refreshes_in_background() {
        let generation = generation_with_usage(
            ClientId::Amp,
            1,
            "cached-session",
            512,
            tokenx_engine::SourceFingerprint::from_bytes([2; 32]),
        );

        let (cached_data, needs_background_load, retry_backoff, cache_warning) =
            decide_initial_data(CacheResult::Stale {
                generation,
                retry_backoff: None,
            });

        let cached_data = cached_data.expect("stale cache must remain immediately visible");
        assert!(needs_background_load);
        assert!(retry_backoff.is_none());
        assert!(cache_warning.is_none());
        assert_eq!(
            cached_data.sessions()[0].session_id.as_ref(),
            "cached-session"
        );
    }

    #[test]
    fn missing_cache_has_no_snapshot_and_requests_background_load_without_warning() {
        let (cached_data, needs_background_load, retry_backoff, cache_warning) =
            decide_initial_data(CacheResult::Missing);

        assert!(cached_data.is_none());
        assert!(needs_background_load);
        assert!(retry_backoff.is_none());
        assert!(cache_warning.is_none());
    }

    #[test]
    fn failed_cache_has_no_snapshot_and_surfaces_startup_warning() {
        let failure = crate::generation_cache::CacheFailure::new(
            crate::generation_cache::CacheFailureKind::Decode,
            "cache digest mismatch",
        );
        let (cached_data, needs_background_load, retry_backoff, cache_warning) =
            decide_initial_data(CacheResult::Failure(failure));

        assert!(cached_data.is_none());
        assert!(needs_background_load);
        assert!(retry_backoff.is_none());
        assert_eq!(
            cache_warning.as_deref(),
            Some("Generation cache warning: decode failure: cache digest mismatch")
        );
    }

    #[test]
    #[serial]
    fn fresh_generation_fingerprint_skips_unchanged_inputs_and_reloads_changed_inputs() {
        let home = TempDir::new().unwrap();
        let _guard = EnvGuard::set(home.path());
        write_amp_input(home.path(), 10);
        let loader = acquisition_engine(
            home.path().join("cache"),
            home.path().to_path_buf(),
            ClientUniverse::new([ClientId::Amp]).unwrap(),
            tokenx_engine::DateRange::none(),
            tokenx_engine::scanner::ScannerSettings::default(),
            tokenx_engine::CalendarContext::explicit("UTC").unwrap(),
            crate::acquisition::test_pricing_snapshot(),
        )
        .unwrap();
        let prepared = loader.prepare().unwrap();
        let signature_a = prepared.source_fingerprint();
        let cached = generation_with_usage(ClientId::Amp, 12, "cached-session", 512, signature_a);
        let (_, needs_load, _, _) = decide_initial_data(CacheResult::Fresh(cached));
        let baseline = Some(signature_a);
        assert!(!needs_load);

        assert!(matches!(
            load_background_data(&loader, false, baseline).unwrap(),
            BackgroundLoad::Unchanged
        ));

        write_amp_input(home.path(), 1000);
        let changed = load_background_data(&loader, false, baseline).unwrap();
        match changed {
            BackgroundLoad::Loaded { generation, .. } => {
                assert_ne!(Some(generation.source_fingerprint()), baseline);
                let data = generation
                    .project_usage(&tokenx_engine::UsageQuery::full(
                        generation.universe(),
                        tokenx_engine::GroupBy::Model,
                        chrono::NaiveDate::from_ymd_opt(2026, 7, 26).unwrap(),
                    ))
                    .unwrap();
                assert_eq!(data.total_tokens, 1002);
            }
            BackgroundLoad::Unchanged => {
                panic!("changed input B must consume its inventory")
            }
        }
    }

    #[test]
    #[serial]
    fn background_reload_reprojects_to_group_selected_while_loading() {
        let home = TempDir::new().unwrap();
        let _guard = EnvGuard::set(home.path());
        write_amp_model_input(home.path(), "old-model", 10);
        let loader = acquisition_engine(
            home.path().join("cache"),
            home.path().to_path_buf(),
            ClientUniverse::new([ClientId::Amp]).unwrap(),
            tokenx_engine::DateRange::none(),
            tokenx_engine::scanner::ScannerSettings::default(),
            tokenx_engine::CalendarContext::explicit("UTC").unwrap(),
            crate::acquisition::test_pricing_snapshot(),
        )
        .unwrap();
        let old = load_background_data(&loader, true, None).unwrap();
        let mut app = app_on_client(Tab::Models, ClientId::Amp);
        let mut controller = controller_for(&app, loader.clone());
        controller.apply_result_for_test(&mut app, Ok(old), true);
        assert_eq!(app.usage().models[0].model_id.as_ref(), "old-model");

        write_amp_model_input(home.path(), "new-model", 100);
        app.set_group_by_for_test(tokenx_engine::GroupBy::ClientProviderModel);
        let baseline = app
            .generation_for_test()
            .map(Generation::source_fingerprint);
        let loaded = load_background_data(&loader, true, baseline).unwrap();
        controller.apply_result_for_test(&mut app, Ok(loaded), true);

        assert_eq!(app.group_by(), tokenx_engine::GroupBy::ClientProviderModel);
        assert_eq!(app.usage().models[0].model_id.as_ref(), "new-model");
        assert_eq!(app.usage().models[0].clients, [ClientId::Amp]);
        let model_projection = app
            .generation_for_test()
            .expect("loaded generation is installed")
            .project_usage(&tokenx_engine::UsageQuery::full(
                app.generation_for_test().unwrap().universe(),
                tokenx_engine::GroupBy::Model,
                chrono::NaiveDate::from_ymd_opt(2026, 7, 26).unwrap(),
            ))
            .unwrap();
        assert_eq!(model_projection.models[0].model_id.as_ref(), "new-model");
        assert_eq!(
            app.local_usage_status(),
            local_usage::LocalUsageStatus::Ready
        );
        assert_eq!(app.status_message.as_deref(), Some("Data loaded"));
        assert_eq!(app.status_message_tone(), StatusTone::Success);
        assert_eq!(
            app.general_status_message(),
            None,
            "local load success must not leak into the Subscription status row"
        );
    }

    #[test]
    fn loaded_result_replaces_the_canonical_generation_atomically() {
        let old_signature = tokenx_engine::SourceFingerprint::from_bytes([3; 32]);
        let new_signature = tokenx_engine::SourceFingerprint::from_bytes([4; 32]);
        let mut app = app_on_client(Tab::Models, ClientId::Amp);
        let mut controller = controller_for_client(&app, ClientId::Amp);
        controller.apply_result_for_test(
            &mut app,
            Ok(loaded_generation(generation_with_usage(
                ClientId::Amp,
                11,
                "old-session",
                11,
                old_signature,
            ))),
            false,
        );
        app.set_refresh_loading_for_test(true);

        controller.apply_result_for_test(
            &mut app,
            Ok(loaded_generation(generation_with_usage(
                ClientId::Amp,
                99,
                "new-session",
                4096,
                new_signature,
            ))),
            false,
        );

        assert!(!app.is_background_loading());
        assert_eq!(app.usage().total_tokens, 99);
        assert_eq!(
            app.session_snapshot().sessions()[0].session_id.as_ref(),
            "new-session"
        );
        assert_eq!(input_bytes_for(&app, ClientId::Amp), Some(4096));
        assert!(app.has_installed_generation());
        assert_eq!(
            app.generation_for_test()
                .map(Generation::source_fingerprint),
            Some(new_signature)
        );
        assert_eq!(
            app.local_usage_status(),
            local_usage::LocalUsageStatus::Ready
        );
    }

    #[test]
    fn unchanged_inventory_does_not_replace_any_snapshot_component() {
        let signature = tokenx_engine::SourceFingerprint::from_bytes([9; 32]);
        let mut app = app_on_client(Tab::Models, ClientId::Amp);
        let mut controller = controller_for_client(&app, ClientId::Amp);
        controller.apply_result_for_test(
            &mut app,
            Ok(loaded_generation(generation_with_usage(
                ClientId::Amp,
                77,
                "retained-session",
                2048,
                signature,
            ))),
            false,
        );
        app.set_refresh_loading_for_test(true);

        controller.apply_result_for_test(&mut app, Ok(BackgroundLoad::Unchanged), false);

        assert!(!app.is_background_loading());
        assert_eq!(app.usage().total_tokens, 77);
        assert_eq!(
            app.session_snapshot().sessions()[0].session_id.as_ref(),
            "retained-session"
        );
        assert_eq!(input_bytes_for(&app, ClientId::Amp), Some(2048));
        assert!(app.has_installed_generation());
        assert_eq!(
            app.generation_for_test()
                .map(Generation::source_fingerprint),
            Some(signature)
        );
        assert_eq!(
            app.local_usage_status(),
            local_usage::LocalUsageStatus::Ready
        );
    }

    #[test]
    fn failed_cold_load_marks_sessions_unavailable_without_inventing_snapshot() {
        let mut app = app_on(Tab::Models);
        let mut controller = controller_for_client(&app, ClientId::Amp);
        app.set_refresh_loading_for_test(true);

        controller.apply_result_for_test(&mut app, Err(anyhow::anyhow!("load failed")), false);

        assert!(!app.is_background_loading());
        assert!(!app.has_installed_generation());
        assert!(matches!(
            app.local_usage_status(),
            local_usage::LocalUsageStatus::Failed {
                diagnostic: "load failed"
            }
        ));
        assert_eq!(app.general_status_message(), None);
        assert_eq!(app.status_message.as_deref(), Some("Error: load failed"));
    }

    #[test]
    fn background_worker_panic_clears_loading_and_marks_snapshot_degraded() {
        let mut app = app_on(Tab::Models);
        app.set_refresh_loading_for_test(true);
        app.install_generation_fixture(
            tokenx_engine::FrozenUsageIndex::new(),
            Vec::new(),
            Default::default(),
        );
        app.set_refresh_loading_for_test(true);
        let (tx, rx) = mpsc::channel();
        let mut controller = controller_for_client(&app, ClientId::Amp);

        run_acquisition_task(&tx, 1, || -> Result<BackgroundLoad> {
            panic!("injected worker panic")
        });
        let completed = rx.recv().unwrap();
        controller.apply_result_for_test(&mut app, completed.result, false);

        assert!(!app.is_background_loading());
        assert!(matches!(
            app.local_usage_status(),
            local_usage::LocalUsageStatus::Degraded { diagnostic }
                if diagnostic.contains("injected worker panic")
        ));
    }

    #[test]
    #[serial]
    fn failed_background_reload_keeps_existing_snapshot_and_marks_it_degraded() {
        let home = TempDir::new().unwrap();
        let _guard = EnvGuard::set(home.path());
        write_amp_model_input(home.path(), "retained-model", 10);
        let loader = acquisition_engine(
            home.path().join("cache"),
            home.path().to_path_buf(),
            ClientUniverse::new([ClientId::Amp]).unwrap(),
            tokenx_engine::DateRange::none(),
            tokenx_engine::scanner::ScannerSettings::default(),
            tokenx_engine::CalendarContext::explicit("UTC").unwrap(),
            crate::acquisition::test_pricing_snapshot(),
        )
        .unwrap();
        let loaded = load_background_data(&loader, true, None).unwrap();
        let mut app = app_on_client(Tab::Models, ClientId::Amp);
        let mut controller = controller_for(&app, loader);
        controller.apply_result_for_test(&mut app, Ok(loaded), true);
        let old_tokens = app.usage().total_tokens;
        let old_sessions = app.session_snapshot().sessions().to_vec();
        let old_input_bytes = input_bytes_for(&app, ClientId::Amp);

        controller.apply_result_for_test(&mut app, Err(anyhow::anyhow!("load failed")), false);

        assert_eq!(app.usage().total_tokens, old_tokens);
        assert_eq!(app.usage().models[0].model_id.as_ref(), "retained-model");
        assert_eq!(app.session_snapshot().sessions(), old_sessions);
        assert_eq!(input_bytes_for(&app, ClientId::Amp), old_input_bytes);
        assert!(app.has_installed_generation());
        assert!(matches!(
            app.local_usage_status(),
            local_usage::LocalUsageStatus::Degraded { .. }
        ));
        assert_eq!(app.status_message.as_deref(), Some("Error: load failed"));
    }

    #[test]
    #[serial]
    fn cache_failure_keeps_the_built_generation() {
        let home = TempDir::new().unwrap();
        let blocked_config = home.path().join("config-is-a-file");
        std::fs::write(&blocked_config, b"not a directory").unwrap();
        let _guard = EnvGuard::set(&blocked_config);
        let signature = tokenx_engine::SourceFingerprint::from_bytes([7; 32]);
        let loaded = loaded_generation(generation_with_usage(
            ClientId::Amp,
            42,
            "loaded-despite-cache-error",
            42,
            signature,
        ));

        let persisted =
            persist_background_load(&blocked_config.join("generation.bin"), Ok(loaded)).unwrap();

        match persisted {
            BackgroundLoad::Loaded {
                generation,
                cache_persistence_warning,
                retry_backoff: _,
            } => {
                let data = generation
                    .project_usage(&tokenx_engine::UsageQuery::full(
                        generation.universe(),
                        tokenx_engine::GroupBy::Model,
                        chrono::NaiveDate::from_ymd_opt(2026, 7, 26).unwrap(),
                    ))
                    .unwrap();
                assert_eq!(data.total_tokens, 42);
                assert_eq!(
                    generation.sessions()[0].session_id.as_ref(),
                    "loaded-despite-cache-error"
                );
                assert_eq!(generation.source_fingerprint(), signature);
                let warning = cache_persistence_warning
                    .as_deref()
                    .expect("cache persistence warning must be retained");
                assert!(warning.contains("Cache persistence warning"));
            }
            BackgroundLoad::Unchanged => {
                panic!("loaded data must not become unchanged")
            }
        }
    }
}
