use std::{
    collections::{HashMap, VecDeque},
    fs::File,
    io::{BufRead, BufReader, Read as _},
    path::{Path, PathBuf},
    sync::Arc,
    time::UNIX_EPOCH,
};

use base64::Engine as _;
use rusqlite::{Connection, OpenFlags, params};

use super::provider::ProviderId;

const MAX_TRANSCRIPT_EVENTS: usize = 4_096;
const MAX_EXTERNAL_RECORD_BYTES: usize = 24 * 1024 * 1024;
const MAX_EXTERNAL_TRANSCRIPT_BYTES: usize = 256 * 1024 * 1024;
const MAX_HANDOFF_BYTES: usize = 64 * 1024;
const MAX_HANDOFF_MESSAGE_BYTES: usize = 16 * 1024;
const MAX_CURSOR_BUBBLE_BYTES: usize = 1024 * 1024;
const MAX_EXTERNAL_DIFF_BYTES: usize = 16 * 1024 * 1024;
const MAX_EXTERNAL_IMAGE_BYTES: usize = 16 * 1024 * 1024;
const MAX_EXTERNAL_IMAGE_TOTAL_BYTES: usize = 64 * 1024 * 1024;
const MAX_CURSOR_IMAGE_FILES: usize = 65_536;
const EXTERNAL_ID_PREFIX: &str = "external:";

pub(crate) struct BoundedHandoff {
    prefix: String,
    suffix: String,
    available: usize,
    history: VecDeque<String>,
    history_bytes: usize,
}

impl BoundedHandoff {
    pub(crate) fn new(prefix: String, suffix: String) -> Result<Self, String> {
        let available = MAX_HANDOFF_BYTES
            .checked_sub(prefix.len().saturating_add(suffix.len()))
            .ok_or_else(|| "handoff instructions exceed the prompt size limit".to_owned())?;
        Ok(Self {
            prefix,
            suffix,
            available,
            history: VecDeque::new(),
            history_bytes: 0,
        })
    }

    pub(crate) fn push(&mut self, message: ExternalMessage) {
        let Some(mut message) = handoff_message(message) else {
            return;
        };
        if message.len() > MAX_HANDOFF_MESSAGE_BYTES {
            let mut end = MAX_HANDOFF_MESSAGE_BYTES - '…'.len_utf8();
            while !message.is_char_boundary(end) {
                end -= 1;
            }
            message.truncate(end);
            message.push('…');
        }
        self.history_bytes = self
            .history_bytes
            .saturating_add(message.len() + usize::from(!self.history.is_empty()) * 2);
        self.history.push_back(message);
        while self.history_bytes > self.available && self.history.len() > 1 {
            self.history_bytes = self
                .history_bytes
                .saturating_sub(self.history.pop_front().unwrap().len() + 2);
        }
        if self.history_bytes > self.available {
            let message = self.history.front_mut().unwrap();
            let mut start = message.len().saturating_sub(self.available);
            while !message.is_char_boundary(start) {
                start += 1;
            }
            *message = message[start..].to_owned();
            self.history_bytes = message.len();
        }
    }

    pub(crate) fn finish(mut self) -> String {
        format!(
            "{}{}{}",
            self.prefix,
            self.history.make_contiguous().join("\n\n"),
            self.suffix
        )
    }
}

struct BoundedLines<R> {
    reader: R,
    scanned: usize,
    max_record: usize,
    max_total: usize,
    finished: bool,
}

impl<R: BufRead> BoundedLines<R> {
    fn new(reader: R) -> Self {
        Self {
            reader,
            scanned: 0,
            max_record: MAX_EXTERNAL_RECORD_BYTES,
            max_total: MAX_EXTERNAL_TRANSCRIPT_BYTES,
            finished: false,
        }
    }

    #[cfg(test)]
    fn with_limits(reader: R, max_record: usize, max_total: usize) -> Self {
        Self {
            reader,
            scanned: 0,
            max_record,
            max_total,
            finished: false,
        }
    }
}

impl<R: BufRead> Iterator for BoundedLines<R> {
    type Item = Result<String, String>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.finished {
            return None;
        }
        let mut line = Vec::new();
        loop {
            let (chunk_len, consumed, newline) = match self.reader.fill_buf() {
                Ok([]) => {
                    self.finished = true;
                    if line.is_empty() {
                        return None;
                    }
                    (0, 0, true)
                }
                Ok(buffer) => match buffer.iter().position(|byte| *byte == b'\n') {
                    Some(index) => (index, index + 1, true),
                    None => (buffer.len(), buffer.len(), false),
                },
                Err(error) => {
                    self.finished = true;
                    return Some(Err(format!("cannot read external transcript: {error}")));
                }
            };
            if self.scanned.saturating_add(consumed) > self.max_total {
                self.finished = true;
                return Some(Err(format!(
                    "external transcript exceeds the {} byte scan limit",
                    self.max_total
                )));
            }
            if line.len().saturating_add(chunk_len) > self.max_record {
                self.finished = true;
                return Some(Err(format!(
                    "external transcript record exceeds the {} byte limit",
                    self.max_record
                )));
            }
            if chunk_len > 0 {
                let buffer = self.reader.fill_buf().expect("buffer was read above");
                line.extend_from_slice(&buffer[..chunk_len]);
            }
            self.reader.consume(consumed);
            self.scanned += consumed;
            if newline {
                if line.last() == Some(&b'\r') {
                    line.pop();
                }
                return Some(
                    String::from_utf8(line)
                        .map_err(|_| "external transcript contains a non-UTF-8 record".to_owned()),
                );
            }
        }
    }
}

fn bounded_lines(file: File) -> BoundedLines<BufReader<File>> {
    BoundedLines::new(BufReader::new(file))
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ExternalMessage {
    User(String),
    Assistant(String),
    Thought(String),
    Image(ExternalImage),
    Tool(ExternalTool),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ExternalImage {
    pub mime_type: String,
    pub bytes: Arc<[u8]>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ExternalTool {
    pub id: String,
    pub name: String,
    pub status: Option<String>,
    pub kind: Option<String>,
    pub input: Option<String>,
    pub output: Option<String>,
    pub paths: Vec<PathBuf>,
    pub diffs: Vec<ExternalDiff>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ExternalDiff {
    pub path: PathBuf,
    pub old_text: Option<Arc<str>>,
    pub new_text: Arc<str>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TranscriptFormat {
    Cursor,
    Codex,
    Claude,
}

#[derive(Clone, Debug)]
pub(crate) struct ExternalSession {
    pub id: String,
    pub title: Option<String>,
    pub updated_at: Option<String>,
    pub transcript_path: PathBuf,
    cursor_database: Option<PathBuf>,
    format: TranscriptFormat,
}

impl ExternalSession {
    #[cfg(test)]
    pub(crate) fn test_fixture(id: &str, transcript_path: PathBuf) -> Self {
        Self {
            id: id.into(),
            title: Some("Imported session".into()),
            updated_at: None,
            transcript_path,
            cursor_database: None,
            format: TranscriptFormat::Codex,
        }
    }

    #[cfg(test)]
    pub(crate) fn transcript(&self) -> Result<Vec<ExternalMessage>, String> {
        let mut messages = Vec::new();
        self.visit_transcript(&mut |message| {
            messages.push(message);
            Ok(())
        })?;
        Ok(messages)
    }

    pub(crate) fn visit_transcript(
        &self,
        visit: &mut impl FnMut(ExternalMessage) -> Result<(), String>,
    ) -> Result<usize, String> {
        match self.format {
            TranscriptFormat::Cursor => {
                if let Some(database) = &self.cursor_database {
                    let mut emitted = 0;
                    let result =
                        visit_cursor_database_transcript(database, &self.id, &mut |message| {
                            visit(message)?;
                            emitted += 1;
                            Ok(())
                        });
                    match result {
                        Ok(count) if count > 0 => return Ok(count),
                        Err(error) if emitted > 0 => return Err(error),
                        _ => {}
                    }
                }
                visit_cursor_transcript(&self.transcript_path, visit)
            }
            TranscriptFormat::Codex => {
                visit_messages(codex_transcript(&self.transcript_path)?, visit)
            }
            TranscriptFormat::Claude => {
                visit_messages(claude_transcript(&self.transcript_path)?, visit)
            }
        }
    }

    pub(crate) fn choice_id(&self) -> String {
        format!("{EXTERNAL_ID_PREFIX}{}", self.id)
    }

    pub(crate) fn handoff_prompt(&self, next_message: &str) -> Result<String, String> {
        let prefix = "Continue the imported conversation below. Treat it as prior context, do not repeat or summarize it unless asked, and respond only to the new user message.\n\n<imported-conversation>\n".to_owned();
        let suffix = format!(
            "\n</imported-conversation>\n\n<new-user-message>\n{next_message}\n</new-user-message>"
        );
        let mut handoff = BoundedHandoff::new(prefix, suffix)?;
        self.visit_transcript(&mut |message| {
            handoff.push(message);
            Ok(())
        })?;
        Ok(handoff.finish())
    }
}

fn handoff_message(message: ExternalMessage) -> Option<String> {
    match message {
        ExternalMessage::User(text) => Some(format!("User:\n{text}")),
        ExternalMessage::Assistant(text) => Some(format!("Assistant:\n{text}")),
        ExternalMessage::Thought(_) | ExternalMessage::Image(_) => None,
        ExternalMessage::Tool(tool) => {
            let mut text = format!("Tool {}", tool.name);
            if let Some(status) = tool.status {
                text.push_str(&format!(" ({status})"));
            }
            if !tool.paths.is_empty() {
                text.push_str(" paths:");
                for path in tool.paths.into_iter().take(16) {
                    text.push_str(&format!("\n{}", path.display()));
                }
            }
            if let Some(input) = tool.input {
                text.push_str(&format!(" input:\n{input}"));
            }
            if let Some(output) = tool.output {
                text.push_str(&format!("\nTool output:\n{output}"));
            }
            Some(text)
        }
    }
}

fn visit_messages(
    messages: Vec<ExternalMessage>,
    visit: &mut impl FnMut(ExternalMessage) -> Result<(), String>,
) -> Result<usize, String> {
    let count = messages.len();
    for message in messages {
        visit(message)?;
    }
    Ok(count)
}

pub(crate) fn is_external_choice(id: &str) -> bool {
    id.starts_with(EXTERNAL_ID_PREFIX)
}

pub(crate) fn discover(
    provider: ProviderId,
    project_root: &Path,
    provider_data: Option<&Path>,
) -> Result<Vec<ExternalSession>, String> {
    if provider == ProviderId::Cursor && provider_data.is_some() {
        return Ok(Vec::new());
    }
    let base = directories::BaseDirs::new()
        .ok_or_else(|| "cannot locate provider session storage".to_owned())?;
    let home = base.home_dir();
    let session_root = provider_session_root(provider, provider_data, home);
    match provider {
        ProviderId::Cursor => {
            let database = cursor_databases(home, base.config_dir())
                .into_iter()
                .find(|path| path.is_file());
            let Some(database) = database else {
                return Ok(Vec::new());
            };
            discover_cursor(project_root, &database, &home.join(".cursor/projects"))
        }
        ProviderId::Codex => {
            let database = [
                session_root.join("state_5.sqlite"),
                session_root.join("sqlite/state_5.sqlite"),
            ]
            .into_iter()
            .find(|path| path.is_file());
            let Some(database) = database else {
                return Ok(Vec::new());
            };
            discover_codex(project_root, &database, &session_root)
        }
        ProviderId::Claude => discover_claude(project_root, &session_root.join("projects")),
    }
}

fn provider_session_root(
    provider: ProviderId,
    provider_data: Option<&Path>,
    home: &Path,
) -> PathBuf {
    match provider {
        ProviderId::Cursor => home.to_path_buf(),
        ProviderId::Codex => provider_data.map(Path::to_path_buf).unwrap_or_else(|| {
            std::env::var_os("CODEX_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| home.join(".codex"))
        }),
        ProviderId::Claude => provider_data.map(Path::to_path_buf).unwrap_or_else(|| {
            std::env::var_os("CLAUDE_CONFIG_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|| home.join(".claude"))
        }),
    }
}

fn cursor_databases(home: &Path, config: &Path) -> Vec<PathBuf> {
    vec![
        home.join("Library/Application Support/Cursor/User/globalStorage/conversation-search.db"),
        config.join("Cursor/User/globalStorage/conversation-search.db"),
    ]
}

fn cursor_project_directory(projects: &Path, project_root: &Path) -> PathBuf {
    let encoded = project_root
        .to_string_lossy()
        .trim_start_matches(['/', '\\'])
        .chars()
        .map(|character| match character {
            '/' | '\\' | ':' => '-',
            character => character,
        })
        .collect::<String>();
    projects.join(encoded)
}

fn claude_project_directory(projects: &Path, project_root: &Path) -> PathBuf {
    let encoded = project_root
        .to_string_lossy()
        .chars()
        .map(|character| match character {
            '/' | '\\' | ':' => '-',
            character => character,
        })
        .collect::<String>();
    projects.join(encoded)
}

fn discover_cursor(
    project_root: &Path,
    database: &Path,
    projects: &Path,
) -> Result<Vec<ExternalSession>, String> {
    let connection = Connection::open_with_flags(
        database,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|error| format!("cannot read Cursor history: {error}"))?;
    let mut query = connection
        .prepare(
            "SELECT id, title, updated_at
             FROM conversations
             WHERE source = ?1 AND is_archived = ?2
             ORDER BY updated_at DESC
             LIMIT ?3",
        )
        .map_err(|error| format!("cannot inspect Cursor history: {error}"))?;
    let transcript_root =
        cursor_project_directory(projects, project_root).join("agent-transcripts");
    let cursor_database = database
        .with_file_name("state.vscdb")
        .is_file()
        .then(|| database.with_file_name("state.vscdb"));
    let rows = query
        .query_map(params!["local", 0_i64, 128_i64], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })
        .map_err(|error| format!("cannot list Cursor history: {error}"))?;
    let mut sessions = Vec::new();
    for row in rows {
        let (id, title, updated_at) =
            row.map_err(|error| format!("cannot decode Cursor history: {error}"))?;
        if !valid_session_id(&id) {
            continue;
        }
        let transcript_path = transcript_root.join(&id).join(format!("{id}.jsonl"));
        if transcript_path.is_file() {
            sessions.push(ExternalSession {
                id,
                title: (!title.trim().is_empty()).then_some(title),
                updated_at: Some(format!("{updated_at:020}")),
                transcript_path,
                cursor_database: cursor_database.clone(),
                format: TranscriptFormat::Cursor,
            });
        }
    }
    Ok(sessions)
}

fn discover_codex(
    project_root: &Path,
    database: &Path,
    codex_home: &Path,
) -> Result<Vec<ExternalSession>, String> {
    let connection = Connection::open_with_flags(
        database,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|error| format!("cannot read Codex history: {error}"))?;
    let mut query = connection
        .prepare(
            "SELECT id, title, rollout_path, updated_at
             FROM threads
             WHERE cwd = ?1 AND archived = ?2
             ORDER BY updated_at DESC
             LIMIT ?3",
        )
        .map_err(|error| format!("cannot inspect Codex history: {error}"))?;
    let rows = query
        .query_map(
            params![project_root.to_string_lossy().as_ref(), 0_i64, 128_i64],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            },
        )
        .map_err(|error| format!("cannot list Codex history: {error}"))?;
    let canonical_home = codex_home
        .canonicalize()
        .unwrap_or_else(|_| codex_home.to_owned());
    let mut sessions = Vec::new();
    for row in rows {
        let (id, title, rollout_path, updated_at) =
            row.map_err(|error| format!("cannot decode Codex history: {error}"))?;
        if !valid_session_id(&id) {
            continue;
        }
        let transcript_path = PathBuf::from(rollout_path);
        let canonical_transcript = match transcript_path.canonicalize() {
            Ok(path) => path,
            Err(_) => continue,
        };
        if !canonical_transcript.starts_with(&canonical_home) || !canonical_transcript.is_file() {
            continue;
        }
        sessions.push(ExternalSession {
            id,
            title: (!title.trim().is_empty()).then_some(title),
            updated_at: Some(format!("{updated_at:020}")),
            transcript_path: canonical_transcript,
            cursor_database: None,
            format: TranscriptFormat::Codex,
        });
    }
    Ok(sessions)
}

fn discover_claude(project_root: &Path, projects: &Path) -> Result<Vec<ExternalSession>, String> {
    let directory = claude_project_directory(projects, project_root);
    let entries = match directory.read_dir() {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("cannot read Claude history: {error}")),
    };
    let mut sessions = Vec::new();
    for entry in entries.flatten().take(512) {
        let path = entry.path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("jsonl") {
            continue;
        }
        let Some(id) = path.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        if !valid_session_id(id) {
            continue;
        }
        let Some(title) = claude_summary(&path, project_root, id)? else {
            continue;
        };
        let updated_at = entry
            .metadata()
            .ok()
            .and_then(|metadata| metadata.modified().ok())
            .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
            .map(|duration| format!("{:020}", duration.as_millis()));
        sessions.push(ExternalSession {
            id: id.to_owned(),
            title,
            updated_at,
            transcript_path: path,
            cursor_database: None,
            format: TranscriptFormat::Claude,
        });
    }
    sessions.sort_by(|left, right| right.updated_at.cmp(&left.updated_at));
    sessions.truncate(128);
    Ok(sessions)
}

#[cfg(test)]
fn cursor_database_transcript(
    database: &Path,
    session_id: &str,
) -> Result<Vec<ExternalMessage>, String> {
    let mut messages = Vec::new();
    visit_cursor_database_transcript(database, session_id, &mut |message| {
        messages.push(message);
        Ok(())
    })?;
    Ok(messages)
}

fn visit_cursor_database_transcript(
    database: &Path,
    session_id: &str,
    visit: &mut impl FnMut(ExternalMessage) -> Result<(), String>,
) -> Result<usize, String> {
    if !valid_session_id(session_id) {
        return Err("invalid Cursor session identifier".into());
    }
    let connection = Connection::open_with_flags(
        database,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|error| format!("cannot read Cursor transcript: {error}"))?;
    let mut query = connection
        .prepare(
            "SELECT CAST(value AS TEXT)
             FROM cursorDiskKV
             WHERE key >= ?1 AND key < ?2 AND length(value) <= ?3
             ORDER BY json_extract(value, '$.createdAt'), key",
        )
        .map_err(|error| format!("cannot inspect Cursor transcript: {error}"))?;
    let start = format!("bubbleId:{session_id}:");
    let end = format!("bubbleId:{session_id};");
    let rows = query
        .query_map(params![start, end, MAX_CURSOR_BUBBLE_BYTES as i64], |row| {
            row.get::<_, String>(0)
        })
        .map_err(|error| format!("cannot list Cursor transcript: {error}"))?;
    let mut last = None;
    let mut count = 0;
    let mut content = CursorContentCache::new(&connection);
    let mut images = CursorImageCache::new(database);
    for row in rows {
        let text = row.map_err(|error| format!("cannot decode Cursor transcript: {error}"))?;
        let Ok(record) = serde_json::from_str::<serde_json::Value>(&text) else {
            continue;
        };
        match record.get("type").and_then(serde_json::Value::as_i64) {
            Some(1) => {
                if let Some(tool) = cursor_simulated_task(&record) {
                    visit_distinct(&mut last, ExternalMessage::Tool(tool), &mut count, visit)?;
                } else if let Some(text) = record.get("text").and_then(message_text) {
                    visit_distinct(&mut last, ExternalMessage::User(text), &mut count, visit)?;
                }
                for image in record
                    .get("images")
                    .and_then(serde_json::Value::as_array)
                    .into_iter()
                    .flatten()
                    .take(32)
                    .filter_map(|image| image.get("uuid").and_then(serde_json::Value::as_str))
                    .filter_map(|id| images.get(id))
                {
                    visit_distinct(&mut last, ExternalMessage::Image(image), &mut count, visit)?;
                }
            }
            Some(2) => {
                if let Some(thought) = record
                    .pointer("/thinking/text")
                    .or_else(|| record.get("thinking"))
                    .and_then(message_text)
                {
                    visit_distinct(
                        &mut last,
                        ExternalMessage::Thought(thought),
                        &mut count,
                        visit,
                    )?;
                }
                if let Some(text) = record.get("text").and_then(message_text) {
                    visit_distinct(
                        &mut last,
                        ExternalMessage::Assistant(text),
                        &mut count,
                        visit,
                    )?;
                }
                if let Some(tool) = cursor_tool(&record, &mut content) {
                    visit_distinct(&mut last, ExternalMessage::Tool(tool), &mut count, visit)?;
                }
            }
            _ => {}
        }
    }
    Ok(count)
}

fn cursor_simulated_task(record: &serde_json::Value) -> Option<ExternalTool> {
    if record
        .get("isSimulatedMsg")
        .and_then(serde_json::Value::as_bool)
        != Some(true)
        || record
            .get("simulatedMsgReason")
            .and_then(serde_json::Value::as_i64)
            != Some(3)
    {
        return None;
    }
    let metadata = record.get("simulatedMessageMetadata")?;
    let title = metadata.get("title")?.as_str()?.trim();
    let id = metadata
        .get("taskId")
        .and_then(serde_json::Value::as_str)
        .or_else(|| record.get("bubbleId").and_then(serde_json::Value::as_str))?;
    let text = record
        .get("text")
        .and_then(message_text)
        .unwrap_or_default();
    let status = text
        .lines()
        .find_map(|line| line.strip_prefix("status: "))
        .map(str::to_owned);
    let output = text
        .split_once("<response>")
        .and_then(|(_, response)| response.split_once("</response>"))
        .map(|(response, _)| response.trim())
        .filter(|response| !response.is_empty())
        .or_else(|| {
            text.split_once("\ndetail: ")
                .and_then(|(_, detail)| detail.split_once("\n</task>"))
                .map(|(detail, _)| detail.trim())
                .filter(|detail| !detail.is_empty())
        })
        .unwrap_or(text.trim());
    Some(ExternalTool {
        id: id.to_owned(),
        name: format!("Background task · {title}"),
        status,
        kind: None,
        input: None,
        output: (!output.is_empty()).then(|| bounded_external_text(output)),
        paths: Vec::new(),
        diffs: Vec::new(),
    })
}

struct CursorImageCache {
    root: Option<PathBuf>,
    paths: HashMap<String, PathBuf>,
    indexed: bool,
}

impl CursorImageCache {
    fn new(database: &Path) -> Self {
        Self {
            root: database
                .parent()
                .and_then(Path::parent)
                .map(|user| user.join("workspaceStorage")),
            paths: HashMap::new(),
            indexed: false,
        }
    }

    fn get(&mut self, id: &str) -> Option<ExternalImage> {
        if !self.indexed {
            self.index();
        }
        let path = self.paths.get(id)?.clone();
        let mut bytes = Vec::new();
        File::open(&path)
            .ok()?
            .take((MAX_EXTERNAL_IMAGE_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .ok()?;
        if bytes.is_empty() || bytes.len() > MAX_EXTERNAL_IMAGE_BYTES {
            return None;
        }
        let mime_type = image_mime_bytes(&bytes).or_else(|| image_mime(&path))?;
        Some(ExternalImage {
            mime_type: mime_type.to_owned(),
            bytes: bytes.into(),
        })
    }

    fn index(&mut self) {
        self.indexed = true;
        let Some(root) = &self.root else { return };
        let Ok(workspaces) = root.read_dir() else {
            return;
        };
        let mut count = 0;
        for workspace in workspaces.flatten() {
            let Ok(entries) = workspace.path().join("images").read_dir() else {
                continue;
            };
            for entry in entries.flatten() {
                if count >= MAX_CURSOR_IMAGE_FILES {
                    return;
                }
                count += 1;
                if !entry.file_type().is_ok_and(|kind| kind.is_file()) {
                    continue;
                }
                let name = entry.file_name();
                let Some(name) = name.to_str() else { continue };
                let Some(id) = name.get(..36).filter(|id| valid_cursor_image_id(id)) else {
                    continue;
                };
                if image_mime(&entry.path()).is_none() {
                    continue;
                }
                self.paths
                    .entry(id.to_owned())
                    .or_insert_with(|| entry.path());
            }
        }
    }
}

fn valid_cursor_image_id(id: &str) -> bool {
    id.len() == 36
        && id.bytes().enumerate().all(|(index, byte)| {
            matches!(index, 8 | 13 | 18 | 23) && byte == b'-'
                || !matches!(index, 8 | 13 | 18 | 23) && byte.is_ascii_hexdigit()
        })
}

fn image_mime(path: &Path) -> Option<&'static str> {
    match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        _ => None,
    }
}

fn image_mime_bytes(bytes: &[u8]) -> Option<&'static str> {
    match image::guess_format(bytes).ok()? {
        image::ImageFormat::Png => Some("image/png"),
        image::ImageFormat::Jpeg => Some("image/jpeg"),
        image::ImageFormat::Gif => Some("image/gif"),
        image::ImageFormat::WebP => Some("image/webp"),
        _ => None,
    }
}

fn cursor_tool(
    record: &serde_json::Value,
    content: &mut CursorContentCache<'_>,
) -> Option<ExternalTool> {
    let tool = record.get("toolFormerData")?.as_object()?;
    let name = tool.get("name")?.as_str()?.to_owned();
    let id = tool
        .get("toolCallId")
        .and_then(serde_json::Value::as_str)
        .or_else(|| record.get("bubbleId").and_then(serde_json::Value::as_str))?
        .to_owned();
    let input_value = tool.get("params").and_then(decoded_json_value);
    let paths = input_value
        .as_ref()
        .map(external_tool_paths)
        .unwrap_or_default();
    let output_value = tool.get("result").and_then(decoded_json_value);
    let diffs = cursor_diff(&paths, output_value.as_ref(), content)
        .into_iter()
        .collect();
    let kind = external_tool_kind(&name);
    let output = output_value
        .as_ref()
        .and_then(|output| external_tool_output(output, kind.as_deref()));
    Some(ExternalTool {
        id,
        kind,
        name,
        status: tool
            .get("status")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned),
        input: input_value.as_ref().and_then(external_json),
        output,
        paths,
        diffs,
    })
}

fn cursor_diff(
    paths: &[PathBuf],
    result: Option<&serde_json::Value>,
    content: &mut CursorContentCache<'_>,
) -> Option<ExternalDiff> {
    let result = result?.as_object()?;
    let path = paths.first()?.clone();
    let after_id = result.get("afterContentId")?.as_str()?;
    let new_text = content.get(after_id)?;
    let old_text = match result
        .get("beforeContentId")
        .and_then(serde_json::Value::as_str)
    {
        Some(id) => Some(content.get(id)?),
        None => None,
    };
    Some(ExternalDiff {
        path,
        old_text,
        new_text,
    })
}

struct CursorContentCache<'a> {
    query: Option<rusqlite::Statement<'a>>,
    values: HashMap<String, Option<Arc<str>>>,
}

impl<'a> CursorContentCache<'a> {
    fn new(connection: &'a Connection) -> Self {
        Self {
            query: connection
                .prepare("SELECT CAST(value AS TEXT) FROM cursorDiskKV WHERE key = ?1 LIMIT 1")
                .ok(),
            values: HashMap::new(),
        }
    }

    fn get(&mut self, id: &str) -> Option<Arc<str>> {
        if !valid_cursor_content_id(id) {
            return None;
        }
        if let Some(value) = self.values.get(id) {
            return value.clone();
        }
        let value = self
            .query
            .as_mut()
            .and_then(|query| query.query_row([id], |row| row.get::<_, String>(0)).ok())
            .filter(|value| value.len() <= MAX_EXTERNAL_DIFF_BYTES)
            .map(Arc::from);
        self.values.insert(id.to_owned(), value.clone());
        value
    }
}

fn valid_cursor_content_id(id: &str) -> bool {
    id.strip_prefix("composer.content.")
        .is_some_and(|hash| hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit()))
}

fn decoded_json_value(value: &serde_json::Value) -> Option<serde_json::Value> {
    match value {
        serde_json::Value::String(value) => serde_json::from_str(value)
            .ok()
            .or_else(|| Some(serde_json::Value::String(value.clone()))),
        serde_json::Value::Null => None,
        value => Some(value.clone()),
    }
}

fn external_tool_output(value: &serde_json::Value, kind: Option<&str>) -> Option<String> {
    if kind == Some("Task") {
        return external_json(value);
    }
    if let Some(text) = value.as_str() {
        return Some(bounded_external_text(text));
    }
    if let Some(parts) = value.as_array() {
        let text = parts
            .iter()
            .filter_map(|part| part.get("text").and_then(serde_json::Value::as_str))
            .collect::<Vec<_>>()
            .join("\n");
        if !text.is_empty() {
            return Some(bounded_external_text(&text));
        }
    }
    value
        .get("output")
        .and_then(|output| external_tool_output(output, None))
        .or_else(|| external_json(value))
}

fn external_json(value: &serde_json::Value) -> Option<String> {
    let text = match value {
        serde_json::Value::Null => return None,
        serde_json::Value::String(text) => text.clone(),
        value => serde_json::to_string_pretty(value).ok()?,
    };
    (!text.trim().is_empty()).then(|| bounded_external_text(&text))
}

fn bounded_external_text(text: &str) -> String {
    let mut end = text.len().min(MAX_HANDOFF_BYTES);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let truncated = end < text.len();
    let mut bounded = text[..end].to_owned();
    if truncated {
        bounded.push('…');
    }
    bounded
}

fn external_data_image(url: &str, total_bytes: &mut usize) -> Option<ExternalImage> {
    let (metadata, encoded) = url.strip_prefix("data:")?.split_once(',')?;
    let mut metadata = metadata.split(';');
    let mime_type = metadata.next()?;
    if !mime_type.starts_with("image/")
        || !metadata.any(|value| value.eq_ignore_ascii_case("base64"))
    {
        return None;
    }
    external_base64_image(mime_type, encoded, total_bytes)
}

fn external_base64_image(
    mime_type: &str,
    encoded: &str,
    total_bytes: &mut usize,
) -> Option<ExternalImage> {
    if !mime_type.starts_with("image/") || encoded.len() > MAX_EXTERNAL_IMAGE_BYTES.div_ceil(3) * 4
    {
        return None;
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .ok()?;
    if bytes.len() > MAX_EXTERNAL_IMAGE_BYTES
        || total_bytes.saturating_add(bytes.len()) > MAX_EXTERNAL_IMAGE_TOTAL_BYTES
    {
        return None;
    }
    *total_bytes += bytes.len();
    Some(ExternalImage {
        mime_type: mime_type.to_owned(),
        bytes: bytes.into(),
    })
}

pub(crate) fn is_subagent_tool_name(name: &str) -> bool {
    let name = name
        .bytes()
        .filter(u8::is_ascii_alphanumeric)
        .map(|byte| byte.to_ascii_lowercase())
        .collect::<Vec<_>>();
    matches!(
        name.as_slice(),
        b"task" | b"taskv2" | b"agent" | b"spawnagent" | b"spawnagents"
    )
}

fn external_tool_kind(name: &str) -> Option<String> {
    let normalized = name.to_ascii_lowercase();
    Some(
        if is_subagent_tool_name(name) {
            "Task"
        } else if normalized.contains("read") {
            "Read"
        } else if normalized.contains("edit")
            || normalized.contains("replace")
            || normalized.contains("write")
        {
            "Edit"
        } else if normalized.contains("terminal")
            || normalized.contains("shell")
            || normalized.contains("exec")
            || normalized == "bash"
        {
            "Execute"
        } else if normalized.contains("search")
            || normalized.contains("grep")
            || normalized.contains("glob")
        {
            "Search"
        } else if normalized.contains("fetch") {
            "Fetch"
        } else {
            return None;
        }
        .to_owned(),
    )
}

fn external_tool_paths(value: &serde_json::Value) -> Vec<PathBuf> {
    const PATH_KEYS: &[&str] = &[
        "path",
        "file_path",
        "relativeWorkspacePath",
        "target_directory",
        "targetDirectory",
    ];
    let Some(fields) = value.as_object() else {
        return Vec::new();
    };
    PATH_KEYS
        .iter()
        .filter_map(|key| fields.get(*key).and_then(serde_json::Value::as_str))
        .filter(|path| !path.trim().is_empty())
        .take(16)
        .map(PathBuf::from)
        .collect()
}

fn valid_session_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

#[cfg(test)]
fn cursor_transcript(path: &Path) -> Result<Vec<ExternalMessage>, String> {
    let mut messages = Vec::new();
    visit_cursor_transcript(path, &mut |message| {
        messages.push(message);
        Ok(())
    })?;
    Ok(messages)
}

fn visit_cursor_transcript(
    path: &Path,
    visit: &mut impl FnMut(ExternalMessage) -> Result<(), String>,
) -> Result<usize, String> {
    let file = File::open(path)
        .map_err(|error| format!("cannot read external session {}: {error}", path.display()))?;
    let mut last = None;
    let mut count = 0;
    for line in bounded_lines(file) {
        let line = line?;
        let Ok(record) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        let Some(role) = record.get("role").and_then(serde_json::Value::as_str) else {
            continue;
        };
        let Some(text) = record.pointer("/message/content").and_then(message_text) else {
            continue;
        };
        match role {
            "user" => visit_distinct(&mut last, ExternalMessage::User(text), &mut count, visit)?,
            "assistant" => visit_distinct(
                &mut last,
                ExternalMessage::Assistant(text),
                &mut count,
                visit,
            )?,
            _ => {}
        }
    }
    Ok(count)
}

fn codex_transcript(path: &Path) -> Result<Vec<ExternalMessage>, String> {
    let file = File::open(path)
        .map_err(|error| format!("cannot read external session {}: {error}", path.display()))?;
    let mut messages = Vec::new();
    let mut tools = HashMap::new();
    let mut pending_user_images = Vec::new();
    let mut image_bytes = 0;
    for line in bounded_lines(file).take(MAX_TRANSCRIPT_EVENTS) {
        let line = line?;
        let Ok(record) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        let payload = &record["payload"];
        match record.get("type").and_then(serde_json::Value::as_str) {
            Some("event_msg") => {
                let Some(text) = payload.get("message").and_then(serde_json::Value::as_str) else {
                    continue;
                };
                match payload.get("type").and_then(serde_json::Value::as_str) {
                    Some("user_message") => {
                        push_message(
                            &mut messages,
                            ExternalMessage::User(codex_user_message(text).to_owned()),
                        );
                        messages.extend(pending_user_images.drain(..).map(ExternalMessage::Image));
                    }
                    Some("agent_message") => {
                        push_message(&mut messages, ExternalMessage::Assistant(text.to_owned()));
                    }
                    _ => {}
                }
            }
            Some("response_item") => {
                match payload.get("type").and_then(serde_json::Value::as_str) {
                    Some("message")
                        if payload.get("role").and_then(serde_json::Value::as_str)
                            == Some("user") =>
                    {
                        pending_user_images.extend(
                            payload
                                .get("content")
                                .and_then(serde_json::Value::as_array)
                                .into_iter()
                                .flatten()
                                .filter(|part| {
                                    part.get("type").and_then(serde_json::Value::as_str)
                                        == Some("input_image")
                                })
                                .filter_map(|part| {
                                    part.get("image_url")
                                        .and_then(serde_json::Value::as_str)
                                        .and_then(|url| external_data_image(url, &mut image_bytes))
                                }),
                        );
                    }
                    Some("reasoning") => {
                        let thought = payload
                            .get("summary")
                            .and_then(serde_json::Value::as_array)
                            .into_iter()
                            .flatten()
                            .filter_map(|part| part.get("text").and_then(serde_json::Value::as_str))
                            .collect::<Vec<_>>()
                            .join("\n");
                        if !thought.is_empty() {
                            push_message(&mut messages, ExternalMessage::Thought(thought));
                        }
                    }
                    Some("custom_tool_call" | "function_call") => {
                        let Some(id) = payload.get("call_id").and_then(serde_json::Value::as_str)
                        else {
                            continue;
                        };
                        let Some(name) = payload.get("name").and_then(serde_json::Value::as_str)
                        else {
                            continue;
                        };
                        let input_value = payload
                            .get("arguments")
                            .or_else(|| payload.get("input"))
                            .and_then(decoded_json_value);
                        let diffs = if name.eq_ignore_ascii_case("apply_patch") {
                            input_value
                                .as_ref()
                                .and_then(serde_json::Value::as_str)
                                .map(codex_patch_diffs)
                                .unwrap_or_default()
                        } else {
                            Vec::new()
                        };
                        let mut paths = input_value
                            .as_ref()
                            .map(external_tool_paths)
                            .unwrap_or_default();
                        for diff in &diffs {
                            if !paths.contains(&diff.path) {
                                paths.push(diff.path.clone());
                            }
                        }
                        let tool = ExternalTool {
                            id: id.to_owned(),
                            name: name.to_owned(),
                            status: payload
                                .get("status")
                                .and_then(serde_json::Value::as_str)
                                .map(str::to_owned),
                            kind: external_tool_kind(name),
                            input: input_value.as_ref().and_then(external_json),
                            output: None,
                            paths,
                            diffs,
                        };
                        tools.insert(id.to_owned(), messages.len());
                        messages.push(ExternalMessage::Tool(tool));
                    }
                    Some("custom_tool_call_output" | "function_call_output") => {
                        let Some(id) = payload.get("call_id").and_then(serde_json::Value::as_str)
                        else {
                            continue;
                        };
                        let Some(ExternalMessage::Tool(tool)) =
                            tools.get(id).and_then(|index| messages.get_mut(*index))
                        else {
                            continue;
                        };
                        tool.output = payload
                            .get("output")
                            .and_then(|output| external_tool_output(output, tool.kind.as_deref()));
                        tool.status.get_or_insert_with(|| "completed".into());
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
    messages.extend(pending_user_images.into_iter().map(ExternalMessage::Image));
    Ok(messages)
}

fn codex_user_message(text: &str) -> &str {
    let wrapped = text.trim_start();
    if wrapped.starts_with("# Files mentioned by the user:")
        && let Some((_, request)) = wrapped.split_once("\n## My request:")
        && !request.trim().is_empty()
    {
        return request.trim();
    }
    text
}

struct CodexPatchFile {
    path: PathBuf,
    old_text: String,
    new_text: String,
    old_exists: bool,
}

fn codex_patch_diffs(patch: &str) -> Vec<ExternalDiff> {
    let mut diffs = Vec::new();
    let mut file = None;
    for line in patch.lines() {
        let header = [
            ("*** Update File: ", true),
            ("*** Add File: ", false),
            ("*** Delete File: ", true),
        ]
        .into_iter()
        .find_map(|(prefix, old_exists)| line.strip_prefix(prefix).map(|path| (path, old_exists)));
        if let Some((path, old_exists)) = header {
            push_codex_patch_file(&mut diffs, file.take());
            if !path.is_empty() && !path.contains('\0') {
                file = Some(CodexPatchFile {
                    path: path.into(),
                    old_text: String::new(),
                    new_text: String::new(),
                    old_exists,
                });
            }
            continue;
        }
        if line == "*** End Patch" {
            push_codex_patch_file(&mut diffs, file.take());
            continue;
        }
        let Some(file) = file.as_mut() else {
            continue;
        };
        if line.starts_with("@@") || line.starts_with("*** Move to: ") {
            continue;
        }
        match line.as_bytes().first() {
            Some(b'-') => push_patch_line(&mut file.old_text, &line[1..]),
            Some(b'+') => push_patch_line(&mut file.new_text, &line[1..]),
            Some(b' ') => {
                push_patch_line(&mut file.old_text, &line[1..]);
                push_patch_line(&mut file.new_text, &line[1..]);
            }
            _ => {
                push_patch_line(&mut file.old_text, line);
                push_patch_line(&mut file.new_text, line);
            }
        }
    }
    push_codex_patch_file(&mut diffs, file);
    diffs
}

fn push_patch_line(text: &mut String, line: &str) {
    text.push_str(line);
    text.push('\n');
}

fn push_codex_patch_file(diffs: &mut Vec<ExternalDiff>, file: Option<CodexPatchFile>) {
    let Some(file) = file else { return };
    if file.old_text.len().max(file.new_text.len()) > MAX_EXTERNAL_DIFF_BYTES
        || file.old_text == file.new_text
    {
        return;
    }
    diffs.push(ExternalDiff {
        path: file.path,
        old_text: file.old_exists.then(|| file.old_text.into()),
        new_text: file.new_text.into(),
    });
}

fn claude_summary(
    path: &Path,
    project_root: &Path,
    expected_id: &str,
) -> Result<Option<Option<String>>, String> {
    let file = File::open(path)
        .map_err(|error| format!("cannot read external session {}: {error}", path.display()))?;
    let expected_cwd = project_root.to_string_lossy();
    let mut matching_cwd = false;
    let mut matching_id = false;
    let mut title = None;
    let mut first_user = None;
    for line in bounded_lines(file).take(MAX_TRANSCRIPT_EVENTS) {
        let line = line?;
        let Ok(record) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        matching_cwd |=
            record.get("cwd").and_then(serde_json::Value::as_str) == Some(expected_cwd.as_ref());
        matching_id |=
            record.get("sessionId").and_then(serde_json::Value::as_str) == Some(expected_id);
        if title.is_none() {
            title = record
                .get("aiTitle")
                .and_then(serde_json::Value::as_str)
                .filter(|title| !title.trim().is_empty())
                .map(str::to_owned);
        }
        if first_user.is_none()
            && record.get("type").and_then(serde_json::Value::as_str) == Some("user")
        {
            first_user = record.pointer("/message/content").and_then(message_text);
        }
    }
    if !matching_cwd || !matching_id {
        return Ok(None);
    }
    Ok(Some(
        title.or_else(|| first_user.map(|text| shortened_title(&text))),
    ))
}

fn claude_transcript(path: &Path) -> Result<Vec<ExternalMessage>, String> {
    let file = File::open(path)
        .map_err(|error| format!("cannot read external session {}: {error}", path.display()))?;
    let mut messages = Vec::new();
    let mut tools = HashMap::new();
    let mut image_bytes = 0;
    for line in bounded_lines(file).take(MAX_TRANSCRIPT_EVENTS) {
        let line = line?;
        let Ok(record) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        if record
            .get("isSidechain")
            .and_then(serde_json::Value::as_bool)
            == Some(true)
        {
            continue;
        }
        let Some(content) = record.pointer("/message/content") else {
            continue;
        };
        let Some(role @ ("user" | "assistant")) =
            record.get("type").and_then(serde_json::Value::as_str)
        else {
            continue;
        };
        if let Some(text) = content.as_str() {
            push_message(
                &mut messages,
                if role == "user" {
                    ExternalMessage::User(text.to_owned())
                } else {
                    ExternalMessage::Assistant(text.to_owned())
                },
            );
            continue;
        }
        let Some(parts) = content.as_array() else {
            continue;
        };
        for part in parts {
            match part.get("type").and_then(serde_json::Value::as_str) {
                Some("image") if role == "user" => {
                    let Some(source) = part.get("source") else {
                        continue;
                    };
                    if source.get("type").and_then(serde_json::Value::as_str) != Some("base64") {
                        continue;
                    }
                    if let Some(image) = source
                        .get("media_type")
                        .and_then(serde_json::Value::as_str)
                        .zip(source.get("data").and_then(serde_json::Value::as_str))
                        .and_then(|(mime_type, data)| {
                            external_base64_image(mime_type, data, &mut image_bytes)
                        })
                    {
                        messages.push(ExternalMessage::Image(image));
                    }
                }
                Some("thinking") if role == "assistant" => {
                    if let Some(thought) = part
                        .get("thinking")
                        .or_else(|| part.get("text"))
                        .and_then(serde_json::Value::as_str)
                    {
                        push_message(&mut messages, ExternalMessage::Thought(thought.to_owned()));
                    }
                }
                Some("text") => {
                    let Some(text) = part.get("text").and_then(serde_json::Value::as_str) else {
                        continue;
                    };
                    push_message(
                        &mut messages,
                        if role == "user" {
                            ExternalMessage::User(text.to_owned())
                        } else {
                            ExternalMessage::Assistant(text.to_owned())
                        },
                    );
                }
                Some("tool_use") if role == "assistant" => {
                    let Some(id) = part.get("id").and_then(serde_json::Value::as_str) else {
                        continue;
                    };
                    let Some(name) = part.get("name").and_then(serde_json::Value::as_str) else {
                        continue;
                    };
                    let input_value = part.get("input").and_then(decoded_json_value);
                    let paths = input_value
                        .as_ref()
                        .map(external_tool_paths)
                        .unwrap_or_default();
                    let diffs = claude_tool_diffs(name, input_value.as_ref(), &paths);
                    let tool = ExternalTool {
                        id: id.to_owned(),
                        name: name.to_owned(),
                        status: Some("pending".into()),
                        kind: external_tool_kind(name),
                        input: input_value.as_ref().and_then(external_json),
                        output: None,
                        paths,
                        diffs,
                    };
                    tools.insert(id.to_owned(), messages.len());
                    messages.push(ExternalMessage::Tool(tool));
                }
                Some("tool_result") if role == "user" => {
                    let Some(id) = part.get("tool_use_id").and_then(serde_json::Value::as_str)
                    else {
                        continue;
                    };
                    let Some(ExternalMessage::Tool(tool)) =
                        tools.get(id).and_then(|index| messages.get_mut(*index))
                    else {
                        continue;
                    };
                    tool.output = part
                        .get("content")
                        .and_then(|output| external_tool_output(output, tool.kind.as_deref()));
                    let failed =
                        part.get("is_error").and_then(serde_json::Value::as_bool) == Some(true);
                    if failed {
                        tool.diffs.clear();
                    }
                    tool.status = Some(if failed { "failed" } else { "completed" }.into());
                }
                _ => {}
            }
        }
    }
    Ok(messages)
}

fn claude_tool_diffs(
    name: &str,
    input: Option<&serde_json::Value>,
    paths: &[PathBuf],
) -> Vec<ExternalDiff> {
    let Some(input) = input.and_then(serde_json::Value::as_object) else {
        return Vec::new();
    };
    let Some(path) = paths.first().cloned() else {
        return Vec::new();
    };
    let (old_text, new_text) = match name.to_ascii_lowercase().as_str() {
        "edit" => (
            input.get("old_string").and_then(serde_json::Value::as_str),
            input.get("new_string").and_then(serde_json::Value::as_str),
        ),
        "write" => (
            None,
            input.get("content").and_then(serde_json::Value::as_str),
        ),
        _ => return Vec::new(),
    };
    let Some(new_text) = new_text else {
        return Vec::new();
    };
    if old_text.map_or(0, str::len).max(new_text.len()) > MAX_EXTERNAL_DIFF_BYTES
        || old_text == Some(new_text)
    {
        return Vec::new();
    }
    vec![ExternalDiff {
        path,
        old_text: old_text.map(Arc::from),
        new_text: Arc::from(new_text),
    }]
}

pub(super) fn shortened_title(text: &str) -> String {
    let mut title = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if title.len() > 120 {
        let mut end = 120 - '…'.len_utf8();
        while !title.is_char_boundary(end) {
            end -= 1;
        }
        title.truncate(end);
        title.push('…');
    }
    title
}

fn message_text(content: &serde_json::Value) -> Option<String> {
    let text = if let Some(text) = content.as_str() {
        text.to_owned()
    } else {
        content
            .as_array()?
            .iter()
            .filter(|part| part.get("type").and_then(serde_json::Value::as_str) == Some("text"))
            .filter_map(|part| part.get("text").and_then(serde_json::Value::as_str))
            .collect::<Vec<_>>()
            .join("\n\n")
    };
    (!text.trim().is_empty()).then_some(text)
}

fn push_message(messages: &mut Vec<ExternalMessage>, message: ExternalMessage) {
    if messages.last() == Some(&message) {
        return;
    }
    messages.push(message);
}

fn visit_distinct(
    last: &mut Option<ExternalMessage>,
    message: ExternalMessage,
    count: &mut usize,
    visit: &mut impl FnMut(ExternalMessage) -> Result<(), String>,
) -> Result<(), String> {
    if last.as_ref() == Some(&message) {
        return Ok(());
    }
    *last = Some(message.clone());
    visit(message)?;
    *count += 1;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{fs, path::Path};

    use crate::agent::provider::ProviderId;
    use base64::Engine as _;
    use rusqlite::Connection;
    use tempfile::tempdir;

    use super::{
        CursorContentCache, ExternalMessage, ExternalSession, MAX_HANDOFF_BYTES, TranscriptFormat,
        claude_project_directory, claude_transcript, codex_transcript, cursor_database_transcript,
        cursor_project_directory, discover, discover_claude, discover_codex, discover_cursor,
        external_tool_kind, external_tool_output, provider_session_root,
    };

    #[test]
    fn explicit_account_provider_data_overrides_only_isolatable_session_roots() {
        let home = Path::new("/home/user");
        let account = Path::new("/data/agents/codex/accounts/7/provider-data");

        assert_eq!(
            provider_session_root(ProviderId::Codex, Some(account), home),
            account
        );
        assert_eq!(
            provider_session_root(ProviderId::Claude, Some(account), home),
            account
        );
        assert_eq!(
            provider_session_root(ProviderId::Cursor, Some(account), home),
            home
        );
        assert!(
            discover(
                ProviderId::Cursor,
                Path::new("/work/project"),
                Some(account)
            )
            .unwrap()
            .is_empty()
        );
    }

    #[test]
    fn provider_subagent_names_restore_as_tasks_without_matching_task_management() {
        for name in [
            "task",
            "Task V2",
            "task_v2",
            "task-v2",
            "Agent",
            "spawn_agent",
            "spawnAgent",
            "spawn_agents",
        ] {
            assert_eq!(external_tool_kind(name).as_deref(), Some("Task"), "{name}");
        }
        assert_eq!(external_tool_kind("TaskCreate"), None);
        assert_eq!(external_tool_kind("TaskUpdate"), None);
    }

    #[test]
    fn subagent_results_preserve_metadata_for_the_structured_card() {
        let result = serde_json::json!({
            "agentId": "agent-1",
            "durationMs": 900,
            "output": "No issues found",
        });
        let output = external_tool_output(&result, Some("Task")).unwrap();

        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&output).unwrap(),
            result
        );
    }

    #[test]
    fn cursor_simulated_task_notifications_restore_as_tools() {
        let fixture = tempdir().unwrap();
        let database = fixture.path().join("state.vscdb");
        let connection = Connection::open(&database).unwrap();
        connection
            .execute_batch("CREATE TABLE cursorDiskKV (key TEXT UNIQUE, value BLOB);")
            .unwrap();
        connection
            .execute(
                "INSERT INTO cursorDiskKV VALUES (?1, ?2)",
                (
                    "bubbleId:cursor-session:task-result",
                    serde_json::json!({
                        "bubbleId": "task-result",
                        "createdAt": "2026-08-14T19:48:54.340Z",
                        "type": 1,
                        "isSimulatedMsg": true,
                        "simulatedMsgReason": 3,
                        "simulatedMessageMetadata": {
                            "title": "Review authentication",
                            "taskId": "task-42"
                        },
                        "text": concat!(
                            "<system_notification>\n<task>\n",
                            "kind: subagent\nstatus: success\n",
                            "detail: <user_visible_high_level_summary>Done</user_visible_high_level_summary>\n",
                            "<response># Findings\nUse the shared callback.</response>\n",
                            "</task>\n</system_notification>"
                        )
                    })
                    .to_string(),
                ),
            )
            .unwrap();
        drop(connection);

        let transcript = cursor_database_transcript(&database, "cursor-session").unwrap();
        let [ExternalMessage::Tool(tool)] = transcript.as_slice() else {
            panic!("simulated task was not restored as a tool: {transcript:?}");
        };
        assert_eq!(tool.id, "task-42");
        assert_eq!(tool.name, "Background task · Review authentication");
        assert_eq!(tool.status.as_deref(), Some("success"));
        assert_eq!(
            tool.output.as_deref(),
            Some("# Findings\nUse the shared callback.")
        );
    }

    #[test]
    fn cursor_simulated_shell_notifications_keep_only_the_failure_detail() {
        let fixture = tempdir().unwrap();
        let database = fixture.path().join("state.vscdb");
        let connection = Connection::open(&database).unwrap();
        connection
            .execute_batch("CREATE TABLE cursorDiskKV (key TEXT UNIQUE, value BLOB);")
            .unwrap();
        connection
            .execute(
                "INSERT INTO cursorDiskKV VALUES (?1, ?2)",
                (
                    "bubbleId:cursor-session:shell-result",
                    serde_json::json!({
                        "bubbleId": "shell-result",
                        "createdAt": "2026-08-14T20:19:09.108Z",
                        "type": 1,
                        "isSimulatedMsg": true,
                        "simulatedMsgReason": 3,
                        "simulatedMessageMetadata": {
                            "title": "Typecheck backend",
                            "taskId": "679425"
                        },
                        "text": concat!(
                            "<system_notification>\n<task>\n",
                            "kind: shell\nstatus: error\ntask_id: 679425\n",
                            "detail: exit_code=-1\noutput_path: /tmp/result.txt\n",
                            "</task>\n</system_notification>"
                        )
                    })
                    .to_string(),
                ),
            )
            .unwrap();
        drop(connection);

        let transcript = cursor_database_transcript(&database, "cursor-session").unwrap();
        let [ExternalMessage::Tool(tool)] = transcript.as_slice() else {
            panic!("simulated shell task was not restored as a tool: {transcript:?}");
        };
        assert_eq!(tool.name, "Background task · Typecheck backend");
        assert_eq!(tool.status.as_deref(), Some("error"));
        assert_eq!(
            tool.output.as_deref(),
            Some("exit_code=-1\noutput_path: /tmp/result.txt")
        );
    }

    #[test]
    fn cursor_discovers_active_project_sessions_with_their_transcript() {
        let fixture = tempdir().unwrap();
        let project = fixture.path().join("work/editur");
        fs::create_dir_all(&project).unwrap();
        let projects = fixture.path().join("cursor-projects");
        let transcript_root = cursor_project_directory(&projects, &project)
            .join("agent-transcripts")
            .join("cursor-session");
        fs::create_dir_all(&transcript_root).unwrap();
        fs::write(
            transcript_root.join("cursor-session.jsonl"),
            concat!(
                "{\"role\":\"user\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"Fix the sidebar\"}]}}\n",
                "{\"role\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"Done.\"}]}}\n",
            ),
        )
        .unwrap();
        let archived_transcript =
            cursor_project_directory(&projects, &project).join("agent-transcripts/archived");
        fs::create_dir_all(&archived_transcript).unwrap();
        fs::write(
            archived_transcript.join("archived.jsonl"),
            "{\"role\":\"assistant\",\"message\":{\"content\":\"Do not show me\"}}\n",
        )
        .unwrap();

        let database = fixture.path().join("conversation-search.db");
        let connection = Connection::open(&database).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE conversations (
                    source TEXT NOT NULL,
                    id TEXT NOT NULL,
                    title TEXT NOT NULL,
                    updated_at INTEGER NOT NULL,
                    is_archived INTEGER NOT NULL
                );",
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO conversations VALUES (?1, ?2, ?3, ?4, ?5)",
                ("local", "cursor-session", "Sidebar fix", 200_i64, 0_i64),
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO conversations VALUES (?1, ?2, ?3, ?4, ?5)",
                ("local", "archived", "Old session", 100_i64, 1_i64),
            )
            .unwrap();
        drop(connection);

        let rich_database = database.with_file_name("state.vscdb");
        let connection = Connection::open(&rich_database).unwrap();
        connection
            .execute_batch("CREATE TABLE cursorDiskKV (key TEXT UNIQUE, value BLOB);")
            .unwrap();
        for (key, value) in [
            (
                "bubbleId:cursor-session:user",
                serde_json::json!({
                    "bubbleId": "user",
                    "createdAt": "2026-08-13T10:00:00Z",
                    "type": 1,
                    "text": "Fix the sidebar"
                }),
            ),
            (
                "bubbleId:cursor-session:assistant",
                serde_json::json!({
                    "bubbleId": "assistant",
                    "createdAt": "2026-08-13T10:00:01Z",
                    "type": 2,
                    "thinking": {
                        "text": "Checking the sidebar state.",
                        "signature": ""
                    },
                    "text": "I'll inspect it."
                }),
            ),
            (
                "bubbleId:cursor-session:tool",
                serde_json::json!({
                    "bubbleId": "tool",
                    "createdAt": "2026-08-13T10:00:02Z",
                    "type": 2,
                    "toolFormerData": {
                        "toolCallId": "tool-1",
                        "name": "run_terminal_command_v2",
                        "status": "completed",
                        "params": "{\"command\":\"cargo test\"}",
                        "result": "{\"output\":\"all tests passed\\n\"}"
                    }
                }),
            ),
            (
                "bubbleId:cursor-session:final",
                serde_json::json!({
                    "bubbleId": "final",
                    "createdAt": "2026-08-13T10:00:03Z",
                    "type": 2,
                    "text": "Done."
                }),
            ),
        ] {
            connection
                .execute(
                    "INSERT INTO cursorDiskKV VALUES (?1, ?2)",
                    (key, value.to_string()),
                )
                .unwrap();
        }
        drop(connection);

        let sessions = discover_cursor(&project, &database, &projects).unwrap();

        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].id, "cursor-session");
        assert_eq!(sessions[0].title.as_deref(), Some("Sidebar fix"));
        let transcript = sessions[0].transcript().unwrap();
        assert_eq!(transcript.len(), 5);
        assert_eq!(
            transcript[0],
            ExternalMessage::User("Fix the sidebar".into())
        );
        assert_eq!(
            transcript[1],
            ExternalMessage::Thought("Checking the sidebar state.".into())
        );
        assert_eq!(
            transcript[2],
            ExternalMessage::Assistant("I'll inspect it.".into())
        );
        assert!(matches!(
            &transcript[3],
            ExternalMessage::Tool(tool)
                if tool.id == "tool-1"
                    && tool.name == "run_terminal_command_v2"
                    && tool.status.as_deref() == Some("completed")
                    && tool.input.as_deref().is_some_and(|input| input.contains("cargo test"))
                    && tool.output.as_deref() == Some("all tests passed\n")
        ));
        assert_eq!(transcript[4], ExternalMessage::Assistant("Done.".into()));
        assert!(Path::new(&sessions[0].transcript_path).is_file());
    }

    #[test]
    fn cursor_edit_snapshots_are_imported_as_a_diff() {
        let fixture = tempdir().unwrap();
        let database = fixture.path().join("state.vscdb");
        let connection = Connection::open(&database).unwrap();
        connection
            .execute_batch("CREATE TABLE cursorDiskKV (key TEXT UNIQUE, value BLOB);")
            .unwrap();
        let before_id = format!("composer.content.{}", "a".repeat(64));
        let after_id = format!("composer.content.{}", "b".repeat(64));
        let result = serde_json::json!({
            "beforeContentId": before_id,
            "afterContentId": after_id,
        })
        .to_string();
        let bubble = serde_json::json!({
            "bubbleId": "tool",
            "createdAt": "2026-08-13T10:00:02Z",
            "type": 2,
            "toolFormerData": {
                "toolCallId": "tool-1",
                "name": "edit_file_v2",
                "status": "completed",
                "params": "{\"relativeWorkspacePath\":\"src/app.rs\"}",
                "result": result,
            }
        });
        for (key, value) in [
            (
                "bubbleId:cursor-session:tool".to_owned(),
                bubble.to_string(),
            ),
            (before_id, "fn before() {}\n".to_owned()),
            (after_id, "fn after() {}\n".to_owned()),
        ] {
            connection
                .execute("INSERT INTO cursorDiskKV VALUES (?1, ?2)", (key, value))
                .unwrap();
        }
        drop(connection);

        let transcript = cursor_database_transcript(&database, "cursor-session").unwrap();

        assert!(matches!(
            transcript.as_slice(),
            [ExternalMessage::Tool(tool)]
                if tool.diffs.len() == 1
                    && tool.diffs[0].path == Path::new("src/app.rs")
                    && tool.diffs[0].old_text.as_deref() == Some("fn before() {}\n")
                    && tool.diffs[0].new_text.as_ref() == "fn after() {}\n"
        ));
    }

    #[test]
    fn cursor_content_cache_reuses_a_loaded_revision() {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch("CREATE TABLE cursorDiskKV (key TEXT UNIQUE, value BLOB);")
            .unwrap();
        let id = format!("composer.content.{}", "a".repeat(64));
        connection
            .execute("INSERT INTO cursorDiskKV VALUES (?1, ?2)", (&id, "cached"))
            .unwrap();
        let mut cache = CursorContentCache::new(&connection);

        assert_eq!(cache.get(&id).as_deref(), Some("cached"));
        connection
            .execute("DELETE FROM cursorDiskKV WHERE key = ?1", [&id])
            .unwrap();
        assert_eq!(cache.get(&id).as_deref(), Some("cached"));
    }

    #[test]
    fn cursor_database_transcript_keeps_the_complete_session() {
        let fixture = tempdir().unwrap();
        let database = fixture.path().join("state.vscdb");
        let mut connection = Connection::open(&database).unwrap();
        connection
            .execute_batch("CREATE TABLE cursorDiskKV (key TEXT UNIQUE, value BLOB);")
            .unwrap();
        let transaction = connection.transaction().unwrap();
        for index in 0..300 {
            let bubble = serde_json::json!({
                "bubbleId": format!("user-{index}"),
                "createdAt": format!("2026-08-13T10:{:02}:00Z", index % 60),
                "type": 1,
                "text": format!("{index}:{}", "x".repeat(64 * 1024)),
            });
            transaction
                .execute(
                    "INSERT INTO cursorDiskKV VALUES (?1, ?2)",
                    (
                        format!("bubbleId:cursor-session:{index:04}"),
                        bubble.to_string(),
                    ),
                )
                .unwrap();
        }
        transaction.commit().unwrap();
        drop(connection);

        let transcript = cursor_database_transcript(&database, "cursor-session").unwrap();
        assert_eq!(transcript.len(), 300);
        assert!(matches!(&transcript[0], ExternalMessage::User(text) if text.starts_with("0:")));
        assert!(
            matches!(&transcript[299], ExternalMessage::User(text) if text.starts_with("299:"))
        );
    }

    #[test]
    fn cursor_jsonl_transcript_keeps_the_complete_session() {
        let fixture = tempdir().unwrap();
        let transcript_path = fixture.path().join("cursor-session.jsonl");
        let mut contents = String::new();
        for index in 0..300 {
            contents.push_str(
                &serde_json::json!({
                    "role": "assistant",
                    "message": {"content": format!("{index}:{}", "x".repeat(64 * 1024))},
                })
                .to_string(),
            );
            contents.push('\n');
        }
        fs::write(&transcript_path, contents).unwrap();

        let transcript = super::cursor_transcript(&transcript_path).unwrap();
        assert_eq!(transcript.len(), 300);
        assert!(
            matches!(&transcript[0], ExternalMessage::Assistant(text) if text.starts_with("0:"))
        );
        assert!(
            matches!(&transcript[299], ExternalMessage::Assistant(text) if text.starts_with("299:"))
        );
    }

    #[test]
    fn cursor_jsonl_transcript_has_no_event_count_cutoff() {
        let fixture = tempdir().unwrap();
        let transcript_path = fixture.path().join("cursor-session.jsonl");
        let contents = (0..4_100)
            .map(|index| {
                serde_json::json!({
                    "role": "assistant",
                    "message": {"content": index.to_string()},
                })
                .to_string()
            })
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(&transcript_path, contents).unwrap();

        assert_eq!(
            super::cursor_transcript(&transcript_path).unwrap().len(),
            4_100
        );
    }

    #[test]
    fn cursor_jsonl_transcript_rejects_one_oversized_record() {
        let fixture = tempdir().unwrap();
        let transcript_path = fixture.path().join("oversized.jsonl");
        fs::write(
            &transcript_path,
            serde_json::json!({
                "role": "assistant",
                "message": {"content": "x".repeat(24 * 1024 * 1024 + 1)},
            })
            .to_string(),
        )
        .unwrap();

        let error = super::cursor_transcript(&transcript_path).unwrap_err();
        assert!(error.contains("record exceeds"), "{error}");
    }

    #[test]
    fn bounded_lines_rejects_total_bytes_and_an_unterminated_record() {
        let mut lines = super::BoundedLines::with_limits(
            std::io::BufReader::new(std::io::Cursor::new(b"one\ntwo\nthree\n")),
            16,
            8,
        );
        assert_eq!(lines.next().unwrap().unwrap(), "one");
        assert_eq!(lines.next().unwrap().unwrap(), "two");
        assert!(lines.next().unwrap().unwrap_err().contains("scan limit"));

        let mut lines = super::BoundedLines::with_limits(
            std::io::BufReader::new(std::io::Cursor::new(b"oversized")),
            4,
            32,
        );
        assert!(
            lines
                .next()
                .unwrap()
                .unwrap_err()
                .contains("record exceeds")
        );
        assert!(lines.next().is_none());
    }

    #[test]
    fn cursor_workspace_images_are_imported_after_the_user_message() {
        let fixture = tempdir().unwrap();
        let database = fixture.path().join("Cursor/User/globalStorage/state.vscdb");
        fs::create_dir_all(database.parent().unwrap()).unwrap();
        let image_id = "8d407c70-4915-4270-8de5-53f77210e4ff";
        let image_dir = fixture
            .path()
            .join("Cursor/User/workspaceStorage/workspace/images");
        fs::create_dir_all(&image_dir).unwrap();
        let bytes = [0xff_u8, 0xd8, 0xff, 0xe0];
        fs::write(image_dir.join(format!("{image_id}-copy.png")), bytes).unwrap();
        let connection = Connection::open(&database).unwrap();
        connection
            .execute_batch("CREATE TABLE cursorDiskKV (key TEXT UNIQUE, value BLOB);")
            .unwrap();
        let bubble = serde_json::json!({
            "bubbleId": "user",
            "createdAt": "2026-08-13T10:00:00Z",
            "type": 1,
            "text": "Inspect this screenshot",
            "images": [{"uuid": image_id, "dimension": {"width": 1, "height": 1}}]
        });
        connection
            .execute(
                "INSERT INTO cursorDiskKV VALUES (?1, ?2)",
                ("bubbleId:cursor-session:user", bubble.to_string()),
            )
            .unwrap();
        drop(connection);

        let messages = cursor_database_transcript(&database, "cursor-session").unwrap();

        assert!(matches!(
            messages.as_slice(),
            [ExternalMessage::User(text), ExternalMessage::Image(image)]
                if text == "Inspect this screenshot"
                    && image.mime_type == "image/jpeg"
                    && image.bytes.as_ref() == bytes
        ));
    }

    #[test]
    fn codex_embedded_images_are_imported_after_the_user_message() {
        let fixture = tempdir().unwrap();
        let transcript = fixture.path().join("rollout.jsonl");
        let bytes = [0_u8, 1, 2, 3];
        let wrapped_message = concat!(
            "\n# Files mentioned by the user:\n\n",
            "## screenshot.png: /tmp/screenshot.png\n\n",
            "## My request:\n",
            "Inspect this screenshot\n",
        );
        let image_url = format!(
            "data:image/png;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(bytes)
        );
        let records = [
            serde_json::json!({
                "type": "response_item",
                "payload": {
                    "type": "message",
                    "role": "user",
                    "content": [
                        {"type": "input_text", "text": wrapped_message},
                        {"type": "input_image", "image_url": image_url}
                    ]
                }
            }),
            serde_json::json!({
                "type": "event_msg",
                "payload": {
                    "type": "user_message",
                    "message": wrapped_message
                }
            }),
        ];
        fs::write(
            &transcript,
            records
                .into_iter()
                .map(|record| record.to_string())
                .collect::<Vec<_>>()
                .join("\n"),
        )
        .unwrap();

        let messages = codex_transcript(&transcript).unwrap();

        assert!(matches!(
            messages.as_slice(),
            [ExternalMessage::User(text), ExternalMessage::Image(image)]
                if text == "Inspect this screenshot"
                    && image.mime_type == "image/png"
                    && image.bytes.as_ref() == bytes
        ));
    }

    #[test]
    fn codex_apply_patch_is_imported_as_a_diff() {
        let fixture = tempdir().unwrap();
        let transcript = fixture.path().join("rollout.jsonl");
        let patch = concat!(
            "*** Begin Patch\n",
            "*** Update File: src/main.rs\n",
            "@@\n",
            " fn main() {\n",
            "-    old();\n",
            "+    new();\n",
            " }\n",
            "*** End Patch\n",
        );
        fs::write(
            &transcript,
            format!(
                "{}\n",
                serde_json::json!({
                    "type": "response_item",
                    "payload": {
                        "type": "custom_tool_call",
                        "name": "apply_patch",
                        "call_id": "call-1",
                        "input": patch,
                        "status": "completed"
                    }
                })
            ),
        )
        .unwrap();

        let messages = codex_transcript(&transcript).unwrap();

        assert!(matches!(
            messages.as_slice(),
            [ExternalMessage::Tool(tool)]
                if tool.diffs.len() == 1
                    && tool.diffs[0].path == Path::new("src/main.rs")
                    && tool.diffs[0].old_text.as_deref()
                        == Some("fn main() {\n    old();\n}\n")
                    && tool.diffs[0].new_text.as_ref()
                        == "fn main() {\n    new();\n}\n"
        ));
    }

    #[test]
    fn claude_edit_is_imported_as_a_diff() {
        let fixture = tempdir().unwrap();
        let transcript = fixture.path().join("claude.jsonl");
        let records = [
            serde_json::json!({
                "type": "assistant",
                "message": {
                    "content": [{
                        "type": "tool_use",
                        "id": "toolu-1",
                        "name": "Edit",
                        "input": {
                            "file_path": "src/main.rs",
                            "old_string": "fn before() {}\n",
                            "new_string": "fn after() {}\n"
                        }
                    }]
                }
            }),
            serde_json::json!({
                "type": "user",
                "message": {
                    "content": [{
                        "type": "tool_result",
                        "tool_use_id": "toolu-1",
                        "content": "Updated src/main.rs",
                        "is_error": false
                    }]
                }
            }),
        ];
        fs::write(
            &transcript,
            records
                .into_iter()
                .map(|record| record.to_string())
                .collect::<Vec<_>>()
                .join("\n"),
        )
        .unwrap();

        let messages = claude_transcript(&transcript).unwrap();

        assert!(matches!(
            messages.as_slice(),
            [ExternalMessage::Tool(tool)]
                if tool.diffs.len() == 1
                    && tool.diffs[0].path == Path::new("src/main.rs")
                    && tool.diffs[0].old_text.as_deref() == Some("fn before() {}\n")
                    && tool.diffs[0].new_text.as_ref() == "fn after() {}\n"
        ));
    }

    #[test]
    fn claude_embedded_images_are_imported() {
        let fixture = tempdir().unwrap();
        let transcript = fixture.path().join("claude.jsonl");
        let bytes = [4_u8, 5, 6, 7];
        fs::write(
            &transcript,
            serde_json::json!({
                "type": "user",
                "message": {
                    "content": [{
                        "type": "image",
                        "source": {
                            "type": "base64",
                            "media_type": "image/jpeg",
                            "data": base64::engine::general_purpose::STANDARD.encode(bytes)
                        }
                    }]
                }
            })
            .to_string(),
        )
        .unwrap();

        let messages = claude_transcript(&transcript).unwrap();

        assert!(matches!(
            messages.as_slice(),
            [ExternalMessage::Image(image)]
                if image.mime_type == "image/jpeg" && image.bytes.as_ref() == bytes
        ));
    }

    #[test]
    fn claude_failed_edit_does_not_show_an_unapplied_diff() {
        let fixture = tempdir().unwrap();
        let transcript = fixture.path().join("claude.jsonl");
        fs::write(
            &transcript,
            concat!(
                "{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"tool_use\",\"id\":\"toolu-1\",\"name\":\"Edit\",\"input\":{\"file_path\":\"src/main.rs\",\"old_string\":\"before\",\"new_string\":\"after\"}}]}}\n",
                "{\"type\":\"user\",\"message\":{\"content\":[{\"type\":\"tool_result\",\"tool_use_id\":\"toolu-1\",\"content\":\"not found\",\"is_error\":true}]}}\n",
            ),
        )
        .unwrap();

        let messages = claude_transcript(&transcript).unwrap();

        assert!(matches!(
            messages.as_slice(),
            [ExternalMessage::Tool(tool)]
                if tool.status.as_deref() == Some("failed") && tool.diffs.is_empty()
        ));
    }

    #[test]
    fn codex_discovers_active_threads_for_the_exact_project() {
        let fixture = tempdir().unwrap();
        let project = fixture.path().join("work/editur");
        let codex_home = fixture.path().join(".codex");
        let transcript = codex_home.join("sessions/rollout-codex-session.jsonl");
        fs::create_dir_all(transcript.parent().unwrap()).unwrap();
        fs::create_dir_all(&project).unwrap();
        fs::write(
            &transcript,
            concat!(
                "{\"type\":\"session_meta\",\"payload\":{\"id\":\"codex-session\",\"cwd\":\"/ignored\"}}\n",
                "{\"type\":\"event_msg\",\"payload\":{\"type\":\"user_message\",\"message\":\"Review this code\"}}\n",
                "{\"type\":\"event_msg\",\"payload\":{\"type\":\"agent_message\",\"phase\":\"commentary\",\"message\":\"I'll run the tests.\"}}\n",
                "{\"type\":\"response_item\",\"payload\":{\"type\":\"reasoning\",\"summary\":[{\"type\":\"summary_text\",\"text\":\"Checking the test result\"}]}}\n",
                "{\"type\":\"response_item\",\"payload\":{\"type\":\"custom_tool_call\",\"name\":\"exec\",\"call_id\":\"call-1\",\"input\":\"cargo test\",\"status\":\"completed\"}}\n",
                "{\"type\":\"response_item\",\"payload\":{\"type\":\"custom_tool_call_output\",\"call_id\":\"call-1\",\"output\":[{\"type\":\"input_text\",\"text\":\"tests passed\"}]}}\n",
                "{\"type\":\"event_msg\",\"payload\":{\"type\":\"agent_message\",\"phase\":\"final_answer\",\"message\":\"Looks good.\"}}\n",
            ),
        )
        .unwrap();

        let database = codex_home.join("state_5.sqlite");
        let connection = Connection::open(&database).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE threads (
                    id TEXT PRIMARY KEY,
                    title TEXT NOT NULL,
                    rollout_path TEXT NOT NULL,
                    cwd TEXT NOT NULL,
                    updated_at INTEGER NOT NULL,
                    updated_at_ms INTEGER,
                    archived INTEGER NOT NULL
                );",
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO threads VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                (
                    "codex-session",
                    "Code review",
                    transcript.to_string_lossy().as_ref(),
                    project.to_string_lossy().as_ref(),
                    1_i64,
                    2_000_i64,
                    0_i64,
                ),
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO threads VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                (
                    "archived",
                    "Old review",
                    transcript.to_string_lossy().as_ref(),
                    project.to_string_lossy().as_ref(),
                    1_i64,
                    1_000_i64,
                    1_i64,
                ),
            )
            .unwrap();
        drop(connection);

        let sessions = discover_codex(&project, &database, &codex_home).unwrap();

        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].id, "codex-session");
        assert_eq!(sessions[0].title.as_deref(), Some("Code review"));
        let transcript = sessions[0].transcript().unwrap();
        assert_eq!(transcript.len(), 5);
        assert_eq!(
            transcript[0],
            ExternalMessage::User("Review this code".into())
        );
        assert_eq!(
            transcript[1],
            ExternalMessage::Assistant("I'll run the tests.".into())
        );
        assert_eq!(
            transcript[2],
            ExternalMessage::Thought("Checking the test result".into())
        );
        assert!(matches!(
            &transcript[3],
            ExternalMessage::Tool(tool)
                if tool.id == "call-1"
                    && tool.name == "exec"
                    && tool.input.as_deref() == Some("cargo test")
                    && tool.output.as_deref().is_some_and(|output| output.contains("tests passed"))
        ));
        assert_eq!(
            transcript[4],
            ExternalMessage::Assistant("Looks good.".into())
        );
    }

    #[test]
    fn claude_discovers_project_transcripts_and_ignores_other_cwds() {
        let fixture = tempdir().unwrap();
        let project = fixture.path().join("work/editur");
        let projects = fixture.path().join("claude-projects");
        let project_dir = claude_project_directory(&projects, &project);
        fs::create_dir_all(&project_dir).unwrap();
        fs::create_dir_all(&project).unwrap();
        fs::write(
            project_dir.join("claude-session.jsonl"),
            format!(
                concat!(
                    "{{\"type\":\"ai-title\",\"aiTitle\":\"Fix the renderer\",\"sessionId\":\"claude-session\"}}\n",
                    "{{\"type\":\"user\",\"sessionId\":\"claude-session\",\"cwd\":{},\"message\":{{\"role\":\"user\",\"content\":\"Investigate the renderer\"}}}}\n",
                    "{{\"type\":\"assistant\",\"sessionId\":\"claude-session\",\"cwd\":{},\"message\":{{\"role\":\"assistant\",\"content\":[{{\"type\":\"thinking\",\"thinking\":\"Checking the renderer state\"}},{{\"type\":\"text\",\"text\":\"I'll inspect it.\"}},{{\"type\":\"tool_use\",\"id\":\"toolu-1\",\"name\":\"Bash\",\"input\":{{\"command\":\"cargo test\"}}}}]}}}}\n",
                    "{{\"type\":\"user\",\"sessionId\":\"claude-session\",\"cwd\":{},\"message\":{{\"role\":\"user\",\"content\":[{{\"type\":\"tool_result\",\"tool_use_id\":\"toolu-1\",\"content\":\"tests passed\",\"is_error\":false}}]}}}}\n",
                    "{{\"type\":\"assistant\",\"sessionId\":\"claude-session\",\"cwd\":{},\"message\":{{\"role\":\"assistant\",\"content\":[{{\"type\":\"text\",\"text\":\"I found the issue.\"}}]}}}}\n",
                ),
                serde_json::to_string(project.to_string_lossy().as_ref()).unwrap(),
                serde_json::to_string(project.to_string_lossy().as_ref()).unwrap(),
                serde_json::to_string(project.to_string_lossy().as_ref()).unwrap(),
                serde_json::to_string(project.to_string_lossy().as_ref()).unwrap(),
            ),
        )
        .unwrap();
        fs::write(
            project_dir.join("wrong-project.jsonl"),
            "{\"type\":\"user\",\"sessionId\":\"wrong-project\",\"cwd\":\"/other\",\"message\":{\"role\":\"user\",\"content\":\"Ignore me\"}}\n",
        )
        .unwrap();

        let sessions = discover_claude(&project, &projects).unwrap();

        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].id, "claude-session");
        assert_eq!(sessions[0].title.as_deref(), Some("Fix the renderer"));
        let transcript = sessions[0].transcript().unwrap();
        assert_eq!(transcript.len(), 5);
        assert_eq!(
            transcript[0],
            ExternalMessage::User("Investigate the renderer".into())
        );
        assert_eq!(
            transcript[1],
            ExternalMessage::Thought("Checking the renderer state".into())
        );
        assert_eq!(
            transcript[2],
            ExternalMessage::Assistant("I'll inspect it.".into())
        );
        assert!(matches!(
            &transcript[3],
            ExternalMessage::Tool(tool)
                if tool.id == "toolu-1"
                    && tool.name == "Bash"
                    && tool.input.as_deref().is_some_and(|input| input.contains("cargo test"))
                    && tool.output.as_deref() == Some("tests passed")
                    && tool.status.as_deref() == Some("completed")
        ));
        assert_eq!(
            transcript[4],
            ExternalMessage::Assistant("I found the issue.".into())
        );
    }

    #[test]
    fn handoff_keeps_the_new_message_and_bounds_imported_history() {
        let fixture = tempdir().unwrap();
        let transcript = fixture.path().join("session.jsonl");
        fs::write(
            &transcript,
            format!(
                "{{\"role\":\"user\",\"message\":{{\"content\":[{{\"type\":\"text\",\"text\":{}}}]}}}}\n",
                serde_json::to_string(&"x".repeat(MAX_HANDOFF_BYTES * 2)).unwrap(),
            ),
        )
        .unwrap();
        let session = ExternalSession {
            id: "session".into(),
            title: None,
            updated_at: None,
            transcript_path: transcript,
            cursor_database: None,
            format: TranscriptFormat::Cursor,
        };

        let prompt = session.handoff_prompt("Fix the regression").unwrap();

        assert!(prompt.contains("Fix the regression"));
        assert!(prompt.len() <= MAX_HANDOFF_BYTES);
    }
}
