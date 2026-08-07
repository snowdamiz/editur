use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::PathBuf,
};

use super::controller::{
    CommandChoice, ConfigChoice, ConnectionState, ContentRole, DisplayContent, Event,
    InteractionKind, InteractionRequest, ModeChoice, PermissionChoice, PlanItem, PlanPhase,
    PlanProposal, Question, QuestionOption, SessionChoice, ToolActivity, ToolDetail, ToolOutput,
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
    pub commands: Vec<CommandChoice>,
    pub sessions: Option<Vec<SessionChoice>>,
    pub title: Option<String>,
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
            commands: Vec::new(),
            sessions: None,
            title: None,
            usage: None,
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
                self.active = false;
                self.transcript.clear();
                self.changed_paths.clear();
                self.title = None;
                self.current_mode = current_mode.map(bounded);
                self.modes = modes
                    .into_iter()
                    .take(MAX_CHOICES)
                    .map(bounded_mode)
                    .collect();
                self.config_options = bounded_configs(config_options);
                self.usage = None;
            }
            Event::SessionsUpdated(sessions) => {
                self.sessions = Some(sessions.into_iter().take(MAX_CHOICES).collect());
            }
            Event::SessionLoading { title } => {
                self.session_ready = false;
                self.active = false;
                self.connection = ConnectionState::Starting;
                self.transcript.clear();
                self.changed_paths.clear();
                self.title = title.map(bounded);
                self.usage = None;
            }
            Event::SessionLoaded {
                current_mode,
                modes,
                config_options,
            } => {
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
            Event::SessionTitleUpdated(title) => self.title = title.map(bounded),
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
            Event::ThoughtDelta(text) => {
                let text = bounded(text);
                if let Some(TranscriptItem::Thought(current)) = self.transcript.back_mut()
                    && current.len() < MAX_ITEM_BYTES
                {
                    let remaining = MAX_ITEM_BYTES - current.len();
                    let mut end = text.len().min(remaining);
                    while !text.is_char_boundary(end) {
                        end -= 1;
                    }
                    current.push_str(&text[..end]);
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
                    .take(MAX_TRANSCRIPT_ITEMS)
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
                        let detail = bounded_tool_detail(detail);
                        if let Some(current) = &mut current.detail {
                            if detail.input.is_some() {
                                current.input = detail.input;
                            }
                            if !detail.content.is_empty() {
                                current.content = detail.content;
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
    tool.detail = tool.detail.map(bounded_tool_detail);
    tool.paths
        .retain(|path| path.as_os_str().as_encoded_bytes().len() <= MAX_ITEM_BYTES);
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
        input: detail.input.map(bounded),
        content: detail
            .content
            .into_iter()
            .take(MAX_CHOICES)
            .map(|content| match content {
                ToolOutput::Text(text) => ToolOutput::Text(bounded(text)),
                ToolOutput::Content(content) => ToolOutput::Content(bounded_content(content)),
                ToolOutput::Diff {
                    path,
                    old_text,
                    new_text,
                } => ToolOutput::Diff {
                    path,
                    old_text: old_text.map(bounded),
                    new_text: bounded(new_text),
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
                    duration_ms,
                } => ToolOutput::Task {
                    description: bounded(description),
                    prompt: bounded(prompt),
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
        } => DisplayContent::Image {
            mime_type: bounded(mime_type),
            uri: uri.map(bounded),
            encoded_bytes,
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
        | TranscriptItem::Thought(text)
        | TranscriptItem::Error(text) => text.len(),
        TranscriptItem::Content { content, .. } => display_content_size(content),
        TranscriptItem::Plan(plan) => plan
            .iter()
            .map(|item| item.content.len() + item.status.len())
            .sum(),
        TranscriptItem::Tool(tool) => {
            tool.id.len()
                + tool.status.as_ref().map_or(0, String::len)
                + tool.detail.as_ref().map_or(0, tool_detail_size)
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
        TranscriptItem::Interaction(card) => interaction_size(&card.request),
        TranscriptItem::Truncated => 0,
    }
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
                ToolOutput::Content(content) => display_content_size(content),
                ToolOutput::Diff {
                    path,
                    old_text,
                    new_text,
                } => {
                    path.as_os_str().as_encoded_bytes().len()
                        + old_text.as_ref().map_or(0, String::len)
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
                    ..
                } => {
                    description.len()
                        + prompt.len()
                        + subagent_type.len()
                        + model.as_ref().map_or(0, String::len)
                        + agent_id.as_ref().map_or(0, String::len)
                }
                ToolOutput::GeneratedImage {
                    description,
                    file_path,
                    reference_image_paths,
                } => {
                    description.len()
                        + file_path.as_os_str().as_encoded_bytes().len()
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
