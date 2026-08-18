use super::{
    Preset, WireMessage, decode_message, discover_executable, file_uri, incremental_change,
    initialize_params, read_frame, write_frame,
};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    io::{BufReader, Read as _},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command as ProcessCommand, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{Receiver, SyncSender},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

const COMMAND_CAPACITY: usize = 64;
const EVENT_CAPACITY: usize = 512;
const CHANGE_DELAY: Duration = Duration::from_millis(50);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(1);
const STDERR_LIMIT: usize = 64 * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DocumentSnapshot {
    pub path: PathBuf,
    pub language_id: String,
    pub text: String,
    pub revision: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SyncKind {
    Full,
    Incremental,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ServerCapabilities {
    pub sync: SyncKind,
    pub open_close: bool,
    pub save_include_text: Option<bool>,
    pub completion: bool,
    pub completion_triggers: Vec<String>,
    pub hover: bool,
    pub definition: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ServerStatus {
    NotStarted,
    Starting,
    Ready(ServerCapabilities),
    NotFound,
    Failed(String),
    Stopped,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DiagnosticSeverity {
    Error,
    Warning,
    Information,
    Hint,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Diagnostic {
    pub range: std::ops::Range<usize>,
    pub line: u32,
    pub severity: DiagnosticSeverity,
    pub source: Option<String>,
    pub code: Option<String>,
    pub message: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RequestTag {
    pub path: PathBuf,
    pub revision: u64,
    pub cursor: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompletionEdit {
    pub range: std::ops::Range<usize>,
    pub new_text: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompletionItem {
    pub label: String,
    pub kind: Option<i32>,
    pub detail: Option<String>,
    pub insert_text: String,
    pub edit: Option<CompletionEdit>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HoverContent {
    pub text: String,
    pub markdown: bool,
    pub range: Option<std::ops::Range<usize>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DefinitionLocation {
    pub path: PathBuf,
    pub line: u32,
    pub character: u32,
    pub end_line: u32,
    pub end_character: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Event {
    StateChanged(ServerStatus),
    Diagnostics {
        path: PathBuf,
        revision: u64,
        version: Option<i32>,
        diagnostics: Vec<Diagnostic>,
        truncated: bool,
    },
    DiagnosticsStale(PathBuf),
    Completion {
        tag: RequestTag,
        items: Vec<CompletionItem>,
        truncated: bool,
    },
    Hover {
        tag: RequestTag,
        content: Option<HoverContent>,
    },
    Definitions {
        tag: RequestTag,
        locations: Vec<DefinitionLocation>,
        truncated: bool,
    },
    ServerMessage(String),
    ProcessExited {
        error: String,
        stderr: String,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ServerLaunch {
    Auto,
    Custom { command: String, args: Vec<String> },
    Off,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Command {
    Open(DocumentSnapshot),
    Change {
        path: PathBuf,
        text: String,
        revision: u64,
    },
    Save(PathBuf),
    Close(PathBuf),
    Complete {
        tag: RequestTag,
        trigger: Option<String>,
    },
    Hover(RequestTag),
    Definition(RequestTag),
    Restart(ServerLaunch),
    Rescan,
    Shutdown,
}

enum Input {
    Command(Command),
    Wire(Result<WireMessage, String>),
}

pub struct Controller {
    input: SyncSender<Input>,
    events: Receiver<Event>,
    worker: Option<JoinHandle<()>>,
}

impl Controller {
    pub fn start(
        project_root: PathBuf,
        preset: &'static Preset,
        launch: ServerLaunch,
        wake: Arc<dyn Fn() + Send + Sync>,
    ) -> Self {
        Self::spawn(project_root, preset, launch, wake)
    }

    pub fn start_process(
        project_root: PathBuf,
        command: PathBuf,
        args: Vec<String>,
        wake: Arc<dyn Fn() + Send + Sync>,
    ) -> Self {
        Self::spawn(
            project_root,
            &super::catalog()[0],
            ServerLaunch::Custom {
                command: command.to_string_lossy().into_owned(),
                args,
            },
            wake,
        )
    }

    fn spawn(
        project_root: PathBuf,
        preset: &'static Preset,
        launch: ServerLaunch,
        wake: Arc<dyn Fn() + Send + Sync>,
    ) -> Self {
        let (input_tx, input_rx) = std::sync::mpsc::sync_channel(COMMAND_CAPACITY);
        let (event_tx, event_rx) = std::sync::mpsc::sync_channel(EVENT_CAPACITY);
        let worker_input = input_tx.clone();
        let worker_events = event_tx.clone();
        let worker_wake = Arc::clone(&wake);
        let worker = thread::Builder::new()
            .name(format!("editur-lsp-{}", preset.id.as_str()))
            .spawn(move || {
                run(
                    project_root,
                    preset,
                    launch,
                    input_rx,
                    worker_input,
                    worker_events,
                    worker_wake,
                )
            });
        let worker = match worker {
            Ok(worker) => Some(worker),
            Err(error) => {
                let _ = event_tx.try_send(Event::StateChanged(ServerStatus::Failed(format!(
                    "cannot start LSP controller thread: {error}"
                ))));
                (wake)();
                None
            }
        };
        Self {
            input: input_tx,
            events: event_rx,
            worker,
        }
    }

    pub fn send(&self, command: Command) -> Result<(), String> {
        self.input
            .try_send(Input::Command(command))
            .map_err(|error| format!("LSP command could not be sent: {error}"))
    }

    pub fn events(&self) -> &Receiver<Event> {
        &self.events
    }
}

impl Drop for Controller {
    fn drop(&mut self) {
        let Some(worker) = self.worker.take() else {
            return;
        };
        loop {
            match self.input.try_send(Input::Command(Command::Shutdown)) {
                Ok(()) | Err(std::sync::mpsc::TrySendError::Disconnected(_)) => break,
                Err(std::sync::mpsc::TrySendError::Full(_)) => {
                    self.events.try_iter().for_each(drop);
                    if worker.is_finished() {
                        break;
                    }
                    thread::park_timeout(Duration::from_millis(1));
                }
            }
        }
        while !worker.is_finished() {
            self.events.try_iter().for_each(drop);
            thread::park_timeout(Duration::from_millis(1));
        }
        let _ = worker.join();
    }
}

struct Document {
    language_id: String,
    text: String,
    last_sent: String,
    revision: u64,
    version: i32,
    opened: bool,
    change_due: Option<Instant>,
    save_pending: bool,
}

struct Process {
    child: Child,
    #[cfg(windows)]
    job: Option<crate::agent::WindowsJob>,
    stdin: ChildStdin,
    stdout: Option<JoinHandle<()>>,
    stderr_worker: Option<JoinHandle<()>>,
    reader_running: Arc<AtomicBool>,
    stderr: Arc<Mutex<String>>,
    capabilities: Option<ServerCapabilities>,
    next_id: u64,
    pending: HashMap<u64, PendingRequest>,
    completed: HashSet<u64>,
    completed_order: VecDeque<u64>,
    latest_completion: HashMap<PathBuf, u64>,
    latest_hover: HashMap<PathBuf, u64>,
    latest_definition: HashMap<PathBuf, u64>,
}

enum PendingRequest {
    Completion(RequestTag),
    Hover(RequestTag),
    Definition(RequestTag),
}

struct Events {
    tx: SyncSender<Event>,
    wake: Arc<dyn Fn() + Send + Sync>,
}

impl Events {
    fn send(&self, event: Event) {
        let _ = self.tx.send(event);
        (self.wake)();
    }
}

fn run(
    project_root: PathBuf,
    preset: &'static Preset,
    mut launch: ServerLaunch,
    input: Receiver<Input>,
    input_tx: SyncSender<Input>,
    event_tx: SyncSender<Event>,
    wake: Arc<dyn Fn() + Send + Sync>,
) {
    let events = Events { tx: event_tx, wake };
    events.send(Event::StateChanged(ServerStatus::NotStarted));
    let mut documents = HashMap::<PathBuf, Document>::new();
    let mut process = None;
    let mut queued_commands = VecDeque::new();
    loop {
        let timeout = documents
            .values()
            .filter_map(|document| document.change_due)
            .min()
            .map_or(Duration::from_secs(60), |due| {
                due.saturating_duration_since(Instant::now())
            });
        let received = queued_commands.pop_front().map_or_else(
            || input.recv_timeout(timeout),
            |command| Ok(Input::Command(command)),
        );
        match received {
            Ok(Input::Command(command)) => match command {
                Command::Open(snapshot) => {
                    documents.entry(snapshot.path.clone()).or_insert(Document {
                        language_id: snapshot.language_id,
                        text: snapshot.text.clone(),
                        last_sent: snapshot.text,
                        revision: snapshot.revision,
                        version: 0,
                        opened: false,
                        change_due: None,
                        save_pending: false,
                    });
                    if process.is_none() {
                        match start_process(&project_root, preset, &launch, &input_tx, &events) {
                            Ok(started) => process = started,
                            Err(error) => fail(&events, error, String::new()),
                        }
                    } else if process
                        .as_ref()
                        .and_then(|process| process.capabilities.as_ref())
                        .is_some()
                        && let Err(error) =
                            open_document(process.as_mut().unwrap(), &snapshot.path, &mut documents)
                    {
                        fail_process(&events, &mut process, error);
                    }
                }
                Command::Change {
                    path,
                    text,
                    revision,
                } => {
                    if let Some(document) = documents.get_mut(&path) {
                        document.text = text;
                        document.revision = revision;
                        document.change_due = Some(Instant::now() + CHANGE_DELAY);
                        events.send(Event::DiagnosticsStale(path));
                    }
                }
                Command::Save(path) => {
                    if let Some(document) = documents.get_mut(&path) {
                        document.save_pending = true;
                    }
                    if let Some(active) = process.as_mut()
                        && active.capabilities.is_some()
                    {
                        match flush_document(active, &path, &mut documents)
                            .and_then(|()| save_document(active, &path, &documents))
                        {
                            Ok(()) => documents.get_mut(&path).unwrap().save_pending = false,
                            Err(error) => fail_process(&events, &mut process, error),
                        }
                    }
                }
                Command::Close(path) => {
                    if let Some(active) = process.as_mut()
                        && active.capabilities.is_some()
                        && let Err(error) = flush_document(active, &path, &mut documents)
                            .and_then(|()| close_document(active, &path, &documents))
                    {
                        fail_process(&events, &mut process, error);
                    }
                    documents.remove(&path);
                    if let Some(active) = process.as_mut() {
                        active.latest_completion.remove(&path);
                        active.latest_hover.remove(&path);
                        active.latest_definition.remove(&path);
                    }
                    if documents.is_empty() {
                        if let Some(active) = process.take() {
                            stop_process(
                                active,
                                &project_root,
                                &input,
                                &mut queued_commands,
                                &events,
                            );
                        }
                        events.send(Event::StateChanged(ServerStatus::Stopped));
                    }
                }
                Command::Complete { tag, trigger } => {
                    if let Some(active) = process.as_mut()
                        && let Err(error) = flush_document(active, &tag.path, &mut documents)
                            .and_then(|()| request_completion(active, tag, trigger, &documents))
                    {
                        events.send(Event::ServerMessage(error));
                    }
                }
                Command::Hover(tag) => {
                    if let Some(active) = process.as_mut()
                        && let Err(error) = flush_document(active, &tag.path, &mut documents)
                            .and_then(|()| request_hover(active, tag, &documents))
                    {
                        events.send(Event::ServerMessage(error));
                    }
                }
                Command::Definition(tag) => {
                    if let Some(active) = process.as_mut()
                        && let Err(error) = flush_document(active, &tag.path, &mut documents)
                            .and_then(|()| request_definition(active, tag, &documents))
                    {
                        events.send(Event::ServerMessage(error));
                    }
                }
                Command::Restart(next) => {
                    launch = next;
                    if let Some(active) = process.take() {
                        stop_process(active, &project_root, &input, &mut queued_commands, &events);
                    }
                    for document in documents.values_mut() {
                        document.opened = false;
                    }
                    if !documents.is_empty() {
                        match start_process(&project_root, preset, &launch, &input_tx, &events) {
                            Ok(started) => process = started,
                            Err(error) => fail(&events, error, String::new()),
                        }
                    } else {
                        events.send(Event::StateChanged(ServerStatus::Stopped));
                    }
                }
                Command::Rescan => {
                    if let Some(active) = process.take() {
                        stop_process(active, &project_root, &input, &mut queued_commands, &events);
                    }
                    for document in documents.values_mut() {
                        document.opened = false;
                    }
                    if !documents.is_empty() {
                        match start_process(&project_root, preset, &launch, &input_tx, &events) {
                            Ok(started) => process = started,
                            Err(error) => fail(&events, error, String::new()),
                        }
                    } else {
                        probe_launch(preset, &launch, &events);
                    }
                }
                Command::Shutdown => {
                    if let Some(active) = process.take() {
                        stop_process(active, &project_root, &input, &mut queued_commands, &events);
                    }
                    break;
                }
            },
            Ok(Input::Wire(message)) => match message {
                Ok(message) => {
                    if let Err(error) = handle_wire(
                        message,
                        &project_root,
                        &mut process,
                        &mut documents,
                        &events,
                    ) {
                        fail_process(&events, &mut process, error);
                    }
                }
                Err(error) if process.is_some() => fail_process(&events, &mut process, error),
                Err(_) => {}
            },
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                if let Some(active) = process.as_mut() {
                    let due = documents
                        .iter()
                        .filter(|(_, document)| {
                            document.change_due.is_some_and(|due| due <= Instant::now())
                        })
                        .map(|(path, _)| path.clone())
                        .collect::<Vec<_>>();
                    for path in due {
                        if let Err(error) = flush_document(active, &path, &mut documents) {
                            fail_process(&events, &mut process, error);
                            break;
                        }
                    }
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
}

fn start_process(
    project_root: &Path,
    preset: &Preset,
    launch: &ServerLaunch,
    input: &SyncSender<Input>,
    events: &Events,
) -> Result<Option<Process>, String> {
    let initialize = initialize_params(project_root)?;
    let (command, args) = match launch {
        ServerLaunch::Off => {
            events.send(Event::StateChanged(ServerStatus::Stopped));
            return Ok(None);
        }
        ServerLaunch::Auto => (
            preset.command,
            preset.args.iter().map(|arg| (*arg).to_owned()).collect(),
        ),
        ServerLaunch::Custom { command, args } => (command.as_str(), args.clone()),
    };
    let Some(executable) = discover_executable(command, None)? else {
        events.send(Event::StateChanged(ServerStatus::NotFound));
        return Ok(None);
    };
    events.send(Event::StateChanged(ServerStatus::Starting));
    let mut command = ProcessCommand::new(executable);
    command
        .args(args)
        .current_dir(project_root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    configure_process_tree(&mut command);
    #[cfg(windows)]
    let (_, job) = crate::agent::new_windows_job()?;
    let mut child = command
        .spawn()
        .map_err(|error| format!("cannot start {}: {error}", preset.name))?;
    #[cfg(windows)]
    if let Err(error) = job.assign_child(&child) {
        let _ = child.kill();
        let _ = child.wait();
        return Err(error);
    }
    let Some(stdout) = child.stdout.take() else {
        abort_child(&mut child);
        return Err("language server stdout was not piped".into());
    };
    let Some(stderr) = child.stderr.take() else {
        abort_child(&mut child);
        return Err("language server stderr was not piped".into());
    };
    let Some(stdin) = child.stdin.take() else {
        abort_child(&mut child);
        return Err("language server stdin was not piped".into());
    };
    let reader_input = input.clone();
    let reader_running = Arc::new(AtomicBool::new(true));
    let stdout_running = Arc::clone(&reader_running);
    let stdout = match thread::Builder::new()
        .name("editur-lsp-stdout".into())
        .spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                let message = read_frame(&mut reader).and_then(|body| decode_message(&body));
                let failed = message.is_err();
                let mut input = Input::Wire(message);
                loop {
                    match reader_input.try_send(input) {
                        Ok(()) => break,
                        Err(std::sync::mpsc::TrySendError::Full(returned))
                            if stdout_running.load(Ordering::Acquire) =>
                        {
                            input = returned;
                            thread::park_timeout(Duration::from_millis(1));
                        }
                        Err(_) => return,
                    }
                }
                if failed {
                    break;
                }
            }
        }) {
        Ok(worker) => worker,
        Err(error) => {
            abort_child(&mut child);
            return Err(format!("cannot start LSP stdout reader: {error}"));
        }
    };
    let retained_stderr = Arc::new(Mutex::new(String::new()));
    let thread_stderr = Arc::clone(&retained_stderr);
    let stderr_worker = match thread::Builder::new()
        .name("editur-lsp-stderr".into())
        .spawn(move || {
            let mut reader = BufReader::new(stderr);
            let mut chunk = [0; 4 * 1024];
            loop {
                match reader.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(read) => append_bounded(
                        &thread_stderr,
                        &String::from_utf8_lossy(&chunk[..read]),
                        STDERR_LIMIT,
                    ),
                }
            }
        }) {
        Ok(worker) => worker,
        Err(error) => {
            abort_child(&mut child);
            reader_running.store(false, Ordering::Release);
            #[cfg(windows)]
            drop(job);
            drop(stdin);
            let _ = stdout.join();
            return Err(format!("cannot start LSP stderr reader: {error}"));
        }
    };
    let mut process = Process {
        child,
        #[cfg(windows)]
        job: Some(job),
        stdin,
        stdout: Some(stdout),
        stderr_worker: Some(stderr_worker),
        reader_running,
        stderr: retained_stderr,
        capabilities: None,
        next_id: 2,
        pending: HashMap::new(),
        completed: HashSet::new(),
        completed_order: VecDeque::new(),
        latest_completion: HashMap::new(),
        latest_hover: HashMap::new(),
        latest_definition: HashMap::new(),
    };
    if let Err(error) = send_request(&mut process.stdin, 1, "initialize", initialize) {
        terminate_and_join(process);
        return Err(error);
    }
    Ok(Some(process))
}

fn probe_launch(preset: &Preset, launch: &ServerLaunch, events: &Events) {
    let command = match launch {
        ServerLaunch::Auto => preset.command,
        ServerLaunch::Custom { command, .. } => command,
        ServerLaunch::Off => {
            events.send(Event::StateChanged(ServerStatus::Stopped));
            return;
        }
    };
    match discover_executable(command, None) {
        Ok(Some(_)) => events.send(Event::StateChanged(ServerStatus::NotStarted)),
        Ok(None) => events.send(Event::StateChanged(ServerStatus::NotFound)),
        Err(error) => events.send(Event::StateChanged(ServerStatus::Failed(error))),
    }
}

fn handle_wire(
    message: WireMessage,
    project_root: &Path,
    process: &mut Option<Process>,
    documents: &mut HashMap<PathBuf, Document>,
    events: &Events,
) -> Result<(), String> {
    let active = process
        .as_mut()
        .ok_or_else(|| "LSP message arrived without a process".to_owned())?;
    match message {
        WireMessage::Response { id: 1, result } if active.capabilities.is_none() => {
            let result = result.map_err(|error| rpc_error("initialize", error))?;
            let capabilities = parse_capabilities(&result)?;
            active.capabilities = Some(capabilities.clone());
            send_notification(&mut active.stdin, "initialized", json!({}))?;
            let paths = documents.keys().cloned().collect::<Vec<_>>();
            for path in paths {
                open_document(active, &path, documents)?;
            }
            let saves = documents
                .iter()
                .filter(|(_, document)| document.save_pending)
                .map(|(path, _)| path.clone())
                .collect::<Vec<_>>();
            for path in saves {
                save_document(active, &path, documents)?;
                documents.get_mut(&path).unwrap().save_pending = false;
            }
            events.send(Event::StateChanged(ServerStatus::Ready(capabilities)));
        }
        WireMessage::Response { id, result } => {
            let Some(pending) = active.pending.remove(&id) else {
                return Err(if active.completed.contains(&id) {
                    format!("duplicate LSP response ID {id}")
                } else {
                    format!("unknown LSP response ID {id}")
                });
            };
            active.completed.insert(id);
            active.completed_order.push_back(id);
            if active.completed_order.len() > 1_024
                && let Some(oldest) = active.completed_order.pop_front()
            {
                active.completed.remove(&oldest);
            }
            match pending {
                PendingRequest::Completion(tag) => {
                    if active.latest_completion.get(&tag.path) == Some(&id)
                        && documents
                            .get(&tag.path)
                            .is_some_and(|document| document.revision == tag.revision)
                    {
                        match result {
                            Ok(result) => {
                                let (items, truncated) =
                                    normalize_completion(result, documents.get(&tag.path))?;
                                events.send(Event::Completion {
                                    tag,
                                    items,
                                    truncated,
                                });
                            }
                            Err(error) => {
                                events.send(Event::ServerMessage(rpc_error("completion", error)))
                            }
                        }
                    }
                }
                PendingRequest::Hover(tag) => {
                    if active.latest_hover.get(&tag.path) == Some(&id)
                        && documents
                            .get(&tag.path)
                            .is_some_and(|document| document.revision == tag.revision)
                    {
                        match result {
                            Ok(result) => {
                                let content = normalize_hover(result, documents.get(&tag.path))?;
                                events.send(Event::Hover { tag, content });
                            }
                            Err(error) => {
                                events.send(Event::ServerMessage(rpc_error("hover", error)))
                            }
                        }
                    }
                }
                PendingRequest::Definition(tag) => {
                    if active.latest_definition.get(&tag.path) == Some(&id)
                        && documents
                            .get(&tag.path)
                            .is_some_and(|document| document.revision == tag.revision)
                    {
                        match result {
                            Ok(result) => {
                                let (locations, truncated) = normalize_definitions(result)?;
                                events.send(Event::Definitions {
                                    tag,
                                    locations,
                                    truncated,
                                });
                            }
                            Err(error) => {
                                events.send(Event::ServerMessage(rpc_error("definition", error)))
                            }
                        }
                    }
                }
            }
        }
        WireMessage::Notification { method, params } => {
            if method == "textDocument/publishDiagnostics" {
                publish_diagnostics(params, documents, events)?;
            } else if std::env::var("EDITUR_LOG").as_deref() == Ok("debug") {
                eprintln!("editur: LSP <- {method}");
            }
        }
        WireMessage::Request { id, method, params } => {
            respond_server_request(
                &mut active.stdin,
                project_root,
                events,
                id,
                &method,
                &params,
            )?;
        }
    }
    Ok(())
}

fn request_definition(
    process: &mut Process,
    tag: RequestTag,
    documents: &HashMap<PathBuf, Document>,
) -> Result<(), String> {
    if !process
        .capabilities
        .as_ref()
        .is_some_and(|capabilities| capabilities.definition)
    {
        return Err("language server does not provide go to definition".into());
    }
    let document = documents
        .get(&tag.path)
        .ok_or_else(|| "unknown LSP document".to_owned())?;
    if document.revision != tag.revision {
        return Err("definition context is stale".into());
    }
    if process.pending.len() >= 512 {
        return Err("too many pending LSP requests".into());
    }
    let position = super::position_for_byte(
        &document.text,
        character_to_byte(&document.text, tag.cursor),
    );
    let id = process.next_id;
    process.next_id += 1;
    send_request(
        &mut process.stdin,
        id,
        "textDocument/definition",
        json!({
            "textDocument": {"uri": file_uri(&tag.path)?},
            "position": position,
        }),
    )?;
    process.latest_definition.insert(tag.path.clone(), id);
    process.pending.insert(id, PendingRequest::Definition(tag));
    Ok(())
}

fn normalize_definitions(result: Value) -> Result<(Vec<DefinitionLocation>, bool), String> {
    const MAX_LOCATIONS: usize = 200;
    if result.is_null() {
        return Ok((Vec::new(), false));
    }
    let response: lsp_types::GotoDefinitionResponse = serde_json::from_value(result)
        .map_err(|error| format!("invalid definition response: {error}"))?;
    let raw = match response {
        lsp_types::GotoDefinitionResponse::Scalar(location) => vec![(location.uri, location.range)],
        lsp_types::GotoDefinitionResponse::Array(locations) => locations
            .into_iter()
            .map(|location| (location.uri, location.range))
            .collect(),
        lsp_types::GotoDefinitionResponse::Link(locations) => locations
            .into_iter()
            .map(|location| (location.target_uri, location.target_selection_range))
            .collect(),
    };
    let truncated = raw.len() > MAX_LOCATIONS;
    let locations = raw
        .into_iter()
        .filter_map(|(uri, range)| {
            if (range.end.line, range.end.character) < (range.start.line, range.start.character) {
                return None;
            }
            let path = super::path_from_file_uri(&uri).ok()?;
            if !std::fs::metadata(&path).is_ok_and(|metadata| metadata.is_file()) {
                return None;
            }
            Some(DefinitionLocation {
                path,
                line: range.start.line,
                character: range.start.character,
                end_line: range.end.line,
                end_character: range.end.character,
            })
        })
        .take(MAX_LOCATIONS)
        .collect();
    Ok((locations, truncated))
}

fn request_hover(
    process: &mut Process,
    tag: RequestTag,
    documents: &HashMap<PathBuf, Document>,
) -> Result<(), String> {
    if !process
        .capabilities
        .as_ref()
        .is_some_and(|capabilities| capabilities.hover)
    {
        return Err("language server does not provide hover".into());
    }
    let document = documents
        .get(&tag.path)
        .ok_or_else(|| "unknown LSP document".to_owned())?;
    if document.revision != tag.revision {
        return Err("hover context is stale".into());
    }
    if process.pending.len() >= 512 {
        return Err("too many pending LSP requests".into());
    }
    let position = super::position_for_byte(
        &document.text,
        character_to_byte(&document.text, tag.cursor),
    );
    let id = process.next_id;
    process.next_id += 1;
    send_request(
        &mut process.stdin,
        id,
        "textDocument/hover",
        json!({
            "textDocument": {"uri": file_uri(&tag.path)?},
            "position": position,
        }),
    )?;
    process.latest_hover.insert(tag.path.clone(), id);
    process.pending.insert(id, PendingRequest::Hover(tag));
    Ok(())
}

fn normalize_hover(
    result: Value,
    document: Option<&Document>,
) -> Result<Option<HoverContent>, String> {
    let document =
        document.ok_or_else(|| "hover document closed before its response".to_owned())?;
    if result.is_null() {
        return Ok(None);
    }
    let hover: lsp_types::Hover = serde_json::from_value(result)
        .map_err(|error| format!("invalid hover response: {error}"))?;
    let (text, markdown) = match hover.contents {
        lsp_types::HoverContents::Scalar(marked) => marked_string(marked),
        lsp_types::HoverContents::Array(marked) => {
            let parts = marked.into_iter().map(marked_string).collect::<Vec<_>>();
            let markdown = parts.iter().any(|(_, markdown)| *markdown);
            (
                parts
                    .into_iter()
                    .map(|(text, _)| text)
                    .collect::<Vec<_>>()
                    .join("\n\n"),
                markdown,
            )
        }
        lsp_types::HoverContents::Markup(markup) => {
            (markup.value, markup.kind == lsp_types::MarkupKind::Markdown)
        }
    };
    Ok(Some(HoverContent {
        text: truncate(&text, 256 * 1024),
        markdown,
        range: hover
            .range
            .and_then(|range| super::byte_range_for_lsp_range(&document.text, range)),
    }))
}

fn marked_string(marked: lsp_types::MarkedString) -> (String, bool) {
    match marked {
        lsp_types::MarkedString::String(text) => (text, true),
        lsp_types::MarkedString::LanguageString(code) => {
            (format!("```{}\n{}\n```", code.language, code.value), true)
        }
    }
}

fn publish_diagnostics(
    params: Value,
    documents: &HashMap<PathBuf, Document>,
    events: &Events,
) -> Result<(), String> {
    const MAX_DIAGNOSTICS: usize = 5_000;
    let params: lsp_types::PublishDiagnosticsParams = serde_json::from_value(params)
        .map_err(|error| format!("invalid publishDiagnostics notification: {error}"))?;
    let Ok(path) = super::path_from_file_uri(&params.uri) else {
        return Ok(());
    };
    let Some(document) = documents.get(&path) else {
        return Ok(());
    };
    if document.change_due.is_some() || document.text != document.last_sent {
        return Ok(());
    }
    if params
        .version
        .is_some_and(|version| version < document.version)
    {
        return Ok(());
    }
    let truncated = params.diagnostics.len() > MAX_DIAGNOSTICS;
    let diagnostics = params
        .diagnostics
        .into_iter()
        .take(MAX_DIAGNOSTICS)
        .filter_map(|diagnostic| {
            let range = super::byte_range_for_lsp_range(&document.text, diagnostic.range)?;
            let severity = match diagnostic.severity {
                Some(lsp_types::DiagnosticSeverity::ERROR) => DiagnosticSeverity::Error,
                Some(lsp_types::DiagnosticSeverity::WARNING) => DiagnosticSeverity::Warning,
                Some(lsp_types::DiagnosticSeverity::HINT) => DiagnosticSeverity::Hint,
                _ => DiagnosticSeverity::Information,
            };
            let code = diagnostic.code.map(|code| match code {
                lsp_types::NumberOrString::Number(number) => number.to_string(),
                lsp_types::NumberOrString::String(string) => string,
            });
            Some(Diagnostic {
                range,
                line: diagnostic.range.start.line,
                severity,
                source: diagnostic.source.map(|source| truncate(&source, 256)),
                code: code.map(|code| truncate(&code, 256)),
                message: truncate(&diagnostic.message, 16 * 1024),
            })
        })
        .collect();
    events.send(Event::Diagnostics {
        path: path.clone(),
        revision: document.revision,
        version: params.version,
        diagnostics,
        truncated,
    });
    if truncated {
        events.send(Event::ServerMessage(format!(
            "Diagnostics for {} were truncated to {MAX_DIAGNOSTICS} items",
            path.display()
        )));
    }
    Ok(())
}

fn parse_capabilities(initialize: &Value) -> Result<ServerCapabilities, String> {
    let capabilities = initialize
        .get("capabilities")
        .ok_or_else(|| "initialize response has no capabilities".to_owned())?;
    if capabilities
        .get("positionEncoding")
        .and_then(Value::as_str)
        .is_some_and(|encoding| encoding != "utf-16")
    {
        return Err("server selected an unsupported position encoding".into());
    }
    let sync = capabilities
        .get("textDocumentSync")
        .ok_or_else(|| "server does not support document synchronization".to_owned())?;
    let (kind, open_close, save_include_text) = if let Some(kind) = sync.as_u64() {
        (kind, true, None)
    } else {
        let options = sync
            .as_object()
            .ok_or_else(|| "invalid textDocumentSync capability".to_owned())?;
        let kind = options.get("change").and_then(Value::as_u64).unwrap_or(0);
        let open_close = options
            .get("openClose")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let save = options.get("save");
        let save_include_text = match save {
            Some(Value::Bool(true)) => Some(false),
            Some(Value::Object(options)) => Some(
                options
                    .get("includeText")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            ),
            _ => None,
        };
        (kind, open_close, save_include_text)
    };
    let sync = match kind {
        1 => SyncKind::Full,
        2 => SyncKind::Incremental,
        _ => {
            return Err(
                "server does not support full or incremental document synchronization".into(),
            );
        }
    };
    let completion_triggers = capabilities
        .pointer("/completionProvider/triggerCharacters")
        .and_then(Value::as_array)
        .map(|triggers| {
            triggers
                .iter()
                .filter_map(Value::as_str)
                .take(64)
                .map(truncate_trigger)
                .collect()
        })
        .unwrap_or_default();
    Ok(ServerCapabilities {
        sync,
        open_close,
        save_include_text,
        completion: capabilities
            .get("completionProvider")
            .is_some_and(Value::is_object),
        completion_triggers,
        hover: capability_enabled(capabilities.get("hoverProvider")),
        definition: capability_enabled(capabilities.get("definitionProvider")),
    })
}

fn request_completion(
    process: &mut Process,
    tag: RequestTag,
    trigger: Option<String>,
    documents: &HashMap<PathBuf, Document>,
) -> Result<(), String> {
    if !process
        .capabilities
        .as_ref()
        .is_some_and(|capabilities| capabilities.completion)
    {
        return Err("language server does not provide completion".into());
    }
    let document = documents
        .get(&tag.path)
        .ok_or_else(|| "unknown LSP document".to_owned())?;
    if document.revision != tag.revision {
        return Err("completion context is stale".into());
    }
    if process.pending.len() >= 512 {
        return Err("too many pending LSP requests".into());
    }
    let byte = character_to_byte(&document.text, tag.cursor);
    let position = super::position_for_byte(&document.text, byte);
    let id = process.next_id;
    process.next_id += 1;
    let context = trigger.map_or_else(
        || json!({"triggerKind": 1}),
        |trigger| json!({"triggerKind": 2, "triggerCharacter": trigger}),
    );
    send_request(
        &mut process.stdin,
        id,
        "textDocument/completion",
        json!({
            "textDocument": {"uri": file_uri(&tag.path)?},
            "position": position,
            "context": context,
        }),
    )?;
    process.latest_completion.insert(tag.path.clone(), id);
    process.pending.insert(id, PendingRequest::Completion(tag));
    Ok(())
}

fn normalize_completion(
    result: Value,
    document: Option<&Document>,
) -> Result<(Vec<CompletionItem>, bool), String> {
    const MAX_ITEMS: usize = 500;
    let document =
        document.ok_or_else(|| "completion document closed before its response".to_owned())?;
    if result.is_null() {
        return Ok((Vec::new(), false));
    }
    let response: lsp_types::CompletionResponse = serde_json::from_value(result)
        .map_err(|error| format!("invalid completion response: {error}"))?;
    let raw = match response {
        lsp_types::CompletionResponse::Array(items) => items,
        lsp_types::CompletionResponse::List(list) => list.items,
    };
    let truncated = raw.len() > MAX_ITEMS;
    let items = raw
        .into_iter()
        .filter(|item| item.insert_text_format != Some(lsp_types::InsertTextFormat::SNIPPET))
        .take(MAX_ITEMS)
        .filter_map(|item| {
            let label = truncate(&item.label, 1024);
            let insert_text = truncate(
                item.insert_text.as_deref().unwrap_or(&item.label),
                64 * 1024,
            );
            let edit = match item.text_edit {
                None => None,
                Some(edit) => {
                    let (range, new_text) = match edit {
                        lsp_types::CompletionTextEdit::Edit(edit) => (edit.range, edit.new_text),
                        lsp_types::CompletionTextEdit::InsertAndReplace(edit) => {
                            (edit.replace, edit.new_text)
                        }
                    };
                    Some(CompletionEdit {
                        range: super::byte_range_for_lsp_range(&document.text, range)?,
                        new_text: truncate(&new_text, 64 * 1024),
                    })
                }
            };
            let kind = item
                .kind
                .and_then(|kind| serde_json::to_value(kind).ok()?.as_i64()?.try_into().ok());
            Some(CompletionItem {
                label,
                kind,
                detail: item.detail.map(|detail| one_line(&detail, 512)),
                insert_text,
                edit,
            })
        })
        .collect();
    Ok((items, truncated))
}

fn character_to_byte(text: &str, character: usize) -> usize {
    text.char_indices()
        .nth(character)
        .map_or(text.len(), |(byte, _)| byte)
}

fn one_line(value: &str, limit: usize) -> String {
    truncate(
        &value.split_whitespace().collect::<Vec<_>>().join(" "),
        limit,
    )
}

fn rpc_error(feature: &str, error: super::RpcError) -> String {
    let _ = error.data;
    format!(
        "{feature} failed: {} ({})",
        truncate(&error.message, 4 * 1024),
        error.code
    )
}

fn capability_enabled(value: Option<&Value>) -> bool {
    matches!(value, Some(Value::Object(_)) | Some(Value::Bool(true)))
}

fn truncate_trigger(trigger: &str) -> String {
    truncate(trigger, 16)
}

fn open_document(
    process: &mut Process,
    path: &PathBuf,
    documents: &mut HashMap<PathBuf, Document>,
) -> Result<(), String> {
    let capabilities = process
        .capabilities
        .as_ref()
        .ok_or_else(|| "server is not initialized".to_owned())?;
    let document = documents
        .get_mut(path)
        .ok_or_else(|| "unknown LSP document".to_owned())?;
    if document.opened {
        return Ok(());
    }
    if capabilities.open_close {
        send_notification(
            &mut process.stdin,
            "textDocument/didOpen",
            json!({
                "textDocument": {
                    "uri": file_uri(path)?,
                    "languageId": document.language_id,
                    "version": document.version,
                    "text": document.text,
                }
            }),
        )?;
    }
    document.last_sent.clone_from(&document.text);
    document.opened = true;
    document.change_due = None;
    Ok(())
}

fn flush_document(
    process: &mut Process,
    path: &PathBuf,
    documents: &mut HashMap<PathBuf, Document>,
) -> Result<(), String> {
    let capabilities = process
        .capabilities
        .as_ref()
        .ok_or_else(|| "server is not initialized".to_owned())?;
    let document = documents
        .get_mut(path)
        .ok_or_else(|| "unknown LSP document".to_owned())?;
    if !document.opened || document.last_sent == document.text {
        document.change_due = None;
        return Ok(());
    }
    document.version = document
        .version
        .checked_add(1)
        .ok_or_else(|| "LSP document version overflow".to_owned())?;
    let content_changes = match capabilities.sync {
        SyncKind::Full => vec![json!({"text": document.text})],
        SyncKind::Incremental => vec![
            serde_json::to_value(incremental_change(&document.last_sent, &document.text))
                .map_err(|error| format!("cannot encode document change: {error}"))?,
        ],
    };
    send_notification(
        &mut process.stdin,
        "textDocument/didChange",
        json!({
            "textDocument": {"uri": file_uri(path)?, "version": document.version},
            "contentChanges": content_changes,
        }),
    )?;
    document.last_sent.clone_from(&document.text);
    document.change_due = None;
    Ok(())
}

fn save_document(
    process: &mut Process,
    path: &PathBuf,
    documents: &HashMap<PathBuf, Document>,
) -> Result<(), String> {
    let Some(include_text) = process
        .capabilities
        .as_ref()
        .and_then(|capabilities| capabilities.save_include_text)
    else {
        return Ok(());
    };
    let document = documents
        .get(path)
        .ok_or_else(|| "unknown LSP document".to_owned())?;
    let mut params = json!({"textDocument": {"uri": file_uri(path)?}});
    if include_text {
        params["text"] = Value::String(document.text.clone());
    }
    send_notification(&mut process.stdin, "textDocument/didSave", params)
}

fn close_document(
    process: &mut Process,
    path: &PathBuf,
    documents: &HashMap<PathBuf, Document>,
) -> Result<(), String> {
    let document = documents
        .get(path)
        .ok_or_else(|| "unknown LSP document".to_owned())?;
    if document.opened
        && process
            .capabilities
            .as_ref()
            .is_some_and(|capabilities| capabilities.open_close)
    {
        send_notification(
            &mut process.stdin,
            "textDocument/didClose",
            json!({"textDocument": {"uri": file_uri(path)?}}),
        )?;
    }
    Ok(())
}

fn send_request(
    stdin: &mut ChildStdin,
    id: u64,
    method: &str,
    params: Value,
) -> Result<(), String> {
    debug_wire("->", method, Some(id));
    write_frame(
        stdin,
        &json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}),
    )
}

fn send_notification(stdin: &mut ChildStdin, method: &str, params: Value) -> Result<(), String> {
    debug_wire("->", method, None);
    write_frame(
        stdin,
        &json!({"jsonrpc": "2.0", "method": method, "params": params}),
    )
}

fn send_response(
    stdin: &mut ChildStdin,
    id: Value,
    result: Result<Value, Value>,
) -> Result<(), String> {
    let message = match result {
        Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
        Err(error) => json!({"jsonrpc": "2.0", "id": id, "error": error}),
    };
    write_frame(stdin, &message)
}

fn respond_server_request(
    stdin: &mut ChildStdin,
    project_root: &Path,
    events: &Events,
    id: Value,
    method: &str,
    params: &Value,
) -> Result<(), String> {
    let result = match method {
        "workspace/configuration" => {
            let count = params
                .get("items")
                .and_then(Value::as_array)
                .map_or(0, Vec::len);
            Ok(Value::Array(vec![Value::Null; count]))
        }
        "workspace/workspaceFolders" => Ok(json!([{
            "uri": file_uri(project_root)?,
            "name": project_root.file_name().and_then(|name| name.to_str()).unwrap_or("project")
        }])),
        "window/showMessageRequest" => {
            if let Some(message) = params.get("message").and_then(Value::as_str) {
                events.send(Event::ServerMessage(truncate(message, 4096)));
            }
            Ok(Value::Null)
        }
        "workspace/applyEdit" => Ok(json!({
            "applied": false,
            "failureReason": "Editur does not apply server-initiated workspace edits"
        })),
        _ => Err(json!({"code": -32601, "message": "Method not found"})),
    };
    send_response(stdin, id, result)
}

fn stop_process(
    mut process: Process,
    project_root: &Path,
    input: &Receiver<Input>,
    queued_commands: &mut VecDeque<Command>,
    events: &Events,
) {
    let shutdown_id = process.next_id;
    process.next_id += 1;
    let deadline = Instant::now() + SHUTDOWN_TIMEOUT;
    let mut acknowledged =
        send_request(&mut process.stdin, shutdown_id, "shutdown", Value::Null).is_ok();
    acknowledged = acknowledged
        && loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break false;
            }
            match input.recv_timeout(remaining) {
                Ok(Input::Wire(Ok(WireMessage::Response { id, result }))) if id == shutdown_id => {
                    break result.is_ok();
                }
                Ok(Input::Wire(Ok(WireMessage::Request { id, method, params }))) => {
                    if respond_server_request(
                        &mut process.stdin,
                        project_root,
                        events,
                        id,
                        &method,
                        &params,
                    )
                    .is_err()
                    {
                        break false;
                    }
                }
                Ok(Input::Wire(_)) => continue,
                Ok(Input::Command(command)) => queued_commands.push_back(command),
                Err(_) => break false,
            }
        };
    if acknowledged {
        let _ = send_notification(&mut process.stdin, "exit", Value::Null);
        while Instant::now() < deadline {
            if process.child.try_wait().ok().flatten().is_some() {
                break;
            }
            thread::sleep(Duration::from_millis(5));
        }
    }
    terminate_and_join(process);
    loop {
        match input.try_recv() {
            Ok(Input::Command(command)) => queued_commands.push_back(command),
            Ok(Input::Wire(_)) => {}
            Err(_) => break,
        }
    }
}

fn fail(events: &Events, error: String, stderr: String) {
    events.send(Event::StateChanged(ServerStatus::Failed(error.clone())));
    events.send(Event::ProcessExited { error, stderr });
}

fn fail_process(events: &Events, process: &mut Option<Process>, error: String) {
    let stderr = process.as_ref().map_or_else(String::new, |process| {
        process
            .stderr
            .lock()
            .map(|stderr| stderr.clone())
            .unwrap_or_default()
    });
    if let Some(active) = process.take() {
        terminate_and_join(active);
    }
    fail(events, error, stderr);
}

fn abort_child(child: &mut Child) {
    terminate_process_tree(child);
    let _ = child.wait();
}

fn terminate_and_join(mut process: Process) {
    process.reader_running.store(false, Ordering::Release);
    terminate_process_tree(&mut process.child);
    #[cfg(windows)]
    drop(process.job.take());
    let _ = process.child.wait();
    drop(process.stdin);
    if let Some(worker) = process.stdout.take() {
        let _ = worker.join();
    }
    if let Some(worker) = process.stderr_worker.take() {
        let _ = worker.join();
    }
}

fn append_bounded(buffer: &Mutex<String>, line: &str, limit: usize) {
    let Ok(mut buffer) = buffer.lock() else {
        return;
    };
    buffer.push_str(line);
    if buffer.len() > limit {
        let mut remove = buffer.len() - limit;
        while !buffer.is_char_boundary(remove) {
            remove += 1;
        }
        if let Some(newline) = buffer[remove..].find('\n') {
            remove += newline + 1;
        }
        let remove = remove.min(buffer.len());
        buffer.drain(..remove);
    }
}

fn truncate(value: &str, bytes: usize) -> String {
    if value.len() <= bytes {
        return value.to_owned();
    }
    let mut end = bytes;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_owned()
}

fn debug_wire(direction: &str, method: &str, id: Option<u64>) {
    if std::env::var("EDITUR_LOG").as_deref() == Ok("debug") {
        if let Some(id) = id {
            eprintln!("editur: LSP {direction} {method} ({id})");
        } else {
            eprintln!("editur: LSP {direction} {method}");
        }
    }
}

#[cfg(unix)]
fn configure_process_tree(command: &mut ProcessCommand) {
    use std::os::unix::process::CommandExt as _;
    command.process_group(0);
}

#[cfg(not(unix))]
fn configure_process_tree(_command: &mut ProcessCommand) {}

#[cfg(unix)]
fn terminate_process_tree(child: &mut Child) {
    unsafe extern "C" {
        fn kill(pid: i32, signal: i32) -> i32;
    }
    unsafe {
        kill(-(child.id() as i32), 9);
    }
    let _ = child.kill();
}

#[cfg(not(unix))]
fn terminate_process_tree(child: &mut Child) {
    let _ = child.kill();
}
