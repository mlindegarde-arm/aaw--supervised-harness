use crate::error::{HarnessError, HarnessResult};
use crate::runtime::{CommandEvent, CommandResult, CommandRuntime, OutputSink, TuiRuntimeEvent};
use crate::service::HarnessService;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

pub type RuntimeServiceFactory = Arc<dyn Fn() -> Box<dyn HarnessService> + Send + Sync + 'static>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BridgeState {
    Idle,
    Running,
    Cancelling,
}

pub struct RuntimeBridge {
    service_factory: RuntimeServiceFactory,
    sender: Sender<TuiRuntimeEvent>,
    receiver: Receiver<TuiRuntimeEvent>,
    worker: Option<JoinHandle<()>>,
    cancellation: Arc<AtomicBool>,
    state: BridgeState,
}

impl RuntimeBridge {
    pub fn new(service_factory: RuntimeServiceFactory) -> Self {
        let (sender, receiver) = mpsc::channel();
        Self {
            service_factory,
            sender,
            receiver,
            worker: None,
            cancellation: Arc::new(AtomicBool::new(false)),
            state: BridgeState::Idle,
        }
    }

    pub fn state(&self) -> BridgeState {
        self.state
    }

    pub fn is_running(&self) -> bool {
        self.state != BridgeState::Idle
    }

    pub fn start_command(&mut self, command: impl Into<String>) -> HarnessResult<()> {
        if self.is_running() {
            return Err(HarnessError::Conflict(
                "a foreground command is already running".to_string(),
            ));
        }

        let command = command.into();
        self.cancellation = Arc::new(AtomicBool::new(false));
        let cancellation = self.cancellation.clone();
        let sender = self.sender.clone();
        let service_factory = self.service_factory.clone();
        self.state = BridgeState::Running;
        self.worker = Some(thread::spawn(move || {
            let service = service_factory();
            let runtime = CommandRuntime::new(service.as_ref());
            let next_command = Arc::new(Mutex::new(None));
            let mut sink = BridgeSink {
                sender: sender.clone(),
                cancellation: cancellation.clone(),
                next_command: next_command.clone(),
                finished: false,
            };

            let exit = runtime.dispatch_line(&command, &mut sink);
            if cancellation.load(Ordering::SeqCst) && !sink.finished {
                let next_command = next_command.lock().ok().and_then(|guard| guard.clone());
                let _ = sender.send(TuiRuntimeEvent::CancelAcknowledged { next_command });
            } else if exit.code() != 0 && !sink.finished {
                let _ = sender.send(TuiRuntimeEvent::CommandFinished(CommandResult::new(exit)));
            }
        }));
        Ok(())
    }

    pub fn start_event_worker<F>(&mut self, worker: F) -> HarnessResult<()>
    where
        F: FnOnce(Sender<TuiRuntimeEvent>, Arc<AtomicBool>) + Send + 'static,
    {
        if self.is_running() {
            return Err(HarnessError::Conflict(
                "a foreground command is already running".to_string(),
            ));
        }

        self.cancellation = Arc::new(AtomicBool::new(false));
        let cancellation = self.cancellation.clone();
        let sender = self.sender.clone();
        self.state = BridgeState::Running;
        self.worker = Some(thread::spawn(move || worker(sender, cancellation)));
        Ok(())
    }

    pub fn cancel(&mut self) -> bool {
        if self.state == BridgeState::Idle {
            return false;
        }
        self.cancellation.store(true, Ordering::SeqCst);
        self.state = BridgeState::Cancelling;
        true
    }

    pub fn try_recv(&mut self) -> Option<TuiRuntimeEvent> {
        self.reap_finished_worker();
        match self.receiver.try_recv() {
            Ok(event) => {
                if matches!(
                    event,
                    TuiRuntimeEvent::CommandFinished(_)
                        | TuiRuntimeEvent::CancelAcknowledged { .. }
                        | TuiRuntimeEvent::Failed(_)
                ) {
                    self.state = BridgeState::Idle;
                    self.join_worker();
                }
                Some(event)
            }
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => {
                self.state = BridgeState::Idle;
                None
            }
        }
    }

    pub fn drain(&mut self) -> Vec<TuiRuntimeEvent> {
        let mut events = Vec::new();
        while let Some(event) = self.try_recv() {
            events.push(event);
        }
        events
    }

    fn reap_finished_worker(&mut self) {
        if self
            .worker
            .as_ref()
            .is_some_and(|worker| worker.is_finished())
        {
            self.join_worker();
            if self.state != BridgeState::Idle {
                self.state = BridgeState::Idle;
            }
        }
    }

    fn join_worker(&mut self) {
        if let Some(worker) = self.worker.take() {
            if worker.join().is_err() {
                let _ = self.sender.send(TuiRuntimeEvent::Failed(
                    "runtime worker panicked".to_string(),
                ));
                self.state = BridgeState::Idle;
            }
        }
    }
}

impl Drop for RuntimeBridge {
    fn drop(&mut self) {
        self.cancellation.store(true, Ordering::SeqCst);
        self.join_worker();
    }
}

struct BridgeSink {
    sender: Sender<TuiRuntimeEvent>,
    cancellation: Arc<AtomicBool>,
    next_command: Arc<Mutex<Option<String>>>,
    finished: bool,
}

impl BridgeSink {
    fn send_event(&self, event: TuiRuntimeEvent) -> HarnessResult<()> {
        self.sender
            .send(event)
            .map_err(|err| HarnessError::External(format!("send TUI runtime event: {err}")))
    }

    fn remember_next_command(&self, event: &CommandEvent) {
        let Some(progress) = &event.supervise_progress else {
            return;
        };
        if let Some(next) = &progress.next_command
            && let Ok(mut guard) = self.next_command.lock()
        {
            *guard = Some(next.clone());
        }
    }
}

impl OutputSink for BridgeSink {
    fn event(&mut self, event: &CommandEvent) -> HarnessResult<()> {
        self.remember_next_command(event);
        if self.cancellation.load(Ordering::SeqCst) {
            return Err(HarnessError::External("command cancelled".to_string()));
        }
        if let Some(progress) = &event.supervise_progress {
            self.send_event(TuiRuntimeEvent::Progress(progress.clone()))?;
        }
        self.send_event(TuiRuntimeEvent::CommandEvent(event.clone()))
    }

    fn finish(&mut self, result: &CommandResult) -> HarnessResult<()> {
        self.finished = true;
        if self.cancellation.load(Ordering::SeqCst) {
            let next_command = self
                .next_command
                .lock()
                .ok()
                .and_then(|guard| guard.clone());
            self.send_event(TuiRuntimeEvent::CancelAcknowledged { next_command })
        } else {
            self.send_event(TuiRuntimeEvent::CommandFinished(result.clone()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::HarnessResult;
    use crate::domain::{Task, TaskId, Ticket, TicketId};
    use crate::runtime::{
        CommandEventLevel, CommandExit, OutputSink, ResumeTaskOptions, SuperviseCreateOptions,
        SuperviseProgressEvent, SuperviseProgressPhase, SuperviseTaskOptions, TaskRunOptions,
        TicketResolveOptions,
    };
    use crate::service::HarnessService;
    use std::time::{Duration, Instant};

    const TASK_ID: &str = "task_01ARZ3NDEKTSV4RRFFQ69G5FAV";

    #[test]
    fn runtime_bridge_streams_progress_event_and_finish() {
        let mut bridge = RuntimeBridge::new(Arc::new(|| Box::new(FakeService)));

        bridge
            .start_command(format!("supervise {TASK_ID}"))
            .unwrap();
        let events = collect_until_idle(&mut bridge);

        assert!(events.iter().any(|event| matches!(
            event,
            TuiRuntimeEvent::Progress(progress)
                if progress.phase == SuperviseProgressPhase::InspectTask
        )));
        assert!(events.iter().any(|event| matches!(
            event,
            TuiRuntimeEvent::CommandFinished(result) if result.exit.code() == 0
        )));
        assert_eq!(bridge.state(), BridgeState::Idle);
    }

    #[test]
    fn runtime_bridge_observes_progress_while_supervise_worker_is_still_running() {
        let release = Arc::new(AtomicBool::new(false));
        let release_for_service = release.clone();
        let mut bridge = RuntimeBridge::new(Arc::new(move || {
            Box::new(BlockingProgressService {
                release: release_for_service.clone(),
            })
        }));

        bridge
            .start_command(format!("supervise {TASK_ID}"))
            .unwrap();
        let progress = wait_for_progress(&mut bridge);

        assert_eq!(progress.phase, SuperviseProgressPhase::RunTask);
        assert_eq!(bridge.state(), BridgeState::Running);

        release.store(true, Ordering::SeqCst);
        let events = collect_until_idle(&mut bridge);
        assert!(events.iter().any(|event| matches!(
            event,
            TuiRuntimeEvent::CommandFinished(result) if result.exit.code() == 0
        )));
    }

    #[test]
    fn runtime_bridge_ctrl_c_reports_cancellation_acknowledgement() {
        let mut bridge = RuntimeBridge::new(Arc::new(|| Box::new(SlowService)));

        bridge
            .start_command(format!("supervise {TASK_ID}"))
            .unwrap();
        assert!(bridge.cancel());
        let events = collect_until_idle(&mut bridge);

        assert!(events.iter().any(|event| matches!(
            event,
            TuiRuntimeEvent::CancelAcknowledged { next_command }
                if next_command.as_deref() == Some("harness supervise task_01ARZ3NDEKTSV4RRFFQ69G5FAV")
        )));
        assert_eq!(bridge.state(), BridgeState::Idle);
    }

    fn collect_until_idle(bridge: &mut RuntimeBridge) -> Vec<TuiRuntimeEvent> {
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut events = Vec::new();
        while Instant::now() < deadline {
            events.extend(bridge.drain());
            if bridge.state() == BridgeState::Idle {
                return events;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("bridge did not become idle");
    }

    fn wait_for_progress(bridge: &mut RuntimeBridge) -> SuperviseProgressEvent {
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            for event in bridge.drain() {
                if let TuiRuntimeEvent::Progress(progress) = event {
                    return progress;
                }
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("bridge did not stream progress");
    }

    struct FakeService;

    impl HarnessService for FakeService {
        fn create_task(
            &self,
            _title: String,
            _goal: String,
            _validation_commands: Vec<String>,
        ) -> HarnessResult<Task> {
            unreachable!()
        }

        fn list_tasks(&self) -> HarnessResult<Vec<Task>> {
            Ok(Vec::new())
        }

        fn get_task(&self, _task_id: &TaskId) -> HarnessResult<Task> {
            unreachable!()
        }

        fn run_task(
            &self,
            _task_id: &TaskId,
            _options: TaskRunOptions,
        ) -> HarnessResult<CommandResult> {
            unreachable!()
        }

        fn list_tickets(&self) -> HarnessResult<Vec<Ticket>> {
            Ok(Vec::new())
        }

        fn get_ticket(&self, _ticket_id: &TicketId) -> HarnessResult<Ticket> {
            unreachable!()
        }

        fn resolve_ticket(
            &self,
            _ticket_id: &TicketId,
            _options: TicketResolveOptions,
        ) -> HarnessResult<CommandResult> {
            unreachable!()
        }

        fn resume_task(
            &self,
            _task_id: &TaskId,
            _options: ResumeTaskOptions,
        ) -> HarnessResult<CommandResult> {
            unreachable!()
        }

        fn supervise_task(
            &self,
            task_id: &TaskId,
            _options: SuperviseTaskOptions,
        ) -> HarnessResult<CommandResult> {
            let progress = SuperviseProgressEvent {
                phase: SuperviseProgressPhase::InspectTask,
                task_id: Some(task_id.clone()),
                run_id: None,
                ticket_id: None,
                cycle: Some(0),
                message: "inspecting".to_string(),
                next_command: Some(format!("harness supervise {task_id}")),
            };
            Ok(CommandResult::new(CommandExit::success()).with_event(
                CommandEvent::supervise_progress(progress, CommandEventLevel::Info),
            ))
        }

        fn create_and_supervise_task(
            &self,
            _options: SuperviseCreateOptions,
        ) -> HarnessResult<CommandResult> {
            unreachable!()
        }
    }

    struct SlowService;

    impl HarnessService for SlowService {
        fn supervise_task(
            &self,
            task_id: &TaskId,
            _options: SuperviseTaskOptions,
        ) -> HarnessResult<CommandResult> {
            std::thread::sleep(Duration::from_millis(40));
            FakeService.supervise_task(task_id, SuperviseTaskOptions::default())
        }

        fn create_task(&self, a: String, b: String, c: Vec<String>) -> HarnessResult<Task> {
            FakeService.create_task(a, b, c)
        }
        fn list_tasks(&self) -> HarnessResult<Vec<Task>> {
            FakeService.list_tasks()
        }
        fn get_task(&self, task_id: &TaskId) -> HarnessResult<Task> {
            FakeService.get_task(task_id)
        }
        fn run_task(
            &self,
            task_id: &TaskId,
            options: TaskRunOptions,
        ) -> HarnessResult<CommandResult> {
            FakeService.run_task(task_id, options)
        }
        fn list_tickets(&self) -> HarnessResult<Vec<Ticket>> {
            FakeService.list_tickets()
        }
        fn get_ticket(&self, ticket_id: &TicketId) -> HarnessResult<Ticket> {
            FakeService.get_ticket(ticket_id)
        }
        fn resolve_ticket(
            &self,
            ticket_id: &TicketId,
            options: TicketResolveOptions,
        ) -> HarnessResult<CommandResult> {
            FakeService.resolve_ticket(ticket_id, options)
        }
        fn resume_task(
            &self,
            task_id: &TaskId,
            options: ResumeTaskOptions,
        ) -> HarnessResult<CommandResult> {
            FakeService.resume_task(task_id, options)
        }
    }

    struct BlockingProgressService {
        release: Arc<AtomicBool>,
    }

    impl HarnessService for BlockingProgressService {
        fn supervise_task_streaming(
            &self,
            task_id: &TaskId,
            _options: SuperviseTaskOptions,
            sink: &mut dyn OutputSink,
        ) -> HarnessResult<CommandResult> {
            let progress = SuperviseProgressEvent {
                phase: SuperviseProgressPhase::RunTask,
                task_id: Some(task_id.clone()),
                run_id: None,
                ticket_id: None,
                cycle: Some(0),
                message: "running task".to_string(),
                next_command: Some(format!("harness supervise {task_id}")),
            };
            sink.event(&CommandEvent::supervise_progress(
                progress,
                CommandEventLevel::Info,
            ))?;
            while !self.release.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(10));
            }
            Ok(CommandResult::new(CommandExit::success()))
        }

        fn create_task(&self, a: String, b: String, c: Vec<String>) -> HarnessResult<Task> {
            FakeService.create_task(a, b, c)
        }
        fn list_tasks(&self) -> HarnessResult<Vec<Task>> {
            FakeService.list_tasks()
        }
        fn get_task(&self, task_id: &TaskId) -> HarnessResult<Task> {
            FakeService.get_task(task_id)
        }
        fn run_task(
            &self,
            task_id: &TaskId,
            options: TaskRunOptions,
        ) -> HarnessResult<CommandResult> {
            FakeService.run_task(task_id, options)
        }
        fn list_tickets(&self) -> HarnessResult<Vec<Ticket>> {
            FakeService.list_tickets()
        }
        fn get_ticket(&self, ticket_id: &TicketId) -> HarnessResult<Ticket> {
            FakeService.get_ticket(ticket_id)
        }
        fn resolve_ticket(
            &self,
            ticket_id: &TicketId,
            options: TicketResolveOptions,
        ) -> HarnessResult<CommandResult> {
            FakeService.resolve_ticket(ticket_id, options)
        }
        fn resume_task(
            &self,
            task_id: &TaskId,
            options: ResumeTaskOptions,
        ) -> HarnessResult<CommandResult> {
            FakeService.resume_task(task_id, options)
        }
    }
}
