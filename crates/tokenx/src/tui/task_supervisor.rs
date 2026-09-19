use std::sync::{mpsc, Arc, Mutex};
use std::thread;

use crate::subscription::{ProviderId, SubscriptionBatch};

use super::generation_controller::{
    catch_background_task, load_background_data_with_cancellation,
    run_acquisition_task_with_cancellation, AcquisitionTaskResult, PersistenceResult,
};
use super::local_usage::ProjectionRequest;
use super::startup::{load_startup, StartupLoad};
use super::Event;

/// Owns every task whose lifetime is bounded by the interactive TUI session.
///
/// A task handle must not be detached: acquisition can write the generation
/// cache, while pricing and subscription fetches can retain network work.
/// Shutdown first signals cancellation, then aborts async work and joins every
/// owned task.
pub(super) struct TaskSupervisor {
    runtime: tokio::runtime::Handle,
    event_tx: mpsc::Sender<Event>,
    startup_tx: mpsc::Sender<anyhow::Result<StartupLoad>>,
    startup_rx: mpsc::Receiver<anyhow::Result<StartupLoad>>,
    persistence_tx: mpsc::Sender<PersistenceResult>,
    persistence_rx: mpsc::Receiver<PersistenceResult>,
    acquisition_tx: mpsc::Sender<AcquisitionTaskResult>,
    acquisition_rx: mpsc::Receiver<AcquisitionTaskResult>,
    pricing_tx: mpsc::Sender<Arc<tokenx_engine::pricing::ResolvedPricingSnapshot>>,
    pricing_rx: mpsc::Receiver<Arc<tokenx_engine::pricing::ResolvedPricingSnapshot>>,
    cancellation: tokenx_engine::AcquisitionCancellation,
    persistence_gate: Arc<Mutex<()>>,
    acquisition_tasks: Vec<thread::JoinHandle<()>>,
    remote_tasks: Vec<tokio::task::JoinHandle<()>>,
    drained: bool,
}

impl TaskSupervisor {
    pub(super) fn new(runtime: tokio::runtime::Handle, event_tx: mpsc::Sender<Event>) -> Self {
        let (startup_tx, startup_rx) = mpsc::channel();
        let (persistence_tx, persistence_rx) = mpsc::channel();
        let (acquisition_tx, acquisition_rx) = mpsc::channel();
        let (pricing_tx, pricing_rx) = mpsc::channel();
        Self {
            runtime,
            event_tx,
            startup_tx,
            startup_rx,
            persistence_tx,
            persistence_rx,
            acquisition_tx,
            acquisition_rx,
            pricing_tx,
            pricing_rx,
            cancellation: tokenx_engine::AcquisitionCancellation::default(),
            persistence_gate: Arc::new(Mutex::new(())),
            acquisition_tasks: Vec::new(),
            remote_tasks: Vec::new(),
            drained: false,
        }
    }

    pub(super) fn spawn_startup(
        &mut self,
        startup: crate::cli::StartupSnapshot<crate::cli::PendingPricing>,
        date_range: tokenx_engine::DateRange,
        query: tokenx_engine::UsageQuery,
    ) {
        let tx = self.startup_tx.clone();
        let event_tx = self.event_tx.clone();
        let cancellation = self.cancellation.clone();
        self.acquisition_tasks.push(thread::spawn(move || {
            let result = catch_background_task(|| load_startup(startup, date_range, query));
            if !cancellation.is_cancelled() {
                let _ = tx.send(result);
                let _ = event_tx.send(Event::BackgroundReady);
            }
        }));
    }

    pub(super) fn spawn_pricing_refresh(
        &mut self,
        pricing: Arc<tokenx_engine::pricing::ResolvedPricingSnapshot>,
        cache_dir: std::path::PathBuf,
    ) {
        self.reap_finished();
        let tx = self.pricing_tx.clone();
        let event_tx = self.event_tx.clone();
        self.remote_tasks.push(self.runtime.spawn(async move {
            let snapshot = pricing.refresh_public_catalogs(&cache_dir).await;
            let _ = tx.send(Arc::new(snapshot));
            let _ = event_tx.send(Event::BackgroundReady);
        }));
    }

    pub(super) fn spawn_acquisition(
        &mut self,
        request_id: u64,
        engine: tokenx_engine::AcquisitionEngine,
        force: bool,
        last_fingerprint: Option<tokenx_engine::SourceFingerprint>,
        projection: ProjectionRequest,
    ) {
        self.reap_finished();
        let tx = self.acquisition_tx.clone();
        let cancellation = self.cancellation.clone();
        let event_tx = self.event_tx.clone();

        self.acquisition_tasks.push(thread::spawn(move || {
            run_acquisition_task_with_cancellation(&tx, request_id, &cancellation, || {
                load_background_data_with_cancellation(
                    &engine,
                    force,
                    last_fingerprint,
                    projection,
                    &cancellation,
                )
            });
            let _ = event_tx.send(Event::BackgroundReady);
        }));
    }

    pub(super) fn spawn_persistence(
        &mut self,
        request_id: u64,
        path: std::path::PathBuf,
        generation: Arc<tokenx_engine::Generation>,
    ) {
        self.reap_finished();
        let tx = self.persistence_tx.clone();
        let event_tx = self.event_tx.clone();
        let cancellation = self.cancellation.clone();
        let gate = Arc::clone(&self.persistence_gate);
        self.acquisition_tasks.push(thread::spawn(move || {
            let result = catch_background_task(|| {
                let _guard = gate.lock().expect("persistence gate poisoned");
                cancellation.check(tokenx_engine::AcquisitionPhase::CacheFinalization)?;
                crate::generation_cache::save_generation_cache_with_retry_backoff(
                    &path,
                    &generation,
                )
            });
            if !cancellation.is_cancelled() {
                let _ = tx.send(PersistenceResult { request_id, result });
                let _ = event_tx.send(Event::BackgroundReady);
            }
            drop(generation);
            crate::acquisition::trim_allocator();
        }));
    }

    pub(super) fn spawn_subscription_fetch(
        &mut self,
        enabled: Vec<ProviderId>,
        tx: mpsc::Sender<SubscriptionBatch>,
    ) {
        self.reap_finished();
        let event_tx = self.event_tx.clone();
        self.remote_tasks.push(self.runtime.spawn(async move {
            let batch = crate::subscription::service::fetch_enabled(&enabled).await;
            let _ = tx.send(batch);
            let _ = event_tx.send(Event::BackgroundReady);
        }));
    }

    pub(super) fn try_recv_startup(
        &self,
    ) -> Result<anyhow::Result<StartupLoad>, mpsc::TryRecvError> {
        self.startup_rx.try_recv()
    }

    pub(super) fn try_recv_persistence(&self) -> Result<PersistenceResult, mpsc::TryRecvError> {
        self.persistence_rx.try_recv()
    }

    pub(super) fn try_recv_acquisition(
        &self,
    ) -> std::result::Result<AcquisitionTaskResult, mpsc::TryRecvError> {
        self.acquisition_rx.try_recv()
    }

    pub(super) fn try_recv_pricing(
        &self,
    ) -> std::result::Result<Arc<tokenx_engine::pricing::ResolvedPricingSnapshot>, mpsc::TryRecvError>
    {
        self.pricing_rx.try_recv()
    }

    /// Signals cancellation without waiting for blocking acquisition work.
    ///
    /// Terminal restoration must happen after this signal and before
    /// [`Self::drain`], which may wait for an in-progress cache write.
    pub(super) fn signal_cancel(&mut self) {
        self.cancellation.cancel();
        for task in &self.remote_tasks {
            task.abort();
        }
    }

    /// Drains all cancelled work. Call after terminal restoration so a slow
    /// blocking acquisition cannot strand the user in raw terminal mode.
    pub(super) fn drain(&mut self) {
        if self.drained {
            return;
        }
        self.signal_cancel();

        // Linearize cancellation against generation-cache persistence. A
        // writer already holding the gate may finish; every writer reaching
        // the gate after signal_cancel observes cancellation before writing.
        // Do not move this synchronization into signal_cancel: waiting here is
        // deliberately deferred until after terminal restoration.
        drop(
            self.persistence_gate
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
        );

        self.drained = true;
        let remote_tasks = std::mem::take(&mut self.remote_tasks);
        self.runtime.block_on(async move {
            for task in remote_tasks {
                let _ = task.await;
            }
        });

        for task in self.acquisition_tasks.drain(..) {
            let _ = task.join();
        }
    }

    fn reap_finished(&mut self) {
        let mut index = 0;
        while index < self.acquisition_tasks.len() {
            if self.acquisition_tasks[index].is_finished() {
                let task = self.acquisition_tasks.swap_remove(index);
                let _ = task.join();
            } else {
                index += 1;
            }
        }
        self.remote_tasks.retain(|task| !task.is_finished());
    }

    #[cfg(test)]
    pub(super) fn persistence_gate_for_test(&self) -> Arc<Mutex<()>> {
        Arc::clone(&self.persistence_gate)
    }
}

impl Drop for TaskSupervisor {
    fn drop(&mut self) {
        self.drain();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc;
    use std::sync::Arc;
    use std::time::Duration;

    use super::TaskSupervisor;

    struct Dropped(Arc<AtomicBool>);

    impl Drop for Dropped {
        fn drop(&mut self) {
            self.0.store(true, Ordering::Release);
        }
    }

    #[test]
    fn shutdown_aborts_and_drains_subscription_tasks() {
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        let mut supervisor = TaskSupervisor::new(runtime.handle().clone(), mpsc::channel().0);
        let dropped = Arc::new(AtomicBool::new(false));
        let task_dropped = Arc::clone(&dropped);
        let (started_tx, started_rx) = mpsc::channel();

        supervisor.remote_tasks.push(runtime.spawn(async move {
            let _guard = Dropped(task_dropped);
            started_tx.send(()).expect("test receiver remains open");
            tokio::time::sleep(Duration::from_secs(60)).await;
        }));
        started_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("task started");

        supervisor.signal_cancel();
        supervisor.drain();

        assert!(dropped.load(Ordering::Acquire));
        assert!(supervisor.remote_tasks.is_empty());
    }

    #[test]
    fn shutdown_cancels_and_joins_acquisition_threads() {
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        let mut supervisor = TaskSupervisor::new(runtime.handle().clone(), mpsc::channel().0);
        let cancellation = supervisor.cancellation.clone();
        let joined = Arc::new(AtomicBool::new(false));
        let task_joined = Arc::clone(&joined);

        supervisor
            .acquisition_tasks
            .push(std::thread::spawn(move || {
                while !cancellation.is_cancelled() {
                    std::thread::yield_now();
                }
                task_joined.store(true, Ordering::Release);
            }));

        supervisor.signal_cancel();
        supervisor.drain();

        assert!(joined.load(Ordering::Acquire));
        assert!(supervisor.acquisition_tasks.is_empty());
    }

    #[test]
    fn signal_cancel_does_not_wait_for_persistence_gate() {
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        let mut supervisor = TaskSupervisor::new(runtime.handle().clone(), mpsc::channel().0);
        let persistence_gate = Arc::clone(&supervisor.persistence_gate);
        let persistence_guard = persistence_gate
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let (signalled_tx, signalled_rx) = mpsc::channel();

        std::thread::scope(|scope| {
            let signal_task = scope.spawn(|| {
                supervisor.signal_cancel();
                signalled_tx
                    .send(())
                    .expect("test receiver remains available");
            });

            signalled_rx
                .recv_timeout(Duration::from_secs(1))
                .expect("signal_cancel must not wait for persistence");
            drop(persistence_guard);
            signal_task.join().expect("signal task");
        });

        assert!(supervisor.cancellation.is_cancelled());
        supervisor.drain();
    }

    #[test]
    fn acquisition_notifies_the_ui_before_any_generation_cache_write() {
        let root = tempfile::TempDir::new().unwrap();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let (event_tx, event_rx) = mpsc::channel();
        let mut supervisor = TaskSupervisor::new(runtime.handle().clone(), event_tx);
        let engine = crate::acquisition::acquisition_engine(
            root.path().join("cache"),
            root.path().to_path_buf(),
            tokenx_engine::ClientUniverse::new([tokenx_engine::ClientId::Amp]).unwrap(),
            tokenx_engine::DateRange::none(),
            Default::default(),
            tokenx_engine::CalendarContext::explicit("UTC").unwrap(),
            crate::acquisition::test_pricing_snapshot(),
        )
        .unwrap();
        let query = tokenx_engine::UsageQuery::full(
            engine.config().universe(),
            tokenx_engine::GroupBy::default(),
            engine.config().calendar().current_date(),
        );
        let path = root.path().join("cache/generation.bin");
        let gate = supervisor.persistence_gate_for_test();
        let held = gate.lock().unwrap();
        supervisor.spawn_acquisition(
            1,
            engine,
            false,
            None,
            super::ProjectionRequest {
                query,
                details: Default::default(),
            },
        );
        assert!(matches!(
            event_rx.recv_timeout(Duration::from_secs(5)).unwrap(),
            super::Event::BackgroundReady
        ));
        let completed = supervisor.try_recv_acquisition().unwrap();
        let super::super::generation_controller::BackgroundLoad::Loaded { generation } =
            completed.result.unwrap()
        else {
            panic!("cold acquisition must publish a complete generation");
        };
        assert!(
            !path.exists(),
            "publishing data must not write the generation cache"
        );
        supervisor.spawn_persistence(1, path.clone(), generation.shared_generation());
        assert!(matches!(
            supervisor.try_recv_persistence(),
            Err(mpsc::TryRecvError::Empty)
        ));
        assert!(!path.exists());
        drop(held);
        assert!(matches!(
            event_rx.recv_timeout(Duration::from_secs(5)).unwrap(),
            super::Event::BackgroundReady
        ));
        assert!(supervisor.try_recv_persistence().unwrap().result.is_ok());
        assert!(path.is_file());
        supervisor.drain();
    }
}
