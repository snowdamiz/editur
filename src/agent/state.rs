use std::{
    collections::{HashSet, VecDeque},
    path::PathBuf,
};

use super::controller::{
    ConfigChoice, ConnectionState, Event, ModeChoice, PermissionChoice, PlanItem, ToolActivity,
};

const MAX_TRANSCRIPT_BYTES: usize = 1024 * 1024;
const MAX_ITEM_BYTES: usize = 64 * 1024;
const MAX_TRANSCRIPT_ITEMS: usize = 2_048;
const MAX_CHANGED_PATHS: usize = 4_096;
const MAX_CHOICES: usize = 128;

#[derive(Clone, Debug, PartialEq)]
pub enum TranscriptItem {
    User(String),
    Assistant(String),
    Plan(Vec<PlanItem>),
    Tool(ToolActivity),
    Permission(PermissionCard),
    Error(String),
    Truncated,
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
pub struct UsageState {
    pub used: u64,
    pub size: u64,
    pub cost: Option<String>,
}

pub struct AgentState {
    pub connection: ConnectionState,
    pub session_ready: bool,
    pub active: bool,
    pub prompt: String,
    pub transcript: VecDeque<TranscriptItem>,
    pub changed_paths: HashSet<PathBuf>,
    pub current_mode: Option<String>,
    pub modes: Vec<ModeChoice>,
    pub config_options: Vec<ConfigChoice>,
    pub usage: Option<UsageState>,
}

impl Default for AgentState {
    fn default() -> Self {
        Self {
            connection: ConnectionState::Disconnected,
            session_ready: false,
            active: false,
            prompt: String::new(),
            transcript: VecDeque::new(),
            changed_paths: HashSet::new(),
            current_mode: None,
            modes: Vec::new(),
            config_options: Vec::new(),
            usage: None,
        }
    }
}

impl AgentState {
    pub fn can_send(&self, buffer_saved: bool) -> bool {
        buffer_saved && self.session_ready && !self.active && !self.prompt.trim().is_empty()
    }

    pub fn waiting_permission(&self) -> bool {
        self.transcript
            .iter()
            .any(|item| matches!(item, TranscriptItem::Permission(card) if card.selected.is_none()))
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

    pub fn apply(&mut self, event: Event) {
        match event {
            Event::ConnectionChanged(connection) => {
                self.session_ready = matches!(connection, ConnectionState::Ready);
                self.connection = connection;
            }
            Event::SessionReady {
                current_mode,
                modes,
                config_options,
            } => {
                self.session_ready = true;
                self.current_mode = current_mode.map(bounded);
                self.modes = modes
                    .into_iter()
                    .take(MAX_CHOICES)
                    .map(bounded_mode)
                    .collect();
                self.config_options = bounded_configs(config_options);
                self.usage = None;
            }
            Event::ModeChanged(mode) => self.current_mode = Some(bounded(mode)),
            Event::ConfigOptionsUpdated(options) => self.config_options = bounded_configs(options),
            Event::UserMessage(text) => {
                self.active = true;
                self.prompt.clear();
                self.push(TranscriptItem::User(bounded(text)));
            }
            Event::AssistantDelta(text) => {
                let text = bounded(text);
                if let Some(TranscriptItem::Assistant(current)) = self.transcript.back_mut()
                    && current.len() < MAX_ITEM_BYTES
                {
                    let remaining = MAX_ITEM_BYTES - current.len();
                    let mut end = text.len().min(remaining);
                    while !text.is_char_boundary(end) {
                        end -= 1;
                    }
                    current.push_str(&text[..end]);
                } else {
                    self.push(TranscriptItem::Assistant(text));
                }
            }
            Event::PlanUpdated(plan) => {
                let plan = plan
                    .into_iter()
                    .take(MAX_TRANSCRIPT_ITEMS)
                    .map(|item| PlanItem {
                        content: bounded(item.content),
                        status: bounded(item.status),
                    })
                    .collect();
                if let Some(TranscriptItem::Plan(current)) = self
                    .transcript
                    .iter_mut()
                    .rev()
                    .find(|item| matches!(item, TranscriptItem::Plan(_)))
                {
                    *current = plan;
                } else {
                    self.push(TranscriptItem::Plan(plan));
                }
            }
            Event::ToolCallUpdated(tool) => {
                let tool = bounded_tool(tool);
                let remaining = MAX_CHANGED_PATHS.saturating_sub(self.changed_paths.len());
                self.changed_paths
                    .extend(tool.paths.iter().take(remaining).cloned());
                if let Some(TranscriptItem::Tool(current)) = self.transcript.iter_mut().rev().find(
                    |item| matches!(item, TranscriptItem::Tool(current) if current.id == tool.id),
                ) {
                    if let Some(title) = tool.title {
                        current.title = Some(bounded(title));
                    }
                    if tool.status.is_some() {
                        current.status = tool.status;
                    }
                    if !tool.paths.is_empty() {
                        current.paths = tool.paths;
                    }
                    if let Some(detail) = tool.detail {
                        current.detail = Some(bounded(detail));
                    }
                } else {
                    self.push(TranscriptItem::Tool(tool));
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
            Event::UsageUpdated { used, size, cost } => {
                self.usage = Some(UsageState {
                    used,
                    size,
                    cost: cost.map(bounded),
                });
            }
            Event::TurnFinished { .. } => self.active = false,
            Event::Error(error) => self.push(TranscriptItem::Error(bounded(error))),
            Event::ProcessExited { error, diagnostics } => {
                self.active = false;
                self.session_ready = false;
                self.connection = ConnectionState::Failed(error.clone());
                self.push(TranscriptItem::Error(bounded(if diagnostics.is_empty() {
                    error
                } else {
                    format!("{error}\n{diagnostics}")
                })));
            }
        }
        self.trim();
    }

    fn push(&mut self, item: TranscriptItem) {
        self.transcript.push_back(item);
        self.trim();
    }

    fn trim(&mut self) {
        let mut removed = matches!(self.transcript.front(), Some(TranscriptItem::Truncated));
        if removed {
            self.transcript.pop_front();
        }
        while self.transcript.len() > MAX_TRANSCRIPT_ITEMS
            || self.transcript.iter().map(item_size).sum::<usize>() > MAX_TRANSCRIPT_BYTES
        {
            if let Some(tool) = self.transcript.iter_mut().find_map(|item| match item {
                TranscriptItem::Tool(tool) if tool.detail.is_some() => Some(tool),
                _ => None,
            }) {
                tool.detail = None;
                continue;
            }
            removed |= self.transcript.pop_front().is_some();
        }
        if removed {
            self.transcript.push_front(TranscriptItem::Truncated);
        }
    }
}

fn bounded_tool(mut tool: ToolActivity) -> ToolActivity {
    tool.id = bounded(tool.id);
    tool.title = tool.title.map(bounded);
    tool.status = tool.status.map(bounded);
    tool.detail = tool.detail.map(bounded);
    tool.paths
        .retain(|path| path.as_os_str().as_encoded_bytes().len() <= MAX_ITEM_BYTES);
    tool.paths.truncate(MAX_CHOICES);
    tool
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

fn item_size(item: &TranscriptItem) -> usize {
    match item {
        TranscriptItem::User(text)
        | TranscriptItem::Assistant(text)
        | TranscriptItem::Error(text) => text.len(),
        TranscriptItem::Plan(plan) => plan
            .iter()
            .map(|item| item.content.len() + item.status.len())
            .sum(),
        TranscriptItem::Tool(tool) => {
            tool.id.len()
                + tool.status.as_ref().map_or(0, String::len)
                + tool.detail.as_ref().map_or(0, String::len)
                + tool.title.as_ref().map_or(0, String::len)
                + tool
                    .paths
                    .iter()
                    .map(|path| path.as_os_str().as_encoded_bytes().len())
                    .sum::<usize>()
        }
        TranscriptItem::Permission(card) => {
            card.action.len()
                + card.tool_call_id.len()
                + card
                    .options
                    .iter()
                    .map(|option| option.id.len() + option.name.len() + option.kind.len())
                    .sum::<usize>()
        }
        TranscriptItem::Truncated => 0,
    }
}
