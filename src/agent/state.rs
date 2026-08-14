use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::{Path, PathBuf},
};

use super::controller::{
    CommandChoice, ConfigChoice, ConnectionState, ContentRole, DisplayContent, Event,
    InteractionKind, InteractionRequest, ModeChoice, PermissionChoice, PlanItem, PlanPhase,
    PlanProposal, Question, QuestionOption, SessionChoice, SessionTranscriptMessage, ToolActivity,
    ToolDetail, ToolOutput,
};

const MAX_ITEM_BYTES: usize = 64 * 1024;
const MAX_CHANGED_PATHS: usize = 4_096;
const MAX_CHOICES: usize = 128;
const MAX_BASELINE_FILE_BYTES: usize = 1024 * 1024;
const MAX_BASELINE_TOTAL_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq)]
pub enum TranscriptItem {
    User(String),
    Assistant(String),
    Thought(String),
    Content {
        role: ContentRole,
        content: DisplayContent,
    },
    Plan(Vec<PlanItem>),
    Tool(ToolActivity),
    Permission(PermissionCard),
    Interaction(InteractionCard),
    Error(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PermissionCard {
    pub request_id: u64,
    pub tool_call_id: String,
    pub action: String,
    pub options: Vec<PermissionChoice>,
    pub selected: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InteractionCard {
    pub request: InteractionRequest,
    pub selections: HashMap<String, Vec<String>>,
    pub answered: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UsageState {
    pub used: u64,
    pub size: u64,
    pub cost: Option<String>,
}

/// Added/removed line counts for one file, accumulated across every diff the
/// agent produced for it this session.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct FileChange {
    pub added: u64,
    pub removed: u64,
}

struct SessionLoadBackup {
    transcript: VecDeque<TranscriptItem>,
    changed_paths: HashMap<PathBuf, FileChange>,
    baselines: HashMap<PathBuf, Option<String>>,
    baseline_bytes: usize,
    tool_credits: HashMap<(String, PathBuf), FileChange>,
    tool_changes: HashMap<String, FileChange>,
    title: Option<String>,
    usage: Option<UsageState>,
}

pub struct AgentState {
    pub connection: ConnectionState,
    pub session_ready: bool,
    pub active: bool,
    pub history_available: bool,
    pub allow_run_everything: bool,
    pub prompt: String,
    pub transcript: VecDeque<TranscriptItem>,
    /// Every file the agent modified this session with its accumulated line
    /// stats; feeds the "files changed" card.
    pub changed_paths: HashMap<PathBuf, FileChange>,
    /// The content each changed file had before the agent's first recorded
    /// edit this session (`None` inside an entry = the agent created the
    /// file). Captured from the first diff per path. Oversized files are
    /// skipped, never truncated.
    pub baselines: HashMap<PathBuf, Option<String>>,
    /// Bytes currently held in `baselines`, enforcing the retention cap.
    baseline_bytes: usize,
    /// What each tool has currently contributed to `changed_paths`, so a tool
    /// that streams the same diff repeatedly replaces its contribution
    /// instead of double-counting it.
    tool_credits: HashMap<(String, PathBuf), FileChange>,
    /// Added/removed totals by tool call, for individual edit card headers.
    pub(crate) tool_changes: HashMap<String, FileChange>,
    /// Paths not yet reflected in the tree/search; drained by the refresh
    /// pass after each turn so `changed_paths` can persist for the UI.
    pub refresh_queue: HashSet<PathBuf>,
    pub current_mode: Option<String>,
    pub modes: Vec<ModeChoice>,
    pub config_options: Vec<ConfigChoice>,
    pub commands: Vec<CommandChoice>,
    pub sessions: Option<Vec<SessionChoice>>,
    pub session_id: Option<String>,
    pub title: Option<String>,
    pub usage: Option<UsageState>,
    pub diagnostics: Option<String>,
    session_load_backup: Option<SessionLoadBackup>,
}

impl Default for AgentState {
    fn default() -> Self {
        Self {
            connection: ConnectionState::Disconnected,
            session_ready: false,
            active: false,
            history_available: false,
            allow_run_everything: false,
            prompt: String::new(),
            transcript: VecDeque::new(),
            changed_paths: HashMap::new(),
            baselines: HashMap::new(),
            baseline_bytes: 0,
            tool_credits: HashMap::new(),
            tool_changes: HashMap::new(),
            refresh_queue: HashSet::new(),
            current_mode: None,
            modes: Vec::new(),
            config_options: Vec::new(),
            commands: Vec::new(),
            sessions: None,
            session_id: None,
            title: None,
            usage: None,
            diagnostics: None,
            session_load_backup: None,
        }
    }
}

impl AgentState {
    pub fn can_send(&self, buffer_saved: bool) -> bool {
        buffer_saved && self.session_ready && !self.active && !self.prompt.trim().is_empty()
    }

    pub fn waiting_permission(&self) -> bool {
        self.transcript.iter().any(|item| match item {
            TranscriptItem::Permission(card) => card.selected.is_none(),
            TranscriptItem::Interaction(card) => !card.answered,
            _ => false,
        })
    }

    pub fn decide_permission(&mut self, request_id: u64, option_id: &str) -> bool {
        let Some(card) = self.transcript.iter_mut().find_map(|item| match item {
            TranscriptItem::Permission(card) if card.request_id == request_id => Some(card),
            _ => None,
        }) else {
            return false;
        };
        if card.selected.is_some() || !card.options.iter().any(|option| option.id == option_id) {
            return false;
        }
        card.selected = Some(option_id.to_owned());
        true
    }

    pub fn answer_interaction(&mut self, request_id: u64) -> bool {
        let Some(card) = self.transcript.iter_mut().find_map(|item| match item {
            TranscriptItem::Interaction(card) if card.request.request_id == request_id => {
                Some(card)
            }
            _ => None,
        }) else {
            return false;
        };
        if card.answered {
            return false;
        }
        card.answered = true;
        true
    }

    pub fn tool_change(&self, tool_id: &str) -> Option<FileChange> {
        self.tool_changes.get(tool_id).copied()
    }

    pub fn apply(&mut self, event: Event) {
        match event {
            Event::ConnectionChanged(connection) => {
                self.session_ready = matches!(connection, ConnectionState::Ready);
                self.connection = connection;
            }
            Event::Capabilities {
                history,
                allow_run_everything,
            } => {
                self.history_available = history;
                self.allow_run_everything = allow_run_everything;
                if !history {
                    self.sessions = None;
                }
            }
            Event::SessionReady {
                current_mode,
                modes,
                config_options,
            } => {
                self.session_load_backup = None;
                self.session_ready = true;
                self.active = false;
                self.transcript.clear();
                self.changed_paths.clear();
                self.baselines.clear();
                self.baseline_bytes = 0;
                self.tool_credits.clear();
                self.tool_changes.clear();
                self.refresh_queue.clear();
                self.session_id = None;
                self.title = None;
                self.current_mode = current_mode.map(bounded);
                self.modes = modes
                    .into_iter()
                    .take(MAX_CHOICES)
                    .map(bounded_mode)
                    .collect();
                self.config_options = bounded_configs(config_options);
                self.usage = None;
                self.diagnostics = None;
            }
            Event::SessionsUpdated(sessions) => {
                self.sessions = Some(sessions.into_iter().take(MAX_CHOICES).collect());
            }
            Event::SessionLoading { title } => {
                self.session_load_backup = Some(SessionLoadBackup {
                    transcript: std::mem::take(&mut self.transcript),
                    changed_paths: std::mem::take(&mut self.changed_paths),
                    baselines: std::mem::take(&mut self.baselines),
                    baseline_bytes: std::mem::take(&mut self.baseline_bytes),
                    tool_credits: std::mem::take(&mut self.tool_credits),
                    tool_changes: std::mem::take(&mut self.tool_changes),
                    title: self.title.take(),
                    usage: self.usage.take(),
                });
                self.refresh_queue.clear();
                self.session_ready = false;
                self.active = false;
                self.connection = ConnectionState::Starting;
                self.title = title.map(bounded);
            }
            Event::SessionLoadFailed => {
                if let Some(backup) = self.session_load_backup.take() {
                    self.transcript = backup.transcript;
                    self.changed_paths = backup.changed_paths;
                    self.baselines = backup.baselines;
                    self.baseline_bytes = backup.baseline_bytes;
                    self.tool_credits = backup.tool_credits;
                    self.tool_changes = backup.tool_changes;
                    self.title = backup.title;
                    self.usage = backup.usage;
                }
                self.session_ready = true;
                self.active = false;
            }
            Event::SessionLoaded {
                current_mode,
                modes,
                config_options,
            } => {
                self.session_load_backup = None;
                self.session_ready = true;
                self.active = false;
                self.current_mode = current_mode.map(bounded);
                self.modes = modes
                    .into_iter()
                    .take(MAX_CHOICES)
                    .map(bounded_mode)
                    .collect();
                self.config_options = bounded_configs(config_options);
            }
            Event::SessionTranscriptLoaded(messages) => {
                self.session_load_backup = None;
                self.session_ready = true;
                self.active = false;
                self.transcript.clear();
                self.changed_paths.clear();
                self.baselines.clear();
                self.baseline_bytes = 0;
                self.tool_credits.clear();
                self.tool_changes.clear();
                self.refresh_queue.clear();
                for message in messages {
                    match message {
                        SessionTranscriptMessage::User(text) => {
                            self.push(TranscriptItem::User(bounded(text)));
                        }
                        SessionTranscriptMessage::Assistant(text) => {
                            self.push(TranscriptItem::Assistant(bounded(text)));
                        }
                        SessionTranscriptMessage::Thought(text) => {
                            self.push(TranscriptItem::Thought(bounded(text)));
                        }
                        SessionTranscriptMessage::Content { role, content } => {
                            self.push(TranscriptItem::Content {
                                role,
                                content: bounded_content(content),
                            });
                        }
                        SessionTranscriptMessage::Tool(tool) => {
                            self.apply(Event::ToolCallUpdated(tool));
                        }
                    }
                }
            }
            Event::ActiveSessionChanged(session_id) => {
                self.session_id = Some(bounded(session_id));
            }
            Event::ModeChanged(mode) => self.current_mode = Some(bounded(mode)),
            Event::ConfigOptionsUpdated(options) => self.config_options = bounded_configs(options),
            Event::CommandsUpdated(commands) => {
                self.commands = commands
                    .into_iter()
                    .take(MAX_CHOICES)
                    .map(|command| CommandChoice {
                        name: bounded(command.name),
                        description: bounded(command.description),
                        input_hint: command.input_hint.map(bounded),
                    })
                    .collect();
            }
            Event::SessionTitleUpdated(title) => {
                self.title = title.map(bounded);
                if let (Some(session_id), Some(title), Some(sessions)) =
                    (&self.session_id, &self.title, &mut self.sessions)
                    && let Some(session) = sessions
                        .iter_mut()
                        .find(|session| &session.id == session_id)
                {
                    session.title = Some(title.clone());
                }
            }
            Event::UserMessage(text) => {
                self.active = true;
                self.prompt.clear();
                if !text.is_empty() {
                    self.push(TranscriptItem::User(text));
                }
            }
            Event::AssistantDelta(text) => {
                if let Some(TranscriptItem::Assistant(current)) = self.transcript.back_mut() {
                    current.push_str(&text);
                } else {
                    self.push(TranscriptItem::Assistant(text));
                }
            }
            Event::ThoughtDelta(text) => {
                if let Some(TranscriptItem::Thought(current)) = self.transcript.back_mut() {
                    current.push_str(&text);
                } else {
                    self.push(TranscriptItem::Thought(text));
                }
            }
            Event::ContentReceived { role, content } => self.push(TranscriptItem::Content {
                role,
                content: bounded_content(content),
            }),
            Event::PlanUpdated(plan) => {
                let plan = plan
                    .into_iter()
                    .map(|item| PlanItem {
                        content: item.content,
                        status: bounded(item.status),
                    })
                    .collect();
                let turn_start = self
                    .transcript
                    .iter()
                    .rev()
                    .position(|item| matches!(item, TranscriptItem::User(_)))
                    .map_or(0, |distance| self.transcript.len() - distance);
                if let Some(TranscriptItem::Plan(current)) = self
                    .transcript
                    .iter_mut()
                    .skip(turn_start)
                    .find(|item| matches!(item, TranscriptItem::Plan(_)))
                {
                    *current = plan;
                } else {
                    self.push(TranscriptItem::Plan(plan));
                }
            }
            Event::ToolCallUpdated(tool) => {
                // Baselines capture the pre-edit file content before
                // `bounded_tool` truncates strings: a truncated baseline
                // would diff as garbage, so oversized files are skipped
                // inside `record_baseline` instead.
                for content in tool.detail.iter().flat_map(|detail| detail.content.iter()) {
                    if let ToolOutput::Diff { path, old_text, .. } = content {
                        self.record_baseline(path, old_text.as_deref());
                    }
                }
                let tool = bounded_tool(tool);
                let remaining = MAX_CHANGED_PATHS.saturating_sub(self.refresh_queue.len());
                self.refresh_queue.extend(
                    tool.paths
                        .iter()
                        .take(remaining)
                        .map(|tool_path| tool_path.path.clone()),
                );
                let tool_id = tool.id.clone();
                let diff_credits = tool
                    .detail
                    .iter()
                    .flat_map(|detail| {
                        detail.content.iter().filter_map(|content| match content {
                            ToolOutput::Diff {
                                path,
                                old_text,
                                new_text,
                            } => Some((path.clone(), diff_stats(old_text.as_deref(), new_text))),
                            _ => None,
                        })
                    })
                    .collect::<Vec<_>>();
                if let Some(TranscriptItem::Tool(current)) = self.transcript.iter_mut().rev().find(
                    |item| matches!(item, TranscriptItem::Tool(current) if current.id == tool.id),
                ) {
                    if let Some(title) = tool.title {
                        current.title = Some(bounded(title));
                    }
                    if tool.status.is_some() {
                        current.status = tool.status;
                    }
                    if tool.kind.is_some() {
                        current.kind = tool.kind;
                    }
                    if !tool.paths.is_empty() {
                        current.paths = tool.paths;
                    }
                    if let Some(detail) = tool.detail {
                        let detail = bounded_tool_detail(detail);
                        if let Some(current) = &mut current.detail {
                            if detail.input.is_some() {
                                current.input = detail.input;
                            }
                            if !detail.content.is_empty() {
                                let task = current
                                    .content
                                    .iter()
                                    .find(|content| matches!(content, ToolOutput::Task { .. }))
                                    .cloned();
                                current.content = detail.content;
                                if let Some(task) = task
                                    && !current
                                        .content
                                        .iter()
                                        .any(|content| matches!(content, ToolOutput::Task { .. }))
                                {
                                    current.content.insert(0, task);
                                }
                            }
                            if detail.output.is_some() {
                                current.output = detail.output;
                            }
                        } else {
                            current.detail = Some(detail);
                        }
                    }
                } else {
                    self.push(TranscriptItem::Tool(tool));
                }
                // "Changed files" means files the agent modified, judged on the
                // merged record: an Edit/Delete/Move kind marks its location
                // paths as changed, and a diff always marks its own path, even
                // when the kind arrived on an earlier update.
                let merged = self.transcript.iter().rev().find_map(|item| match item {
                    TranscriptItem::Tool(current) if current.id == tool_id => Some(current),
                    _ => None,
                });
                if let Some(merged) = merged {
                    let modifies =
                        matches!(merged.kind.as_deref(), Some("Edit" | "Delete" | "Move"));
                    let location_paths = merged
                        .paths
                        .iter()
                        .filter(|_| modifies)
                        .map(|tool_path| tool_path.path.clone())
                        .collect::<Vec<_>>();
                    for path in location_paths {
                        self.register_changed_path(path);
                    }
                }
                for (path, stats) in diff_credits {
                    self.credit_diff(&tool_id, path, stats);
                }
            }
            Event::PermissionRequested(request) => {
                self.push(TranscriptItem::Permission(PermissionCard {
                    request_id: request.request_id,
                    tool_call_id: bounded(request.tool_call_id),
                    action: bounded(request.action),
                    options: request
                        .options
                        .into_iter()
                        .take(MAX_CHOICES)
                        .map(|option| PermissionChoice {
                            id: bounded(option.id),
                            name: bounded(option.name),
                            kind: bounded(option.kind),
                        })
                        .collect(),
                    selected: None,
                }));
            }
            Event::InteractionRequested(request) => {
                self.push(TranscriptItem::Interaction(InteractionCard {
                    request: bounded_interaction(request),
                    selections: HashMap::new(),
                    answered: false,
                }));
            }
            Event::UsageUpdated { used, size, cost } => {
                self.usage = Some(UsageState {
                    used,
                    size,
                    cost: cost.map(bounded),
                });
            }
            Event::TurnFinished { cancelled } => {
                self.active = false;
                self.finalize_running_tools(if cancelled { "Cancelled" } else { "Failed" });
            }
            Event::Error(error) => self.push(TranscriptItem::Error(error)),
            Event::ProcessExited { error, diagnostics } => {
                self.active = false;
                self.session_ready = false;
                self.connection = ConnectionState::Failed(error.clone());
                self.diagnostics = (!diagnostics.is_empty()).then_some(diagnostics);
                self.push(TranscriptItem::Error(error));
                self.finalize_running_tools("Failed");
            }
        }
    }

    fn push(&mut self, item: TranscriptItem) {
        self.transcript.push_back(item);
    }

    /// Tool updates only arrive while a turn runs, so once the turn ends any
    /// tool still pending or in progress can never complete and would show
    /// "Running" forever.
    fn finalize_running_tools(&mut self, status: &str) {
        for item in &mut self.transcript {
            if let TranscriptItem::Tool(tool) = item
                && matches!(tool.status.as_deref(), Some("Pending" | "InProgress"))
            {
                tool.status = Some(status.to_owned());
            }
        }
    }

    /// Remembers the first pre-edit content the session saw for `path`
    /// (`None` = the agent created the file), giving the changed-files card
    /// a stable old side for its diff view. First write wins: a later tool
    /// editing the same file diffs against an intermediate state, not the
    /// session start. Files over the per-file or total byte caps record
    /// nothing, and the UI falls back to a plain open.
    fn record_baseline(&mut self, path: &Path, old_text: Option<&str>) {
        if self.baselines.contains_key(path) || self.baselines.len() >= MAX_CHANGED_PATHS {
            return;
        }
        let Some(text) = old_text else {
            self.baselines.insert(path.to_owned(), None);
            return;
        };
        if text.len() > MAX_BASELINE_FILE_BYTES
            || self.baseline_bytes.saturating_add(text.len()) > MAX_BASELINE_TOTAL_BYTES
        {
            return;
        }
        self.baseline_bytes += text.len();
        self.baselines
            .insert(path.to_owned(), Some(text.to_owned()));
    }

    /// Lists a modified file without line stats (kind-based detection, e.g. an
    /// Edit tool that reported a location but no diff).
    fn register_changed_path(&mut self, path: PathBuf) {
        if self.changed_paths.len() < MAX_CHANGED_PATHS || self.changed_paths.contains_key(&path) {
            self.changed_paths.entry(path).or_default();
        }
    }

    /// Applies one tool's diff stats for one file, replacing whatever that
    /// tool previously contributed for it: a streaming edit re-sends the
    /// growing diff on every update and must not be counted repeatedly.
    fn credit_diff(&mut self, tool_id: &str, path: PathBuf, stats: FileChange) {
        let key = (tool_id.to_owned(), path.clone());
        let previous = if let Some(credit) = self.tool_credits.get_mut(&key) {
            std::mem::replace(credit, stats)
        } else if self.tool_credits.len() < MAX_CHANGED_PATHS {
            self.tool_credits.insert(key, stats);
            FileChange::default()
        } else {
            return;
        };
        let tool = self.tool_changes.entry(tool_id.to_owned()).or_default();
        tool.added = tool
            .added
            .saturating_sub(previous.added)
            .saturating_add(stats.added);
        tool.removed = tool
            .removed
            .saturating_sub(previous.removed)
            .saturating_add(stats.removed);
        if self.changed_paths.len() >= MAX_CHANGED_PATHS && !self.changed_paths.contains_key(&path)
        {
            return;
        }
        let entry = self.changed_paths.entry(path).or_default();
        entry.added = entry
            .added
            .saturating_sub(previous.added)
            .saturating_add(stats.added);
        entry.removed = entry
            .removed
            .saturating_sub(previous.removed)
            .saturating_add(stats.removed);
    }
}

/// Added/removed line counts for one diff, matching the numbers the diff card
/// renders: an LCS comparison while the input is small enough, and a
/// prefix/suffix trim beyond that (the same bound and fallback as the UI's
/// diff builder in `app.rs`).
fn diff_stats(old_text: Option<&str>, new_text: &str) -> FileChange {
    let Some(old_text) = old_text else {
        return FileChange {
            added: new_text.lines().count() as u64,
            removed: 0,
        };
    };
    let old = old_text.lines().collect::<Vec<_>>();
    let new = new_text.lines().collect::<Vec<_>>();
    if (old.len() + 1).saturating_mul(new.len() + 1) <= 250_000 {
        let width = new.len() + 1;
        let mut previous = vec![0_u32; width];
        let mut current = vec![0_u32; width];
        for old_line in old.iter().rev() {
            for new_index in (0..new.len()).rev() {
                current[new_index] = if *old_line == new[new_index] {
                    previous[new_index + 1] + 1
                } else {
                    previous[new_index].max(current[new_index + 1])
                };
            }
            std::mem::swap(&mut previous, &mut current);
        }
        let common = previous[0] as u64;
        FileChange {
            added: new.len() as u64 - common,
            removed: old.len() as u64 - common,
        }
    } else {
        let prefix = old
            .iter()
            .zip(&new)
            .take_while(|(before, after)| before == after)
            .count();
        let suffix = old[prefix..]
            .iter()
            .rev()
            .zip(new[prefix..].iter().rev())
            .take_while(|(before, after)| before == after)
            .count();
        FileChange {
            added: (new.len() - prefix - suffix) as u64,
            removed: (old.len() - prefix - suffix) as u64,
        }
    }
}

fn bounded_tool(mut tool: ToolActivity) -> ToolActivity {
    tool.id = bounded(tool.id);
    tool.title = tool.title.map(bounded);
    tool.status = tool.status.map(bounded);
    tool.kind = tool.kind.map(bounded);
    tool.detail = tool.detail.map(bounded_tool_detail);
    tool.paths
        .retain(|tool_path| tool_path.path.as_os_str().as_encoded_bytes().len() <= MAX_ITEM_BYTES);
    tool.paths.truncate(MAX_CHOICES);
    tool
}

fn bounded_interaction(mut request: InteractionRequest) -> InteractionRequest {
    request.tool_call_id = bounded(request.tool_call_id);
    request.kind = match request.kind {
        InteractionKind::Questions { title, questions } => InteractionKind::Questions {
            title: bounded(title),
            questions: questions
                .into_iter()
                .take(MAX_CHOICES)
                .map(|question| Question {
                    id: bounded(question.id),
                    prompt: bounded(question.prompt),
                    options: question
                        .options
                        .into_iter()
                        .take(MAX_CHOICES)
                        .map(|option| QuestionOption {
                            id: bounded(option.id),
                            label: bounded(option.label),
                        })
                        .collect(),
                    allow_multiple: question.allow_multiple,
                })
                .collect(),
        },
        InteractionKind::Plan(plan) => InteractionKind::Plan(PlanProposal {
            name: plan.name.map(bounded),
            overview: plan.overview.map(bounded),
            plan: bounded(plan.plan),
            todos: plan
                .todos
                .into_iter()
                .take(MAX_CHOICES)
                .map(bounded_plan)
                .collect(),
            is_project: plan.is_project,
            phases: plan
                .phases
                .into_iter()
                .take(MAX_CHOICES)
                .map(|phase| PlanPhase {
                    name: bounded(phase.name),
                    todos: phase
                        .todos
                        .into_iter()
                        .take(MAX_CHOICES)
                        .map(bounded_plan)
                        .collect(),
                })
                .collect(),
        }),
    };
    request
}

fn bounded_plan(item: PlanItem) -> PlanItem {
    PlanItem {
        content: bounded(item.content),
        status: bounded(item.status),
    }
}

fn bounded_tool_detail(detail: ToolDetail) -> ToolDetail {
    ToolDetail {
        input: detail.input,
        content: detail
            .content
            .into_iter()
            .map(|content| match content {
                ToolOutput::Text(text) => ToolOutput::Text(text),
                ToolOutput::Content(content) => ToolOutput::Content(bounded_content(content)),
                ToolOutput::Diff {
                    path,
                    old_text,
                    new_text,
                } => ToolOutput::Diff {
                    path,
                    old_text,
                    new_text,
                },
                ToolOutput::Terminal(id) => ToolOutput::Terminal(bounded(id)),
                ToolOutput::Todo {
                    id,
                    content,
                    status,
                } => ToolOutput::Todo {
                    id: bounded(id),
                    content,
                    status: bounded(status),
                },
                ToolOutput::Task {
                    description,
                    prompt,
                    subagent_type,
                    model,
                    agent_id,
                    duration_ms,
                } => ToolOutput::Task {
                    description,
                    prompt,
                    subagent_type: bounded(subagent_type),
                    model: model.map(bounded),
                    agent_id: agent_id.map(bounded),
                    duration_ms,
                },
                ToolOutput::GeneratedImage {
                    description,
                    file_path,
                    reference_image_paths,
                } => ToolOutput::GeneratedImage {
                    description,
                    file_path,
                    reference_image_paths,
                },
            })
            .collect(),
        output: detail.output,
    }
}

fn bounded_content(content: DisplayContent) -> DisplayContent {
    match content {
        DisplayContent::Image {
            mime_type,
            uri,
            encoded_bytes,
            data,
        } => DisplayContent::Image {
            mime_type: bounded(mime_type),
            uri: uri.map(bounded),
            encoded_bytes,
            data,
        },
        DisplayContent::Audio {
            mime_type,
            encoded_bytes,
        } => DisplayContent::Audio {
            mime_type: bounded(mime_type),
            encoded_bytes,
        },
        DisplayContent::ResourceLink {
            name,
            title,
            uri,
            description,
            mime_type,
            size,
        } => DisplayContent::ResourceLink {
            name: bounded(name),
            title: title.map(bounded),
            uri: bounded(uri),
            description: description.map(bounded),
            mime_type: mime_type.map(bounded),
            size,
        },
        DisplayContent::TextResource {
            uri,
            mime_type,
            text,
        } => DisplayContent::TextResource {
            uri: bounded(uri),
            mime_type: mime_type.map(bounded),
            text,
        },
        DisplayContent::BlobResource {
            uri,
            mime_type,
            encoded_bytes,
        } => DisplayContent::BlobResource {
            uri: bounded(uri),
            mime_type: mime_type.map(bounded),
            encoded_bytes,
        },
    }
}

fn bounded_mode(mode: ModeChoice) -> ModeChoice {
    ModeChoice {
        id: bounded(mode.id),
        name: bounded(mode.name),
        description: mode.description.map(bounded),
    }
}

fn bounded_configs(options: Vec<ConfigChoice>) -> Vec<ConfigChoice> {
    options
        .into_iter()
        .take(MAX_CHOICES)
        .map(|option| ConfigChoice {
            id: bounded(option.id),
            name: bounded(option.name),
            description: option.description.map(bounded),
            value: match option.value {
                super::controller::ConfigValue::Select(value) => {
                    super::controller::ConfigValue::Select(bounded(value))
                }
                value => value,
            },
            options: option
                .options
                .into_iter()
                .take(MAX_CHOICES)
                .map(|value| super::controller::ConfigValueChoice {
                    id: bounded(value.id),
                    name: bounded(value.name),
                    description: value.description.map(bounded),
                })
                .collect(),
        })
        .collect()
}

fn bounded(mut text: String) -> String {
    if text.len() > MAX_ITEM_BYTES {
        let mut end = MAX_ITEM_BYTES;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
        text.push('…');
    }
    text
}
