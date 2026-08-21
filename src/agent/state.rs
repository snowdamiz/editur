use std::{
    collections::{HashMap, HashSet, VecDeque},
    fs::File,
    io::{Read as _, Seek as _, SeekFrom, Write as _},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use super::controller::{
    CommandChoice, ConfigChoice, ConnectionState, ContentRole, DisplayContent, Event, GoalState,
    InteractionKind, InteractionRequest, ModeChoice, PermissionChoice, PlanItem, PlanPhase,
    PlanProposal, Question, QuestionOption, SessionChoice, SessionTranscriptMessage, ToolActivity,
    ToolDetail, ToolOutput,
};
use super::external_sessions::{BoundedHandoff, ExternalMessage, ExternalTool};

const MAX_TRANSCRIPT_BYTES: usize = 16 * 1024 * 1024;
const MAX_ITEM_BYTES: usize = 64 * 1024;
const MAX_TRANSCRIPT_ITEMS: usize = 2_048;
const TRANSCRIPT_PAGE_ITEMS: usize = 256;
const MAX_CHANGED_PATHS: usize = 4_096;
const MAX_CHOICES: usize = 128;
pub(crate) const MAX_BASELINE_FILE_BYTES: usize = 1024 * 1024;
const MAX_BASELINE_TOTAL_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub enum TranscriptItem {
    User(String),
    Assistant(String),
    AccountSwitch {
        from: String,
        to: String,
    },
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

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PermissionCard {
    pub request_id: u64,
    pub tool_call_id: String,
    pub action: String,
    pub options: Vec<PermissionChoice>,
    pub selected: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
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
/// agent produced for it this session. Kept beside the resident transcript
/// page because old items can be paged to disk.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct FileChange {
    pub added: u64,
    pub removed: u64,
}

#[derive(Clone, Copy)]
struct ArchivedTranscriptItem {
    offset: u64,
    len: u64,
}

#[derive(Default)]
struct TranscriptArchive {
    file: Option<File>,
    earlier: Vec<ArchivedTranscriptItem>,
    later: Vec<ArchivedTranscriptItem>,
}

impl TranscriptArchive {
    fn store(&mut self, item: &TranscriptItem) -> Result<ArchivedTranscriptItem, String> {
        let bytes = bincode::serialize(item)
            .map_err(|error| format!("cannot archive agent transcript: {error}"))?;
        let file = match &mut self.file {
            Some(file) => file,
            None => self.file.insert(
                tempfile::tempfile()
                    .map_err(|error| format!("cannot create agent transcript archive: {error}"))?,
            ),
        };
        let offset = file
            .seek(SeekFrom::End(0))
            .map_err(|error| format!("cannot seek agent transcript archive: {error}"))?;
        file.write_all(&bytes)
            .map_err(|error| format!("cannot write agent transcript archive: {error}"))?;
        Ok(ArchivedTranscriptItem {
            offset,
            len: bytes.len() as u64,
        })
    }

    fn load(&mut self, item: ArchivedTranscriptItem) -> Result<TranscriptItem, String> {
        let file = self
            .file
            .as_mut()
            .ok_or_else(|| "agent transcript archive is unavailable".to_owned())?;
        file.seek(SeekFrom::Start(item.offset))
            .map_err(|error| format!("cannot seek agent transcript archive: {error}"))?;
        let mut bytes = vec![0; item.len as usize];
        file.read_exact(&mut bytes)
            .map_err(|error| format!("cannot read agent transcript archive: {error}"))?;
        bincode::deserialize(&bytes)
            .map_err(|error| format!("cannot decode agent transcript archive: {error}"))
    }
}

struct SessionLoadBackup {
    transcript: VecDeque<TranscriptItem>,
    transcript_bytes: usize,
    transcript_records: VecDeque<Option<ArchivedTranscriptItem>>,
    transcript_archive: TranscriptArchive,
    changed_paths: HashMap<PathBuf, FileChange>,
    baselines: HashMap<PathBuf, Option<String>>,
    baseline_bytes: usize,
    tool_credits: HashMap<(String, PathBuf), FileChange>,
    tool_changes: HashMap<String, FileChange>,
    title: Option<String>,
    usage: Option<UsageState>,
    goal: Option<GoalState>,
}

pub struct AgentState {
    pub connection: ConnectionState,
    pub session_ready: bool,
    pub active: bool,
    pub history_available: bool,
    pub allow_run_everything: bool,
    pub steering: bool,
    pub goal_actions: Vec<String>,
    pub prompt: String,
    pub transcript: VecDeque<TranscriptItem>,
    transcript_bytes: usize,
    transcript_records: VecDeque<Option<ArchivedTranscriptItem>>,
    transcript_archive: TranscriptArchive,
    transcript_streaming: bool,
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
    pub goal: Option<GoalState>,
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
            steering: false,
            goal_actions: Vec::new(),
            prompt: String::new(),
            transcript: VecDeque::new(),
            transcript_bytes: 0,
            transcript_records: VecDeque::new(),
            transcript_archive: TranscriptArchive::default(),
            transcript_streaming: false,
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
            goal: None,
            diagnostics: None,
            session_load_backup: None,
        }
    }
}

impl AgentState {
    pub fn can_send(&self, buffer_saved: bool) -> bool {
        buffer_saved
            && self.session_ready
            && (!self.active || self.steering)
            && !self.prompt.trim().is_empty()
    }

    pub fn waiting_permission(&self) -> bool {
        self.transcript.iter().any(|item| match item {
            TranscriptItem::Permission(card) => card.selected.is_none(),
            TranscriptItem::Interaction(card) => !card.answered,
            _ => false,
        })
    }

    pub fn account_handoff_prompt(
        &mut self,
        from: &str,
        to: &str,
        original_prompt: &str,
    ) -> Result<String, String> {
        self.load_latest_transcript()?;
        let marker = account_handoff_marker(from, to)?;
        let prefix = format!(
            "{marker}\nContinue the interrupted conversation below in the current workspace. Treat it as prior context and do not repeat completed work.\n\n<editur-account-conversation>\n"
        );
        let original_prompt = bounded_handoff_original(original_prompt);
        let suffix = format!(
            "\n</editur-account-conversation>\n\n<interrupted-user-message>\n{original_prompt}\n</interrupted-user-message>\n\nThe previous account's usage plan was exhausted during the active turn. Inspect the current workspace before editing, continue the unfinished work, and do not repeat changes already completed."
        );
        let mut handoff = BoundedHandoff::new(prefix, suffix)?;
        for record in self.transcript_archive.earlier.clone() {
            let item = self.transcript_archive.load(record)?;
            if let Some(message) = transcript_handoff_message(&item) {
                handoff.push(message);
            }
        }
        for item in &self.transcript {
            if let Some(message) = transcript_handoff_message(item) {
                handoff.push(message);
            }
        }
        Ok(handoff.finish())
    }

    pub fn restore_account_handoff(&mut self, transcript: VecDeque<TranscriptItem>) {
        self.transcript.clear();
        self.transcript_bytes = 0;
        self.transcript_records.clear();
        self.transcript_archive = TranscriptArchive::default();
        for item in transcript {
            self.push(item);
        }
        self.trim();
    }

    pub fn record_account_switch(&mut self, from: String, to: String) {
        self.push(TranscriptItem::AccountSwitch { from, to });
        self.trim();
    }

    pub fn decide_permission(&mut self, request_id: u64, option_id: &str) -> bool {
        let Some(index) = self.transcript.iter().position(
            |item| matches!(item, TranscriptItem::Permission(card) if card.request_id == request_id),
        ) else {
            return false;
        };
        let before = item_size(&self.transcript[index]);
        let TranscriptItem::Permission(card) = &mut self.transcript[index] else {
            return false;
        };
        if card.selected.is_some() || !card.options.iter().any(|option| option.id == option_id) {
            return false;
        }
        card.selected = Some(option_id.to_owned());
        let after = item_size(&self.transcript[index]);
        self.replace_transcript_bytes(before, after);
        true
    }

    pub fn answer_interaction(&mut self, request_id: u64) -> bool {
        let Some(index) = self.transcript.iter().position(
            |item| matches!(item, TranscriptItem::Interaction(card) if card.request.request_id == request_id),
        ) else {
            return false;
        };
        let before = item_size(&self.transcript[index]);
        let TranscriptItem::Interaction(card) = &mut self.transcript[index] else {
            return false;
        };
        if card.answered {
            return false;
        }
        if let InteractionKind::Questions { questions, .. } = &card.request.kind {
            for question in questions.iter().filter(|question| question.secret) {
                card.selections.remove(&question.id);
            }
        }
        card.answered = true;
        let after = item_size(&self.transcript[index]);
        self.replace_transcript_bytes(before, after);
        true
    }

    pub fn tool_change(&self, tool_id: &str) -> Option<FileChange> {
        self.tool_changes.get(tool_id).copied()
    }

    pub fn apply(&mut self, event: Event) {
        if !self.transcript_archive.later.is_empty()
            && matches!(&event, Event::UserMessage(_))
            && let Err(error) = self.load_latest_transcript()
        {
            self.diagnostics = Some(error);
        }
        match event {
            Event::ConnectionChanged(connection) => {
                self.session_ready = matches!(connection, ConnectionState::Ready);
                self.connection = connection;
            }
            Event::Capabilities {
                history,
                allow_run_everything,
                steering,
                goal_actions,
            } => {
                self.history_available = history;
                self.allow_run_everything = allow_run_everything;
                self.steering = steering;
                self.goal_actions = goal_actions;
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
                self.transcript_bytes = 0;
                self.transcript_records.clear();
                self.transcript_archive = TranscriptArchive::default();
                self.transcript_streaming = false;
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
                self.goal = None;
                self.diagnostics = None;
            }
            Event::SessionsUpdated(sessions) => {
                self.sessions = Some(sessions.into_iter().take(MAX_CHOICES).collect());
            }
            Event::ProjectSessionsUpdated { .. } => {}
            Event::SessionLoading { title } => {
                self.session_load_backup = Some(SessionLoadBackup {
                    transcript: std::mem::take(&mut self.transcript),
                    transcript_bytes: std::mem::take(&mut self.transcript_bytes),
                    transcript_records: std::mem::take(&mut self.transcript_records),
                    transcript_archive: std::mem::take(&mut self.transcript_archive),
                    changed_paths: std::mem::take(&mut self.changed_paths),
                    baselines: std::mem::take(&mut self.baselines),
                    baseline_bytes: std::mem::take(&mut self.baseline_bytes),
                    tool_credits: std::mem::take(&mut self.tool_credits),
                    tool_changes: std::mem::take(&mut self.tool_changes),
                    title: self.title.take(),
                    usage: self.usage.take(),
                    goal: self.goal.take(),
                });
                self.refresh_queue.clear();
                self.session_ready = false;
                self.active = false;
                self.transcript_streaming = false;
                self.connection = ConnectionState::Starting;
                self.title = title.map(bounded);
            }
            Event::SessionLoadFailed => {
                if let Some(backup) = self.session_load_backup.take() {
                    self.transcript = backup.transcript;
                    self.transcript_bytes = backup.transcript_bytes;
                    self.transcript_records = backup.transcript_records;
                    self.transcript_archive = backup.transcript_archive;
                    self.changed_paths = backup.changed_paths;
                    self.baselines = backup.baselines;
                    self.baseline_bytes = backup.baseline_bytes;
                    self.tool_credits = backup.tool_credits;
                    self.tool_changes = backup.tool_changes;
                    self.title = backup.title;
                    self.usage = backup.usage;
                    self.goal = backup.goal;
                }
                self.session_ready = true;
                self.active = false;
                self.transcript_streaming = false;
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
            Event::SessionTranscriptStarted => {
                self.session_load_backup = None;
                self.session_ready = false;
                self.active = false;
                self.transcript.clear();
                self.transcript_bytes = 0;
                self.transcript_records.clear();
                self.transcript_archive = TranscriptArchive::default();
                self.changed_paths.clear();
                self.baselines.clear();
                self.baseline_bytes = 0;
                self.tool_credits.clear();
                self.tool_changes.clear();
                self.refresh_queue.clear();
                self.transcript_streaming = true;
            }
            Event::SessionTranscriptLoaded(messages) => {
                if !self.transcript_streaming {
                    self.session_load_backup = None;
                    self.session_ready = true;
                    self.active = false;
                    self.transcript.clear();
                    self.transcript_bytes = 0;
                    self.transcript_records.clear();
                    self.transcript_archive = TranscriptArchive::default();
                    self.changed_paths.clear();
                    self.baselines.clear();
                    self.baseline_bytes = 0;
                    self.tool_credits.clear();
                    self.tool_changes.clear();
                    self.refresh_queue.clear();
                }
                for message in messages {
                    match message {
                        SessionTranscriptMessage::User(text) => {
                            if let Some((from, to)) = parse_account_handoff_marker(&text) {
                                self.push(TranscriptItem::AccountSwitch { from, to });
                            } else {
                                self.push(TranscriptItem::User(bounded(text)));
                            }
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
            Event::SessionTranscriptFinished => {
                self.transcript_streaming = false;
                self.session_ready = true;
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
                if let Some((from, to)) = parse_account_handoff_marker(&text) {
                    self.push(TranscriptItem::AccountSwitch { from, to });
                } else if !text.is_empty() {
                    self.push(TranscriptItem::User(bounded(text)));
                }
            }
            Event::AssistantDelta(text) => {
                let text = bounded(text);
                let current = self.transcript.len().checked_sub(1).filter(|index| {
                    matches!(
                        &self.transcript[*index],
                        TranscriptItem::Assistant(current)
                            if current.len() < MAX_ITEM_BYTES && !current.ends_with('…')
                    )
                });
                if let Some(index) = current {
                    let before = item_size(&self.transcript[index]);
                    let TranscriptItem::Assistant(current) = &mut self.transcript[index] else {
                        unreachable!();
                    };
                    append_bounded(current, &text);
                    let after = item_size(&self.transcript[index]);
                    self.replace_transcript_bytes(before, after);
                } else {
                    self.push(TranscriptItem::Assistant(text));
                }
            }
            Event::ThoughtDelta(text) => {
                let text = bounded(text);
                let current = self.transcript.len().checked_sub(1).filter(|index| {
                    matches!(
                        &self.transcript[*index],
                        TranscriptItem::Thought(current)
                            if current.len() < MAX_ITEM_BYTES && !current.ends_with('…')
                    )
                });
                if let Some(index) = current {
                    let before = item_size(&self.transcript[index]);
                    let TranscriptItem::Thought(current) = &mut self.transcript[index] else {
                        unreachable!();
                    };
                    append_bounded(current, &text);
                    let after = item_size(&self.transcript[index]);
                    self.replace_transcript_bytes(before, after);
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
                    .take(MAX_CHOICES)
                    .map(|item| PlanItem {
                        content: bounded(item.content),
                        status: bounded(item.status),
                    })
                    .collect();
                let turn_start = self
                    .transcript
                    .iter()
                    .rev()
                    .position(|item| matches!(item, TranscriptItem::User(_)))
                    .map_or(0, |distance| self.transcript.len() - distance);
                if let Some(index) = self
                    .transcript
                    .iter()
                    .skip(turn_start)
                    .position(|item| matches!(item, TranscriptItem::Plan(_)))
                    .map(|index| turn_start + index)
                {
                    let before = item_size(&self.transcript[index]);
                    self.transcript[index] = TranscriptItem::Plan(plan);
                    let after = item_size(&self.transcript[index]);
                    self.replace_transcript_bytes(before, after);
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
                if let Some(index) = self.transcript.iter().rposition(
                    |item| matches!(item, TranscriptItem::Tool(current) if current.id == tool.id),
                ) {
                    let before = item_size(&self.transcript[index]);
                    let TranscriptItem::Tool(current) = &mut self.transcript[index] else {
                        unreachable!();
                    };
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
                            if detail
                                .content
                                .iter()
                                .all(|content| matches!(content, ToolOutput::Log { .. }))
                            {
                                merge_tool_logs(&mut current.content, detail.content);
                            } else if !detail.content.is_empty() {
                                let task = current
                                    .content
                                    .iter()
                                    .find(|content| matches!(content, ToolOutput::Task { .. }))
                                    .cloned();
                                let subagent_logs = if task.is_some() {
                                    current
                                        .content
                                        .iter()
                                        .filter(|content| matches!(content, ToolOutput::Log { .. }))
                                        .cloned()
                                        .collect::<Vec<_>>()
                                } else {
                                    Vec::new()
                                };
                                current.content = detail.content;
                                if let Some(task) = task
                                    && !current
                                        .content
                                        .iter()
                                        .any(|content| matches!(content, ToolOutput::Task { .. }))
                                {
                                    current.content.insert(0, task);
                                }
                                if current
                                    .content
                                    .iter()
                                    .any(|content| matches!(content, ToolOutput::Task { .. }))
                                {
                                    let missing_logs = subagent_logs
                                        .into_iter()
                                        .filter(|old| {
                                            let ToolOutput::Log { label, .. } = old else {
                                                return false;
                                            };
                                            !current.content.iter().any(|new| {
                                                matches!(new, ToolOutput::Log { label: current, .. } if current == label)
                                            })
                                        })
                                        .collect();
                                    merge_tool_logs(&mut current.content, missing_logs);
                                }
                            }
                            if detail.output.is_some() {
                                current.output = detail.output;
                            }
                        } else {
                            current.detail = Some(detail);
                        }
                    }
                    let after = item_size(&self.transcript[index]);
                    self.replace_transcript_bytes(before, after);
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
                let request = bounded_interaction(request);
                let selections = match &request.kind {
                    InteractionKind::Questions { questions, .. } => questions
                        .iter()
                        .filter(|question| !question.default_values.is_empty())
                        .map(|question| (question.id.clone(), question.default_values.clone()))
                        .collect(),
                    _ => HashMap::new(),
                };
                self.push(TranscriptItem::Interaction(InteractionCard {
                    request,
                    selections,
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
            Event::GoalUpdated(goal) => {
                self.goal = goal.map(|goal| GoalState {
                    objective: bounded(goal.objective),
                    status: bounded(goal.status),
                    iterations: goal.iterations,
                    last_reason: goal.last_reason.map(bounded),
                    token_budget: goal.token_budget,
                    tokens_used: goal.tokens_used,
                    time_used_seconds: goal.time_used_seconds,
                });
            }
            Event::TurnFailed { message, .. } => {
                self.push(TranscriptItem::Error(bounded(message)));
            }
            Event::TurnFinished { cancelled } => {
                self.active = false;
                self.finalize_running_tools(if cancelled { "Cancelled" } else { "Failed" });
            }
            Event::Error(error) => self.push(TranscriptItem::Error(bounded(error))),
            Event::ProcessExited { error, diagnostics } => {
                self.active = false;
                self.session_ready = false;
                self.connection = ConnectionState::Failed(error.clone());
                self.diagnostics = (!diagnostics.is_empty()).then(|| bounded(diagnostics));
                self.push(TranscriptItem::Error(bounded(error)));
                self.finalize_running_tools("Failed");
            }
        }
        self.trim();
    }

    fn push(&mut self, item: TranscriptItem) {
        self.transcript_bytes = self.transcript_bytes.saturating_add(item_size(&item));
        self.transcript.push_back(item);
        self.transcript_records.push_back(None);
    }

    fn replace_transcript_bytes(&mut self, before: usize, after: usize) {
        self.transcript_bytes = self
            .transcript_bytes
            .saturating_sub(before)
            .saturating_add(after);
    }

    fn trim(&mut self) {
        if let Err(error) = self.trim_from_front(false) {
            self.diagnostics = Some(error);
        }
    }

    fn trim_from_front(&mut self, keep_one: bool) -> Result<(), String> {
        self.sync_transcript_records();
        while self.transcript.len() > usize::from(keep_one)
            && (self.transcript.len() > MAX_TRANSCRIPT_ITEMS
                || self.transcript_bytes > MAX_TRANSCRIPT_BYTES)
        {
            if !self.front_can_be_archived() {
                break;
            }
            self.archive_front()?;
        }
        Ok(())
    }

    fn front_can_be_archived(&self) -> bool {
        if !self.active {
            return true;
        }
        match self.transcript.front() {
            Some(TranscriptItem::Permission(PermissionCard { selected: None, .. }))
            | Some(TranscriptItem::Interaction(InteractionCard {
                answered: false, ..
            })) => false,
            Some(TranscriptItem::Tool(ToolActivity {
                status: Some(status),
                ..
            })) => !matches!(status.as_str(), "Pending" | "InProgress"),
            _ => true,
        }
    }

    pub fn has_earlier_transcript(&self) -> bool {
        !self.transcript_archive.earlier.is_empty()
    }

    pub fn has_later_transcript(&self) -> bool {
        !self.transcript_archive.later.is_empty()
    }

    pub fn load_earlier_transcript(&mut self) -> Result<bool, String> {
        if self.active || !self.has_earlier_transcript() {
            return Ok(false);
        }
        self.sync_transcript_records();
        for _ in 0..TRANSCRIPT_PAGE_ITEMS {
            let Some(record) = self.transcript_archive.earlier.pop() else {
                break;
            };
            match self.transcript_archive.load(record) {
                Ok(item) => {
                    self.transcript_bytes = self.transcript_bytes.saturating_add(item_size(&item));
                    self.transcript.push_front(item);
                    self.transcript_records.push_front(Some(record));
                }
                Err(error) => {
                    self.transcript_archive.earlier.push(record);
                    return Err(error);
                }
            }
        }
        self.trim_from_back()?;
        Ok(true)
    }

    pub fn load_later_transcript(&mut self) -> Result<bool, String> {
        if self.active || !self.has_later_transcript() {
            return Ok(false);
        }
        self.sync_transcript_records();
        for _ in 0..TRANSCRIPT_PAGE_ITEMS {
            let Some(record) = self.transcript_archive.later.pop() else {
                break;
            };
            match self.transcript_archive.load(record) {
                Ok(item) => {
                    self.transcript_bytes = self.transcript_bytes.saturating_add(item_size(&item));
                    self.transcript.push_back(item);
                    self.transcript_records.push_back(Some(record));
                }
                Err(error) => {
                    self.transcript_archive.later.push(record);
                    return Err(error);
                }
            }
        }
        self.trim_from_front(true)?;
        Ok(true)
    }

    pub fn load_latest_transcript(&mut self) -> Result<bool, String> {
        let mut loaded = false;
        while self.has_later_transcript() {
            loaded |= self.load_later_transcript()?;
        }
        Ok(loaded)
    }

    fn sync_transcript_records(&mut self) {
        self.transcript_records.resize(self.transcript.len(), None);
    }

    fn archive_front(&mut self) -> Result<(), String> {
        let size = self.transcript.front().map_or(0, item_size);
        let record = match self.transcript_records.front().copied().flatten() {
            Some(record) => record,
            None => self
                .transcript_archive
                .store(self.transcript.front().expect("transcript front"))?,
        };
        self.transcript.pop_front();
        self.transcript_records.pop_front();
        self.transcript_bytes = self.transcript_bytes.saturating_sub(size);
        self.transcript_archive.earlier.push(record);
        Ok(())
    }

    fn archive_back(&mut self) -> Result<(), String> {
        let size = self.transcript.back().map_or(0, item_size);
        let record = match self.transcript_records.back().copied().flatten() {
            Some(record) => record,
            None => self
                .transcript_archive
                .store(self.transcript.back().expect("transcript back"))?,
        };
        self.transcript.pop_back();
        self.transcript_records.pop_back();
        self.transcript_bytes = self.transcript_bytes.saturating_sub(size);
        self.transcript_archive.later.push(record);
        Ok(())
    }

    fn trim_from_back(&mut self) -> Result<(), String> {
        while self.transcript.len() > 1
            && (self.transcript.len() > MAX_TRANSCRIPT_ITEMS
                || self.transcript_bytes > MAX_TRANSCRIPT_BYTES)
        {
            self.archive_back()?;
        }
        Ok(())
    }

    /// Tool updates only arrive while a turn runs, so once the turn ends any
    /// tool still pending or in progress can never complete and would show
    /// "Running" forever.
    fn finalize_running_tools(&mut self, status: &str) {
        for index in 0..self.transcript.len() {
            let before = item_size(&self.transcript[index]);
            let changed = if let TranscriptItem::Tool(tool) = &mut self.transcript[index]
                && matches!(tool.status.as_deref(), Some("Pending" | "InProgress"))
            {
                tool.status = Some(status.to_owned());
                true
            } else {
                false
            };
            if changed {
                let after = item_size(&self.transcript[index]);
                self.replace_transcript_bytes(before, after);
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

fn merge_tool_logs(current: &mut Vec<ToolOutput>, logs: Vec<ToolOutput>) {
    for log in logs {
        let ToolOutput::Log { label, text } = log else {
            continue;
        };
        if let Some(ToolOutput::Log {
            text: current_text,
            ..
        }) = current
            .iter_mut()
            .rev()
            .find(|content| matches!(content, ToolOutput::Log { label: current, .. } if current == &label))
        {
            append_bounded(current_text, &text);
        } else {
            current.push(ToolOutput::Log { label, text });
        }
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
                    required: question.required,
                    secret: question.secret,
                    default_values: question
                        .default_values
                        .into_iter()
                        .take(MAX_CHOICES)
                        .map(bounded)
                        .collect(),
                    value_kind: question.value_kind,
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
        InteractionKind::Url { title, url } => InteractionKind::Url {
            title: bounded(title),
            url: bounded(url),
        },
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
        input: detail.input.map(bounded),
        content: detail
            .content
            .into_iter()
            .take(MAX_CHOICES)
            .map(|content| match content {
                ToolOutput::Text(text) => ToolOutput::Text(bounded(text)),
                ToolOutput::Log { label, text } => ToolOutput::Log {
                    label: bounded(label),
                    text: bounded(text),
                },
                ToolOutput::Content(content) => ToolOutput::Content(bounded_content(content)),
                ToolOutput::Diff {
                    path,
                    old_text,
                    new_text,
                } => ToolOutput::Diff {
                    path,
                    old_text: old_text.map(bounded_arc),
                    new_text: bounded_arc(new_text),
                },
                ToolOutput::Terminal(id) => ToolOutput::Terminal(bounded(id)),
                ToolOutput::Todo {
                    id,
                    content,
                    status,
                } => ToolOutput::Todo {
                    id: bounded(id),
                    content: bounded(content),
                    status: bounded(status),
                },
                ToolOutput::Task {
                    description,
                    prompt,
                    subagent_type,
                    model,
                    agent_id,
                    agents,
                    path,
                    activity,
                    duration_ms,
                } => ToolOutput::Task {
                    description: bounded(description),
                    prompt: bounded(prompt),
                    subagent_type: bounded(subagent_type),
                    model: model.map(bounded),
                    agent_id: agent_id.map(bounded),
                    agents: agents
                        .into_iter()
                        .take(MAX_CHOICES)
                        .map(|agent| crate::agent::controller::SubagentInfo {
                            id: bounded(agent.id),
                            status: agent.status.map(bounded),
                            message: agent.message.map(bounded),
                        })
                        .collect(),
                    path: path.map(bounded),
                    activity: activity.map(bounded),
                    duration_ms,
                },
                ToolOutput::GeneratedImage {
                    description,
                    file_path,
                    reference_image_paths,
                } => ToolOutput::GeneratedImage {
                    description: bounded(description),
                    file_path,
                    reference_image_paths: reference_image_paths
                        .into_iter()
                        .take(MAX_CHOICES)
                        .collect(),
                },
            })
            .collect(),
        output: detail.output.map(bounded),
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
            text: bounded(text),
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
        let mut end = MAX_ITEM_BYTES - '…'.len_utf8();
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
        text.push('…');
    }
    text
}

fn bounded_arc(text: std::sync::Arc<str>) -> std::sync::Arc<str> {
    if text.len() <= MAX_ITEM_BYTES {
        text
    } else {
        bounded(text.to_string()).into()
    }
}

fn append_bounded(buffer: &mut String, text: &str) {
    let remaining = MAX_ITEM_BYTES.saturating_sub(buffer.len());
    if text.len() <= remaining {
        buffer.push_str(text);
        return;
    }
    let target = MAX_ITEM_BYTES - '…'.len_utf8();
    if buffer.len() > target {
        let mut end = target;
        while !buffer.is_char_boundary(end) {
            end -= 1;
        }
        buffer.truncate(end);
    }
    let remaining = target.saturating_sub(buffer.len());
    let mut end = text.len().min(remaining);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    buffer.push_str(&text[..end]);
    buffer.push('…');
}

const ACCOUNT_HANDOFF_MARKER: &str = "<!-- editur-account-handoff:v1 ";
const MAX_HANDOFF_ORIGINAL_PROMPT_BYTES: usize = 8 * 1024;

fn bounded_handoff_original(text: &str) -> &str {
    let mut end = text.len().min(MAX_HANDOFF_ORIGINAL_PROMPT_BYTES);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

#[derive(Deserialize, Serialize)]
struct AccountHandoffMarker<'a> {
    from: &'a str,
    to: &'a str,
}

#[derive(Deserialize)]
struct OwnedAccountHandoffMarker {
    from: String,
    to: String,
}

fn account_handoff_marker(from: &str, to: &str) -> Result<String, String> {
    let marker = serde_json::to_string(&AccountHandoffMarker { from, to })
        .map_err(|error| format!("cannot encode account handoff marker: {error}"))?;
    Ok(format!("{ACCOUNT_HANDOFF_MARKER}{marker} -->"))
}

fn parse_account_handoff_marker(text: &str) -> Option<(String, String)> {
    let line = text.lines().next()?;
    let json = line
        .strip_prefix(ACCOUNT_HANDOFF_MARKER)?
        .strip_suffix(" -->")?;
    let marker = serde_json::from_str::<OwnedAccountHandoffMarker>(json).ok()?;
    (!marker.from.is_empty() && !marker.to.is_empty()).then_some((marker.from, marker.to))
}

fn transcript_handoff_message(item: &TranscriptItem) -> Option<ExternalMessage> {
    match item {
        TranscriptItem::User(text) => Some(ExternalMessage::User(text.clone())),
        TranscriptItem::Assistant(text) => Some(ExternalMessage::Assistant(text.clone())),
        TranscriptItem::Content {
            role,
            content: DisplayContent::TextResource { uri, text, .. },
        } => {
            let text = format!("Resource {uri}:\n{text}");
            match role {
                ContentRole::User => Some(ExternalMessage::User(text)),
                ContentRole::Assistant => Some(ExternalMessage::Assistant(text)),
                ContentRole::Thought => None,
            }
        }
        TranscriptItem::Tool(tool) => Some(ExternalMessage::Tool(ExternalTool {
            id: tool.id.clone(),
            name: tool.display_title().into_owned(),
            status: tool.status.clone(),
            kind: tool.kind.clone(),
            input: tool.detail.as_ref().and_then(|detail| detail.input.clone()),
            output: tool.detail.as_ref().and_then(handoff_tool_output),
            paths: tool.paths.iter().map(|path| path.path.clone()).collect(),
            diffs: Vec::new(),
        })),
        TranscriptItem::Thought(_)
        | TranscriptItem::AccountSwitch { .. }
        | TranscriptItem::Content { .. }
        | TranscriptItem::Plan(_)
        | TranscriptItem::Permission(_)
        | TranscriptItem::Interaction(_)
        | TranscriptItem::Error(_) => None,
    }
}

fn handoff_tool_output(detail: &ToolDetail) -> Option<String> {
    use std::fmt::Write as _;

    if let Some(output) = &detail.output {
        return Some(output.clone());
    }
    let mut output = String::new();
    for content in &detail.content {
        match content {
            ToolOutput::Text(text) | ToolOutput::Terminal(text) => {
                let _ = writeln!(output, "{text}");
            }
            ToolOutput::Log { label, text } => {
                let _ = writeln!(output, "{label}: {text}");
            }
            ToolOutput::Diff { path, .. } => {
                let _ = writeln!(output, "Changed {}", path.display());
            }
            ToolOutput::Todo {
                content, status, ..
            } => {
                let _ = writeln!(output, "{status}: {content}");
            }
            ToolOutput::Task {
                description,
                activity,
                ..
            } => {
                let _ = writeln!(
                    output,
                    "{}{}",
                    description,
                    activity
                        .as_deref()
                        .map_or(String::new(), |activity| format!(": {activity}"))
                );
            }
            ToolOutput::GeneratedImage {
                description,
                file_path,
                ..
            } => {
                let _ = writeln!(
                    output,
                    "{}{}",
                    description,
                    file_path
                        .as_ref()
                        .map_or(String::new(), |path| format!(" at {}", path.display()))
                );
            }
            ToolOutput::Content(_) => {}
        }
    }
    (!output.is_empty()).then_some(output)
}

fn item_size(item: &TranscriptItem) -> usize {
    match item {
        TranscriptItem::User(text)
        | TranscriptItem::Assistant(text)
        | TranscriptItem::Thought(text)
        | TranscriptItem::Error(text) => text.len(),
        TranscriptItem::AccountSwitch { from, to } => from.len() + to.len(),
        TranscriptItem::Content { content, .. } => display_content_size(content),
        TranscriptItem::Plan(plan) => plan
            .iter()
            .map(|item| item.content.len() + item.status.len())
            .sum(),
        TranscriptItem::Tool(tool) => tool_size(tool),
        TranscriptItem::Permission(card) => {
            card.action.len()
                + card.tool_call_id.len()
                + card
                    .options
                    .iter()
                    .map(|option| option.id.len() + option.name.len() + option.kind.len())
                    .sum::<usize>()
        }
        TranscriptItem::Interaction(card) => interaction_size(&card.request),
    }
}

fn tool_size(tool: &ToolActivity) -> usize {
    tool.id.len()
        + tool.status.as_ref().map_or(0, String::len)
        + tool.kind.as_ref().map_or(0, String::len)
        + tool.detail.as_ref().map_or(0, tool_detail_size)
        + tool.title.as_ref().map_or(0, String::len)
        + tool
            .paths
            .iter()
            .map(|tool_path| tool_path.path.as_os_str().as_encoded_bytes().len())
            .sum::<usize>()
}

fn interaction_size(request: &InteractionRequest) -> usize {
    request.tool_call_id.len()
        + match &request.kind {
            InteractionKind::Questions { title, questions } => {
                title.len()
                    + questions
                        .iter()
                        .map(|question| {
                            question.id.len()
                                + question.prompt.len()
                                + question
                                    .options
                                    .iter()
                                    .map(|option| option.id.len() + option.label.len())
                                    .sum::<usize>()
                        })
                        .sum::<usize>()
            }
            InteractionKind::Plan(plan) => {
                plan.name.as_ref().map_or(0, String::len)
                    + plan.overview.as_ref().map_or(0, String::len)
                    + plan.plan.len()
                    + plan
                        .todos
                        .iter()
                        .map(|todo| todo.content.len() + todo.status.len())
                        .sum::<usize>()
                    + plan
                        .phases
                        .iter()
                        .map(|phase| {
                            phase.name.len()
                                + phase
                                    .todos
                                    .iter()
                                    .map(|todo| todo.content.len() + todo.status.len())
                                    .sum::<usize>()
                        })
                        .sum::<usize>()
            }
            InteractionKind::Url { title, url } => title.len() + url.len(),
        }
}

fn tool_detail_size(detail: &ToolDetail) -> usize {
    detail.input.as_ref().map_or(0, String::len)
        + detail.output.as_ref().map_or(0, String::len)
        + detail
            .content
            .iter()
            .map(|content| match content {
                ToolOutput::Text(text) | ToolOutput::Terminal(text) => text.len(),
                ToolOutput::Log { label, text } => label.len().saturating_add(text.len()),
                ToolOutput::Content(content) => display_content_size(content),
                ToolOutput::Diff {
                    path,
                    old_text,
                    new_text,
                } => {
                    path.as_os_str().as_encoded_bytes().len()
                        + old_text.as_ref().map_or(0, |text| text.len())
                        + new_text.len()
                }
                ToolOutput::Todo {
                    id,
                    content,
                    status,
                } => id.len() + content.len() + status.len(),
                ToolOutput::Task {
                    description,
                    prompt,
                    subagent_type,
                    model,
                    agent_id,
                    agents,
                    path,
                    activity,
                    ..
                } => {
                    description.len()
                        + prompt.len()
                        + subagent_type.len()
                        + model.as_ref().map_or(0, String::len)
                        + agent_id.as_ref().map_or(0, String::len)
                        + path.as_ref().map_or(0, String::len)
                        + activity.as_ref().map_or(0, String::len)
                        + agents
                            .iter()
                            .map(|agent| {
                                agent.id.len()
                                    + agent.status.as_ref().map_or(0, String::len)
                                    + agent.message.as_ref().map_or(0, String::len)
                            })
                            .sum::<usize>()
                }
                ToolOutput::GeneratedImage {
                    description,
                    file_path,
                    reference_image_paths,
                } => {
                    description.len()
                        + file_path
                            .as_ref()
                            .map_or(0, |path| path.as_os_str().as_encoded_bytes().len())
                        + reference_image_paths
                            .iter()
                            .map(|path| path.as_os_str().as_encoded_bytes().len())
                            .sum::<usize>()
                }
            })
            .sum::<usize>()
}

fn display_content_size(content: &DisplayContent) -> usize {
    match content {
        DisplayContent::Image {
            data: Some(data), ..
        } => data.len(),
        DisplayContent::Image { mime_type, uri, .. } => {
            mime_type.len() + uri.as_ref().map_or(0, String::len)
        }
        DisplayContent::Audio { mime_type, .. } => mime_type.len(),
        DisplayContent::BlobResource { mime_type, uri, .. } => {
            mime_type.as_ref().map_or(0, String::len) + uri.len()
        }
        DisplayContent::ResourceLink {
            name,
            title,
            uri,
            description,
            mime_type,
            ..
        } => {
            name.len()
                + title.as_ref().map_or(0, String::len)
                + uri.len()
                + description.as_ref().map_or(0, String::len)
                + mime_type.as_ref().map_or(0, String::len)
        }
        DisplayContent::TextResource {
            uri,
            mime_type,
            text,
        } => uri.len() + mime_type.as_ref().map_or(0, String::len) + text.len(),
    }
}
