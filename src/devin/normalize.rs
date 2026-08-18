use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
};

use serde_json::Value;

use super::state::{
    Activity, Attachment, Automation, Blueprint, BlueprintFile, ChildSession, DevinMessage,
    DevinRepository, Integration, KnowledgeFolder, KnowledgeNote, KnowledgeSuggestion, Playbook,
    PullRequest, Review, Schedule, SecretMetadata, SessionDetail, SessionInsight, SessionSummary,
    SnapshotBuild, StatusCategory, Usage, WikiDocument,
};

const MAX_ITEMS: usize = 512;
const MAX_TEXT_BYTES: usize = 64 * 1024;
const MAX_DETAIL_BYTES: usize = 8 * 1024;
type SessionPage = (Vec<SessionSummary>, Option<String>, Option<usize>, bool);

pub(super) fn tool_payload(result: Value) -> Result<Value, String> {
    if result.get("isError").and_then(Value::as_bool) == Some(true) {
        return Err("Devin could not complete the request".into());
    }
    if let Some(structured) = result
        .get("structuredContent")
        .or_else(|| result.get("structured_content"))
    {
        let payload = structured.get("result").unwrap_or(structured);
        if let Some(text) = payload.as_str() {
            return Ok(decoded_tool_text(text));
        }
        return Ok(payload.clone());
    }
    if let Some(text) = result
        .get("content")
        .and_then(Value::as_array)
        .and_then(|content| {
            content.iter().find_map(|item| {
                (item.get("type").and_then(Value::as_str) == Some("text"))
                    .then(|| item.get("text").and_then(Value::as_str))
                    .flatten()
            })
        })
    {
        return Ok(decoded_tool_text(text));
    }
    Ok(result)
}

fn decoded_tool_text(text: &str) -> Value {
    serde_json::Deserializer::from_str(text)
        .into_iter::<Value>()
        .next()
        .and_then(Result::ok)
        .unwrap_or_else(|| Value::String(text.into()))
}

pub(super) fn sessions(value: &Value) -> Result<SessionPage, String> {
    if let Some(text) = value.as_str() {
        return text_session_page(text);
    }
    let items = array(value, &["sessions", "results", "items"])
        .ok_or_else(|| "Devin session search did not return a session list".to_owned())?;
    let next_cursor = string(
        value,
        &["end_cursor", "next_cursor", "nextCursor", "cursor"],
    );
    let total = integer(value, &["total", "total_count", "totalCount"])
        .and_then(|total| usize::try_from(total).ok());
    let has_next = boolean(value, &["has_next_page", "hasNextPage", "has_next"])
        .unwrap_or_else(|| next_cursor.is_some());
    Ok((
        items.iter().take(MAX_ITEMS).filter_map(session).collect(),
        next_cursor,
        total,
        has_next,
    ))
}

fn text_session_page(text: &str) -> Result<SessionPage, String> {
    let next_cursor = text.lines().find_map(|line| {
        line.strip_prefix("More results available. Pass after=")?
            .strip_suffix(" to fetch the next page.")
            .map(str::to_owned)
    });
    let total = text
        .lines()
        .find_map(|line| line.strip_prefix("Total: ")?.parse::<usize>().ok());
    let sessions: Vec<_> = text
        .lines()
        .filter_map(|line| {
            let line = line.strip_prefix("- ")?;
            let (line, created_at) = line.rsplit_once(" | created=")?;
            let (identity, status) = line.rsplit_once(" | status=")?;
            let (id, title) = identity.split_once(" | ")?;
            let (status, archived) = status
                .strip_suffix(" [archived]")
                .map_or((status, false), |status| (status, true));
            let (status, status_detail) = status
                .strip_suffix(')')
                .and_then(|status| status.split_once(" ("))
                .map_or((status, None), |(status, detail)| (status, Some(detail)));
            session(&serde_json::json!({
                "session_id": id,
                "title": title,
                "status": status,
                "status_detail": status_detail,
                "created_at": created_at,
                "is_archived": archived,
            }))
        })
        .take(MAX_ITEMS)
        .collect();
    if sessions.is_empty() && total != Some(0) {
        return Err("Devin session search did not return a session list".into());
    }
    let has_next = next_cursor.is_some();
    Ok((sessions, next_cursor, total, has_next))
}

pub(super) fn detail(value: &Value) -> Result<SessionDetail, String> {
    if let Some(text) = value.as_str() {
        return detail(&text_session_detail(text)?);
    }
    let root = object_at(value, &["session", "detail"]).unwrap_or(value);
    let summary = session(root)
        .ok_or_else(|| "Devin session details did not include an identity".to_owned())?;
    let attachments = array(root, &["attachments"])
        .into_iter()
        .flatten()
        .take(MAX_ITEMS)
        .filter_map(attachment)
        .collect();
    let pull_requests = array(root, &["pull_requests", "pullRequests", "prs"])
        .into_iter()
        .flatten()
        .take(MAX_ITEMS)
        .filter_map(pull_request)
        .collect();
    let children = array(
        root,
        &[
            "child_session_ids",
            "child_sessions",
            "childSessions",
            "children",
        ],
    )
    .into_iter()
    .flatten()
    .take(MAX_ITEMS)
    .filter_map(child_session)
    .collect();
    let usage_root = object_at(root, &["usage"]);
    let acus = usage_root
        .and_then(|usage| number(usage, &["acus", "acu", "used"]))
        .or_else(|| number(root, &["acus_consumed"]));
    let limit = usage_root
        .and_then(|usage| number(usage, &["limit", "acu_limit", "acuLimit"]))
        .or_else(|| number(root, &["max_acu_limit"]));
    let usage = (acus.is_some() || limit.is_some()).then_some(Usage { acus, limit });
    Ok(SessionDetail {
        summary,
        attachments,
        pull_requests,
        children,
        usage,
    })
}

fn text_session_detail(text: &str) -> Result<Value, String> {
    let mut fields = serde_json::Map::new();
    let mut pull_requests = Vec::new();
    let mut reading_pull_requests = false;
    for line in text.lines() {
        if line.starts_with("  pull_requests (") {
            reading_pull_requests = true;
            continue;
        }
        if reading_pull_requests && let Some(item) = line.strip_prefix("    - ") {
            let (url, status) = item
                .strip_suffix(']')
                .and_then(|item| item.rsplit_once(" ["))
                .map_or((item, None), |(url, status)| (url, Some(status)));
            pull_requests.push(serde_json::json!({"url": url, "status": status}));
            continue;
        }
        reading_pull_requests = false;
        let Some((key, value)) = line
            .strip_prefix("  ")
            .and_then(|line| line.split_once(": "))
        else {
            continue;
        };
        fields.insert(
            key.into(),
            match key {
                "acus_consumed" => value
                    .parse::<f64>()
                    .map(Value::from)
                    .unwrap_or_else(|_| Value::String(value.into())),
                "is_archived" => value
                    .parse::<bool>()
                    .map(Value::Bool)
                    .unwrap_or_else(|_| Value::String(value.into())),
                _ => Value::String(value.into()),
            },
        );
    }
    if !pull_requests.is_empty() {
        fields.insert("pull_requests".into(), Value::Array(pull_requests));
    }
    if !fields.contains_key("session_id") {
        return Err("Devin session details did not include an identity".into());
    }
    Ok(Value::Object(fields))
}

pub(super) fn messages(value: &Value) -> Result<(Vec<DevinMessage>, Option<String>), String> {
    let items = array(value, &["messages", "results", "items"])
        .ok_or_else(|| "Devin did not return a message list".to_owned())?;
    let messages = items
        .iter()
        .take(MAX_ITEMS)
        .filter_map(|item| {
            let role = string(item, &["source", "role", "author", "type"])
                .unwrap_or_else(|| "unknown".into());
            let raw_text = string(item, &["text", "message", "content"])?;
            if internal_prompt(&role, &raw_text) {
                return None;
            }
            let timestamp = timestamp(item);
            let id = identity(
                item,
                &["message_id", "messageId", "event_id", "eventId", "id"],
                &[&timestamp, &role, &raw_text],
            );
            let mut attachment_ids =
                array(item, &["attachment_ids", "attachmentIds", "attachments"])
                    .into_iter()
                    .flatten()
                    .filter_map(|attachment| {
                        attachment
                            .as_str()
                            .map(str::to_owned)
                            .or_else(|| string(attachment, &["id", "attachment_id"]))
                    })
                    .take(MAX_ITEMS)
                    .collect::<Vec<_>>();
            let text = if role.eq_ignore_ascii_case("user") {
                slack_handoff(&raw_text).map_or(raw_text, |(text, embedded_ids)| {
                    for id in embedded_ids {
                        if attachment_ids.len() == MAX_ITEMS {
                            break;
                        }
                        if !attachment_ids.contains(&id) {
                            attachment_ids.push(id);
                        }
                    }
                    text
                })
            } else {
                raw_text
            };
            Some(DevinMessage {
                id,
                timestamp,
                role,
                text: bounded(text, MAX_TEXT_BYTES),
                attachment_ids,
            })
        })
        .collect();
    Ok((
        messages,
        string(
            value,
            &["end_cursor", "next_cursor", "nextCursor", "cursor"],
        ),
    ))
}

fn internal_prompt(role: &str, text: &str) -> bool {
    if role.eq_ignore_ascii_case("system") || role.eq_ignore_ascii_case("developer") {
        return true;
    }
    if !role.eq_ignore_ascii_case("user") {
        return false;
    }
    // ponytail: recognize Devin's current injected repository wrapper; use
    // structured message visibility if the API exposes it.
    let text = text.to_ascii_lowercase();
    text.starts_with("you are working in the *")
        && text.contains("\n*objective*\n")
        && text.contains("\nbranch and pr requirements")
}

fn slack_handoff(text: &str) -> Option<(String, Vec<String>)> {
    let latest = text
        .strip_prefix("SYSTEM:\n<latest_message>\n")?
        .split_once("\n</latest_message>")?
        .0;
    let (_, message) = latest.split_once("]: ")?;
    let message = message
        .strip_prefix("@Devin")
        .or_else(|| message.strip_prefix("@devin"))
        .unwrap_or(message)
        .trim();
    if message.is_empty() {
        return None;
    }
    let attachment_ids = text
        .lines()
        .filter_map(|line| {
            let url = line
                .trim()
                .strip_prefix("ATTACHMENT:\"")?
                .strip_suffix('"')?;
            let path = url.strip_prefix("https://app.devin.ai/attachments/")?;
            let (id, name) = path.split_once('/')?;
            (!id.is_empty()
                && !name.is_empty()
                && id
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_')))
            .then(|| id.to_owned())
        })
        .take(MAX_ITEMS)
        .collect();
    Some((message.to_owned(), attachment_ids))
}

pub(super) fn activity(value: &Value) -> Result<(Vec<Activity>, Option<String>), String> {
    if let Some(text) = value.as_str() {
        return text_activity_page(text);
    }
    let items = array(value, &["events", "activity", "results", "items"])
        .ok_or_else(|| "Devin did not return an event list".to_owned())?;
    let activity = items
        .iter()
        .take(MAX_ITEMS)
        .map(|item| {
            let category = string(
                item,
                &["category", "event_type", "eventType", "type", "kind"],
            )
            .unwrap_or_else(|| "activity".into());
            let summary = string(item, &["summary", "title", "message", "description"])
                .unwrap_or_else(|| category.clone());
            let timestamp = timestamp(item);
            let id = identity(
                item,
                &["event_id", "eventId", "id"],
                &[&timestamp, &category, &summary],
            );
            Activity {
                id,
                timestamp,
                category,
                summary: bounded(summary, MAX_DETAIL_BYTES),
                details: string(item, &["details", "detail", "output"])
                    .map(|value| bounded(value, MAX_DETAIL_BYTES)),
                path: string(item, &["path", "file_path", "filePath"]),
                command: string(item, &["command", "cmd"])
                    .map(|value| bounded(value, MAX_DETAIL_BYTES)),
                url: string(item, &["url"]).and_then(|url| external_url(&url)),
                attachment_id: string(item, &["attachment_id", "attachmentId"]),
                pull_request_url: string(item, &["pull_request_url", "pullRequestUrl", "pr_url"])
                    .and_then(|url| external_url(&url)),
                child_session_id: string(item, &["child_session_id", "childSessionId"]),
            }
        })
        .collect();
    Ok((
        activity,
        string(
            value,
            &["end_cursor", "next_cursor", "nextCursor", "cursor"],
        ),
    ))
}

pub(super) fn activity_details(value: &Value) -> Result<String, String> {
    let details = value
        .as_str()
        .map(str::to_owned)
        .or_else(|| string(value, &["details", "detail", "content", "output"]))
        .filter(|details| !details.trim().is_empty())
        .ok_or_else(|| "Devin did not return event details".to_owned())?;
    Ok(bounded(details, MAX_DETAIL_BYTES))
}

fn text_activity_page(text: &str) -> Result<(Vec<Activity>, Option<String>), String> {
    let next_cursor = text.lines().find_map(|line| {
        line.strip_prefix("More results available. Pass after=")?
            .strip_suffix(" to fetch the next page.")
            .map(str::to_owned)
    });
    let activity = text
        .lines()
        .filter_map(|line| {
            let (id, rest) = line.strip_prefix('[')?.split_once("] ")?;
            let (timestamp, rest) = rest
                .split_once(" <<< ")
                .or_else(|| rest.split_once(" >>> "))?;
            let (kind, summary) = rest.split_once(": ")?;
            let (event_type, category) = kind
                .strip_suffix(')')
                .and_then(|kind| kind.rsplit_once(" ("))
                .map_or((kind, kind), |(event_type, category)| {
                    (event_type, category)
                });
            Some(Activity {
                id: id.into(),
                timestamp: timestamp.into(),
                category: category.into(),
                summary: bounded(summary.into(), MAX_DETAIL_BYTES),
                details: None,
                path: None,
                command: (category == "shell").then(|| event_type.into()),
                url: None,
                attachment_id: None,
                pull_request_url: None,
                child_session_id: None,
            })
        })
        .take(MAX_ITEMS)
        .collect::<Vec<_>>();
    let total_is_zero = text.lines().any(|line| line == "Total: 0");
    if activity.is_empty() && !total_is_zero {
        return Err("Devin did not return an event list".into());
    }
    Ok((activity, next_cursor))
}

pub(super) fn attachments(value: &Value) -> Vec<Attachment> {
    resource_items(value, &["attachments", "results", "items"])
        .take(MAX_ITEMS)
        .filter_map(attachment)
        .collect()
}

pub(super) fn repositories(value: &Value) -> Vec<DevinRepository> {
    array(value, &["repositories", "repos", "results", "items"])
        .into_iter()
        .flatten()
        .take(MAX_ITEMS)
        .filter_map(|item| {
            if let Some(name) = item.as_str() {
                return Some(DevinRepository {
                    id: name.to_owned(),
                    name: name.to_owned(),
                    ..DevinRepository::default()
                });
            }
            let name = string(
                item,
                &[
                    "repository_path",
                    "repo_name",
                    "repository",
                    "name",
                    "full_name",
                ],
            )?;
            Some(DevinRepository {
                id: string(item, &["repository_path", "repo_id", "id"])
                    .unwrap_or_else(|| name.clone()),
                name,
                indexed: boolean(
                    item,
                    &["indexing_enabled", "indexed", "is_indexed", "enabled"],
                )
                .unwrap_or(false),
                indexing_status: string(item, &["indexing_status", "status", "state"]),
                branches: array(item, &["branches", "indexed_branches"])
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .take(MAX_ITEMS)
                    .collect(),
            })
        })
        .collect()
}

pub(super) fn wiki_documents(value: &Value, repository: &str) -> Vec<WikiDocument> {
    if let Some(items) = array(value, &["topics", "pages", "documents", "items", "results"]) {
        return items
            .iter()
            .take(MAX_ITEMS)
            .map(|item| WikiDocument {
                repository: repository.to_owned(),
                title: string(item, &["title", "name", "path"])
                    .or_else(|| item.as_str().map(str::to_owned))
                    .unwrap_or_else(|| "Documentation".into()),
                content: string(item, &["content", "text", "body"])
                    .map(|content| bounded(content, MAX_TEXT_BYTES))
                    .unwrap_or_default(),
                citations: string_array(item, &["citations", "sources", "paths"]),
            })
            .collect();
    }
    vec![WikiDocument {
        repository: repository.to_owned(),
        title: string(value, &["title", "question"]).unwrap_or_else(|| "Documentation".into()),
        content: string(value, &["content", "answer", "text", "body"])
            .or_else(|| value.as_str().map(str::to_owned))
            .map(|content| bounded(content, MAX_TEXT_BYTES))
            .unwrap_or_default(),
        citations: string_array(value, &["citations", "sources", "paths"]),
    }]
}

pub(super) fn knowledge(value: &Value) -> Vec<KnowledgeNote> {
    resource_items(value, &["notes", "knowledge", "items", "results"])
        .filter_map(|item| {
            Some(KnowledgeNote {
                id: string(item, &["note_id", "knowledge_id", "id"])?,
                name: string(item, &["name", "title"]).unwrap_or_else(|| "Knowledge note".into()),
                content: string(item, &["content", "body", "text"])
                    .map(|content| bounded(content, MAX_TEXT_BYTES))
                    .unwrap_or_default(),
                folder: string(item, &["folder", "folder_path", "folder_id"]),
                repositories: string_array(item, &["repositories", "repos", "repo_names"]),
            })
        })
        .collect()
}

pub(super) fn knowledge_detail(value: &Value) -> Option<KnowledgeNote> {
    knowledge(value).into_iter().next().or_else(|| {
        let item = object_at(value, &["note", "knowledge"]).unwrap_or(value);
        knowledge(&serde_json::json!({ "notes": [item] }))
            .into_iter()
            .next()
    })
}

pub(super) fn knowledge_folders(value: &Value) -> Vec<KnowledgeFolder> {
    resource_items(value, &["folders", "items", "results"])
        .filter_map(|item| {
            Some(KnowledgeFolder {
                id: string(item, &["folder_id", "id", "path"])?,
                name: string(item, &["name", "title", "path"])
                    .unwrap_or_else(|| "Knowledge folder".into()),
                note_count: integer(item, &["note_count", "notes_count", "count"]).unwrap_or(0)
                    as usize,
            })
        })
        .collect()
}

pub(super) fn knowledge_suggestions(value: &Value) -> Vec<KnowledgeSuggestion> {
    resource_items(value, &["suggestions", "items", "results"])
        .filter_map(|item| {
            Some(KnowledgeSuggestion {
                id: string(item, &["suggestion_id", "id"])?,
                title: string(item, &["title", "name"])
                    .unwrap_or_else(|| "Knowledge suggestion".into()),
                content: string(item, &["content", "body", "text"])
                    .map(|content| bounded(content, MAX_TEXT_BYTES))
                    .unwrap_or_default(),
            })
        })
        .collect()
}

pub(super) fn knowledge_suggestion_detail(value: &Value) -> Option<KnowledgeSuggestion> {
    knowledge_suggestions(value).into_iter().next().or_else(|| {
        let item = object_at(value, &["suggestion"]).unwrap_or(value);
        knowledge_suggestions(&serde_json::json!({ "suggestions": [item] }))
            .into_iter()
            .next()
    })
}

pub(super) fn playbooks(value: &Value) -> Vec<Playbook> {
    resource_items(value, &["playbooks", "items", "results"])
        .filter_map(|item| {
            Some(Playbook {
                id: string(item, &["playbook_id", "id"])?,
                title: string(item, &["title", "name"]).unwrap_or_else(|| "Playbook".into()),
                content: string(item, &["content", "body", "prompt"])
                    .map(|content| bounded(content, MAX_TEXT_BYTES))
                    .unwrap_or_default(),
                automation_macro: string(item, &["automation_macro", "macro"]),
            })
        })
        .collect()
}

pub(super) fn playbook_detail(value: &Value) -> Option<Playbook> {
    playbooks(value).into_iter().next().or_else(|| {
        let item = object_at(value, &["playbook"]).unwrap_or(value);
        playbooks(&serde_json::json!({ "playbooks": [item] }))
            .into_iter()
            .next()
    })
}

pub(super) fn schedules(value: &Value) -> Vec<Schedule> {
    resource_items(value, &["schedules", "items", "results"])
        .filter_map(|item| {
            Some(Schedule {
                id: string(item, &["schedule_id", "id"])?,
                title: string(item, &["title", "name"]).unwrap_or_else(|| "Schedule".into()),
                prompt: string(item, &["prompt", "message", "task"])
                    .map(|prompt| bounded(prompt, MAX_TEXT_BYTES))
                    .unwrap_or_default(),
                cadence: string(item, &["cron", "schedule", "run_at", "frequency"])
                    .unwrap_or_default(),
                enabled: boolean(item, &["enabled", "is_enabled", "active"]).unwrap_or(false),
                configuration: item.clone(),
            })
        })
        .collect()
}

pub(super) fn schedule_detail(value: &Value) -> Option<Schedule> {
    schedules(value).into_iter().next().or_else(|| {
        let item = object_at(value, &["schedule"]).unwrap_or(value);
        schedules(&serde_json::json!({ "schedules": [item] }))
            .into_iter()
            .next()
    })
}

pub(super) fn automations(value: &Value) -> Vec<Automation> {
    resource_items(value, &["automations", "items", "results"])
        .filter_map(|item| {
            Some(Automation {
                id: string(item, &["automation_id", "id"])?,
                title: string(item, &["title", "name"]).unwrap_or_else(|| "Automation".into()),
                enabled: boolean(item, &["enabled", "is_enabled", "active"]).unwrap_or(false),
                summary: string(item, &["summary", "description", "trigger"])
                    .map(|summary| bounded(summary, MAX_DETAIL_BYTES))
                    .unwrap_or_default(),
                configuration: item.clone(),
            })
        })
        .collect()
}

pub(super) fn automation_detail(value: &Value) -> Option<Automation> {
    automations(value).into_iter().next().or_else(|| {
        let item = object_at(value, &["automation"]).unwrap_or(value);
        automations(&serde_json::json!({ "automations": [item] }))
            .into_iter()
            .next()
    })
}

pub(super) fn integrations(value: &Value) -> Vec<Integration> {
    let mut items = Vec::new();
    if let Some(values) = value.as_array() {
        items.extend(values.iter().map(|item| (item, "")));
    } else {
        for (name, kind) in [("integrations", "native"), ("mcp_servers", "mcp")] {
            if let Some(values) = value.get(name).and_then(Value::as_array) {
                items.extend(values.iter().map(|item| (item, kind)));
            }
        }
    }
    items
        .into_iter()
        .take(MAX_ITEMS)
        .filter_map(|(item, default_kind)| {
            Some(Integration {
                id: string(item, &["integration_id", "server_id", "id", "slug", "name"])?,
                name: string(item, &["display_name", "name", "title"])
                    .unwrap_or_else(|| "Integration".into()),
                kind: string(item, &["kind", "type", "provider"])
                    .unwrap_or_else(|| default_kind.into()),
                installed: boolean(item, &["installed", "is_installed", "configured"])
                    .unwrap_or(false),
                url: string(item, &["settings_url", "setup_url", "configure_url", "url"])
                    .and_then(|url| integration_url(&url)),
            })
        })
        .collect()
}

pub(super) fn reviews(value: &Value) -> Vec<Review> {
    let mut items = resource_items(value, &["reviews", "items", "results"]).collect::<Vec<_>>();
    if items.is_empty() {
        items.push(value);
    }
    items
        .into_iter()
        .filter_map(|item| {
            Some(Review {
                pull_request_url: string(item, &["pr_url", "pull_request_url", "url"])?,
                status: string(item, &["status", "state"]).unwrap_or_else(|| "unknown".into()),
                result_url: string(item, &["result_url", "review_url"])
                    .and_then(|url| external_url(&url)),
                updated_at: string(item, &["updated_at", "created_at"]),
            })
        })
        .collect()
}

pub(super) fn blueprints(value: &Value) -> Vec<Blueprint> {
    resource_items(value, &["blueprints", "items", "results"])
        .filter_map(|item| {
            Some(Blueprint {
                id: string(item, &["blueprint_id", "id"])?,
                name: string(item, &["name", "title", "type"])
                    .unwrap_or_else(|| "Blueprint".into()),
                repository: string(item, &["repo_name", "repository"]),
                contents_url: string(item, &["contents_url", "url"])
                    .and_then(|url| external_url(&url)),
                contents: string(item, &["contents", "yaml", "body"])
                    .map(|contents| bounded(contents, MAX_TEXT_BYTES)),
            })
        })
        .collect()
}

pub(super) fn blueprint_files(value: &Value) -> Vec<BlueprintFile> {
    resource_items(value, &["files", "items", "results"])
        .filter_map(|item| {
            Some(BlueprintFile {
                id: string(item, &["file_id", "attachment_id", "id"])?,
                name: string(item, &["name", "filename", "file_name"])
                    .unwrap_or_else(|| "Blueprint file".into()),
                size: integer(item, &["size", "byte_size"]),
            })
        })
        .collect()
}

pub(super) fn builds(value: &Value) -> Vec<SnapshotBuild> {
    resource_items(value, &["builds", "items", "results"])
        .filter_map(|item| {
            Some(SnapshotBuild {
                id: string(item, &["build_id", "id"])?,
                status: string(item, &["status", "state"]).unwrap_or_else(|| "unknown".into()),
                created_at: string(item, &["created_at", "started_at"]),
                logs_url: string(item, &["logs_url", "log_url"]).and_then(|url| external_url(&url)),
                pinned: boolean(item, &["pinned", "is_pinned"]).unwrap_or(false),
                configuration: item.clone(),
            })
        })
        .collect()
}

pub(super) fn build_detail(value: &Value) -> Option<SnapshotBuild> {
    builds(value).into_iter().next().or_else(|| {
        let item = object_at(value, &["build"]).unwrap_or(value);
        builds(&serde_json::json!({ "builds": [item] }))
            .into_iter()
            .next()
    })
}

pub(super) fn secrets(value: &Value) -> Vec<SecretMetadata> {
    resource_items(value, &["secrets", "items", "results"])
        .filter_map(|item| {
            Some(SecretMetadata {
                id: string(item, &["secret_id", "id"])?,
                name: string(item, &["name", "key"]).unwrap_or_else(|| "Secret".into()),
                scope: string(item, &["scope", "type"]),
            })
        })
        .collect()
}

pub(super) fn insights(value: &Value) -> Vec<SessionInsight> {
    let mut items =
        resource_items(value, &["insights", "sessions", "items", "results"]).collect::<Vec<_>>();
    if items.is_empty() {
        items.push(value);
    }
    items
        .into_iter()
        .filter_map(|item| {
            Some(SessionInsight {
                session_id: string(item, &["session_id", "devin_id", "id"])?,
                status: string(item, &["status", "generation_status", "state"])
                    .unwrap_or_else(|| "unknown".into()),
                summary: string(item, &["summary", "analysis", "insight"])
                    .map(|summary| bounded(summary, MAX_TEXT_BYTES)),
                message_count: integer(item, &["message_count", "messages_count"]),
            })
        })
        .collect()
}

fn session(value: &Value) -> Option<SessionSummary> {
    let id = string(value, &["session_id", "sessionId", "id"])?;
    let status = string(value, &["status", "state"]).unwrap_or_else(|| "unknown".into());
    let status_detail = string(value, &["status_detail", "statusDetail"]);
    let archived = boolean(value, &["is_archived", "archived", "isArchived"]).unwrap_or(false);
    Some(SessionSummary {
        id,
        title: string(value, &["title", "name"]).unwrap_or_else(|| "Untitled session".into()),
        prompt: string(
            value,
            &["prompt", "initial_prompt", "initialPrompt", "task"],
        )
        .filter(|prompt| !internal_prompt("user", prompt))
        .map(|value| bounded(value, MAX_DETAIL_BYTES)),
        category: status_category(&status, status_detail.as_deref(), archived),
        status,
        status_detail,
        origin: string(value, &["origin", "source"]),
        repository: string(value, &["repository", "repo", "repo_name", "repoName"]),
        created_at: string(value, &["created_at", "createdAt", "created"]),
        updated_at: string(value, &["updated_at", "updatedAt", "updated"]),
        archived,
        parent_session_id: string(
            value,
            &["parent_session_id", "parentSessionId", "parent_id"],
        ),
        url: string(
            value,
            &["url", "session_url", "sessionUrl", "web_url", "webUrl"],
        )
        .and_then(|url| external_url(&url)),
        pull_request_count: array(value, &["pull_requests", "pullRequests", "prs"]).map_or_else(
            || usize::from(object_at(value, &["pull_request", "pullRequest", "pr"]).is_some()),
            Vec::len,
        ),
        tags: array(value, &["tags"])
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .take(MAX_ITEMS)
            .collect(),
        org_id: string(value, &["org_id"]),
        user_id: string(value, &["user_id"]),
        service_user_id: string(value, &["service_user_id"]),
        automation_id: string(value, &["automation_id"]),
        devin_mode: string(value, &["devin_mode"]),
        playbook_id: string(value, &["playbook_id"]),
        session_category: string(value, &["category"]),
        subcategory: string(value, &["subcategory"]),
        structured_output: object_at(value, &["structured_output"])
            .filter(|output| !output.is_null())
            .cloned(),
    })
}

fn attachment(value: &Value) -> Option<Attachment> {
    Some(Attachment {
        id: string(value, &["attachment_id", "attachmentId", "id"])?,
        name: string(value, &["name", "filename", "file_name"])
            .unwrap_or_else(|| "Attachment".into()),
        media_type: string(
            value,
            &["media_type", "mediaType", "content_type", "contentType"],
        ),
        size: integer(value, &["size", "byte_size", "byteSize"]),
        url: string(value, &["url", "download_url", "downloadUrl"])
            .and_then(|url| external_url(&url)),
    })
}

fn pull_request(value: &Value) -> Option<PullRequest> {
    let url = external_url(&string(value, &["pr_url", "url", "html_url", "htmlUrl"])?)?;
    Some(PullRequest {
        id: string(value, &["id", "number"]).unwrap_or_else(|| url.clone()),
        title: string(value, &["title", "name"]).unwrap_or_else(|| "Pull request".into()),
        url,
        status: string(value, &["pr_state", "status", "state"]),
    })
}

fn child_session(value: &Value) -> Option<ChildSession> {
    if let Some(id) = value.as_str() {
        return Some(ChildSession {
            id: id.to_owned(),
            title: "Child session".into(),
            status: "unknown".into(),
        });
    }
    Some(ChildSession {
        id: string(value, &["session_id", "sessionId", "id"])?,
        title: string(value, &["title", "name"]).unwrap_or_else(|| "Child session".into()),
        status: string(value, &["status", "state"]).unwrap_or_else(|| "unknown".into()),
    })
}

fn object_at<'a>(value: &'a Value, names: &[&str]) -> Option<&'a Value> {
    names.iter().find_map(|name| value.get(name)).or_else(|| {
        ["data", "result"].iter().find_map(|parent| {
            value
                .get(parent)
                .and_then(|value| names.iter().find_map(|name| value.get(name)))
        })
    })
}

fn array<'a>(value: &'a Value, names: &[&str]) -> Option<&'a Vec<Value>> {
    object_at(value, names).and_then(Value::as_array)
}

fn resource_items<'a>(value: &'a Value, names: &[&str]) -> impl Iterator<Item = &'a Value> {
    value
        .as_array()
        .or_else(|| array(value, names))
        .into_iter()
        .flatten()
        .take(MAX_ITEMS)
}

fn string_array(value: &Value, names: &[&str]) -> Vec<String> {
    array(value, names)
        .into_iter()
        .flatten()
        .filter_map(|value| {
            value
                .as_str()
                .map(str::to_owned)
                .or_else(|| string(value, &["name", "path", "url", "id"]))
        })
        .take(MAX_ITEMS)
        .collect()
}

fn string(value: &Value, names: &[&str]) -> Option<String> {
    object_at(value, names).and_then(|value| match value {
        Value::String(value) => Some(value.clone()),
        Value::Number(value) => Some(value.to_string()),
        Value::Object(_) => string(value, &["name", "id", "value"]),
        _ => None,
    })
}

fn number(value: &Value, names: &[&str]) -> Option<f64> {
    object_at(value, names)
        .and_then(|value| value.as_f64().or_else(|| value.as_str()?.parse().ok()))
}

fn integer(value: &Value, names: &[&str]) -> Option<u64> {
    object_at(value, names)
        .and_then(|value| value.as_u64().or_else(|| value.as_str()?.parse().ok()))
}

fn boolean(value: &Value, names: &[&str]) -> Option<bool> {
    object_at(value, names)
        .and_then(|value| value.as_bool().or_else(|| value.as_str()?.parse().ok()))
}

fn timestamp(value: &Value) -> String {
    string(value, &["timestamp", "created_at", "createdAt", "time"]).unwrap_or_default()
}

fn identity(value: &Value, names: &[&str], fallback: &[&str]) -> String {
    string(value, names).unwrap_or_else(|| {
        let mut hasher = DefaultHasher::new();
        fallback.hash(&mut hasher);
        format!("generated-{:016x}", hasher.finish())
    })
}

fn status_category(status: &str, detail: Option<&str>, archived: bool) -> StatusCategory {
    if archived {
        return StatusCategory::Completed;
    }
    let status = status.to_ascii_lowercase();
    let detail = detail.map(str::to_ascii_lowercase);
    match (status.as_str(), detail.as_deref()) {
        ("running", Some("waiting_for_user")) => StatusCategory::Waiting,
        ("running", Some("waiting_for_approval")) => StatusCategory::WaitingApproval,
        ("running", Some("finished")) | ("exit", _) => StatusCategory::Completed,
        ("suspended", Some("inactivity" | "user_request")) => StatusCategory::Sleeping,
        ("suspended", Some("error")) | ("error", _) | ("failed" | "errored", _) => {
            StatusCategory::Failed
        }
        (
            "suspended",
            Some(
                "usage_limit_exceeded"
                | "out_of_credits"
                | "out_of_quota"
                | "no_quota_allocation"
                | "payment_declined"
                | "org_usage_limit_exceeded"
                | "user_usage_limit_exceeded"
                | "total_session_limit_exceeded",
            ),
        ) => StatusCategory::Suspended,
        ("new" | "claimed" | "running" | "working" | "active" | "resumed" | "resuming", _) => {
            StatusCategory::Active
        }
        ("waiting" | "blocked" | "pending", _) => StatusCategory::Waiting,
        ("sleeping" | "sleep", _) => StatusCategory::Sleeping,
        ("finished" | "completed" | "done" | "terminated" | "archived", _) => {
            StatusCategory::Completed
        }
        _ => StatusCategory::Unknown,
    }
}

fn bounded(mut value: String, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value;
    }
    let mut end = max_bytes;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value.truncate(end);
    value.push('…');
    value
}

fn external_url(url: &str) -> Option<String> {
    let url = url.trim();
    (url.len() <= MAX_DETAIL_BYTES && (url.starts_with("https://") || url.starts_with("http://")))
        .then(|| url.to_owned())
}

fn integration_url(url: &str) -> Option<String> {
    let url = url.trim();
    if url.starts_with("/settings/")
        && url.len() <= MAX_DETAIL_BYTES
        && !url.contains("..")
        && !url.chars().any(char::is_control)
    {
        return Some(format!("https://app.devin.ai{url}"));
    }
    external_url(url)
}

#[cfg(test)]
mod tests {
    use super::StatusCategory;

    #[test]
    fn slack_handoff_messages_hide_agent_scaffolding_and_link_their_attachments() {
        let payload = serde_json::json!({
            "items": [{
                "event_id": "event-slack",
                "source": "user",
                "created_at": 1786880434,
                "message": concat!(
                    "SYSTEM:\n",
                    "<latest_message>\n",
                    "mac (U07JBLL8DT8) [ts=1786880434.283959]: @Devin why is the image missing?\n",
                    "</latest_message>\n\n",
                    "=== BEGIN THREAD HISTORY ===\n",
                    "mac (U07JBLL8DT8) [ts=1786880434.283959]: @Devin why is the image missing?\n",
                    "ATTACHMENT:\"https://app.devin.ai/attachments/535f062f-2cce-47f1-88e9-ce4872a4bc98/Image%20from%20iOS.jpg\"\n",
                    "=== END THREAD HISTORY ===\n",
                    "Channel ID: C0BPUC3E98R\n\n",
                    "The <latest_message> is the message that you should use to guide your goals."
                )
            }]
        });

        let (messages, _) = super::messages(&payload).unwrap();

        assert_eq!(messages[0].text, "why is the image missing?");
        assert_eq!(
            messages[0].attachment_ids,
            ["535f062f-2cce-47f1-88e9-ce4872a4bc98"]
        );
    }

    #[test]
    fn internal_prompt_scaffolding_is_not_a_conversation_message() {
        let payload = serde_json::json!({
            "items": [
                {
                    "event_id": "system-message",
                    "source": "user",
                    "created_at": 1,
                    "message": concat!(
                        "You are working in the *example/project repository*.\n",
                        "*Objective*\nFix the app.\n",
                        "Branch and PR requirements\n",
                        "For every fix or feature you implement: open a PR."
                    )
                },
                {
                    "event_id": "user-message",
                    "source": "user",
                    "created_at": 2,
                    "message": "Please continue with the focused fix."
                },
                {
                    "event_id": "devin-message",
                    "source": "devin",
                    "created_at": 3,
                    "message": "I am checking the failing path."
                }
            ]
        });

        let (messages, _) = super::messages(&payload).unwrap();

        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].text, "Please continue with the focused fix.");
        assert_eq!(messages[1].text, "I am checking the failing path.");
    }

    #[test]
    fn internal_prompt_scaffolding_is_not_session_preview_text() {
        let detail = super::detail(&serde_json::json!({
            "session_id": "session-1",
            "prompt": concat!(
                "You are working in the *example/project repository*.\n",
                "*Objective*\nFix the app.\n",
                "Branch and PR requirements\nOpen a PR."
            )
        }))
        .unwrap();

        assert!(detail.summary.prompt.is_none());
    }

    #[test]
    fn a_single_session_insight_response_is_not_dropped() {
        let insights = super::insights(&serde_json::json!({
            "session_id": "session-1",
            "status": "complete",
            "analysis": "The prompt was well scoped.",
            "message_count": 4
        }));

        assert_eq!(insights.len(), 1);
        assert_eq!(insights[0].session_id, "session-1");
        assert_eq!(insights[0].message_count, Some(4));
    }

    #[test]
    fn single_mcp_resource_details_are_not_dropped() {
        assert_eq!(
            super::knowledge_detail(&serde_json::json!({
                "note": {"note_id":"note-1","name":"Testing","content":"Full note"}
            }))
            .unwrap()
            .content,
            "Full note"
        );
        assert_eq!(
            super::playbook_detail(&serde_json::json!({
                "playbook": {"playbook_id":"playbook-1","title":"Verify","content":"Full playbook"}
            }))
            .unwrap()
            .content,
            "Full playbook"
        );
        assert_eq!(
            super::schedule_detail(&serde_json::json!({
                "schedule": {"schedule_id":"schedule-1","name":"Nightly","prompt":"Full prompt"}
            }))
            .unwrap()
            .prompt,
            "Full prompt"
        );
        assert_eq!(
            super::knowledge_suggestion_detail(&serde_json::json!({
                "suggestion": {"suggestion_id":"suggestion-1","title":"Suggestion","content":"Full suggestion"}
            }))
            .unwrap()
            .content,
            "Full suggestion"
        );
        assert!(
            super::automation_detail(&serde_json::json!({
                "automation_id":"automation-1",
                "name":"Triage",
                "triggers":[{"type":"github.issue_opened"}]
            }))
            .unwrap()
            .configuration["triggers"]
                .is_array()
        );
        assert_eq!(
            super::build_detail(&serde_json::json!({
                "build_id":"build-1",
                "status":"succeeded",
                "snapshot_id":"snapshot-1"
            }))
            .unwrap()
            .configuration["snapshot_id"],
            "snapshot-1"
        );
    }

    #[test]
    fn current_event_insight_and_resource_fixtures_preserve_supported_fields() {
        let event: serde_json::Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/devin/mcp_event_page.json"
        ))
        .unwrap();
        let (events, cursor) = super::activity(&event).unwrap();
        assert_eq!(cursor.as_deref(), Some("event-cursor-2"));
        assert_eq!(events[0].id, "event-sanitized-1");
        assert_eq!(
            events[0].pull_request_url.as_deref(),
            Some("https://github.com/example/project/pull/1")
        );
        assert_eq!(
            events[0].child_session_id.as_deref(),
            Some("session-sanitized-child")
        );

        let insight: serde_json::Value =
            serde_json::from_str(include_str!("../../tests/fixtures/devin/v3_insight.json"))
                .unwrap();
        assert_eq!(super::insights(&insight)[0].message_count, Some(7));

        let resources: serde_json::Value =
            serde_json::from_str(include_str!("../../tests/fixtures/devin/v3_resources.json"))
                .unwrap();
        assert_eq!(super::repositories(&resources)[0].name, "example/project");
        assert!(super::repositories(&resources)[0].indexed);
        assert_eq!(
            super::knowledge(&resources)[0].folder.as_deref(),
            Some("Engineering")
        );
        assert_eq!(super::playbooks(&resources)[0].title, "Fix and verify");
        assert_eq!(super::schedules(&resources)[0].cadence, "0 2 * * *");
        assert_eq!(super::automations(&resources)[0].title, "Issue triage");
        assert_eq!(super::secrets(&resources)[0].name, "PACKAGE_TOKEN");
        assert_eq!(
            super::blueprints(&resources)[0].repository.as_deref(),
            Some("example/project")
        );
        assert!(super::builds(&resources)[0].pinned);
    }

    #[test]
    fn documented_v3_status_pairs_have_stable_semantics() {
        let cases = [
            ("new", None, false, StatusCategory::Active),
            ("claimed", None, false, StatusCategory::Active),
            ("running", Some("working"), false, StatusCategory::Active),
            (
                "running",
                Some("waiting_for_user"),
                false,
                StatusCategory::Waiting,
            ),
            (
                "running",
                Some("waiting_for_approval"),
                false,
                StatusCategory::WaitingApproval,
            ),
            (
                "running",
                Some("finished"),
                false,
                StatusCategory::Completed,
            ),
            ("exit", None, false, StatusCategory::Completed),
            ("error", None, false, StatusCategory::Failed),
            ("resuming", None, false, StatusCategory::Active),
            (
                "suspended",
                Some("inactivity"),
                false,
                StatusCategory::Sleeping,
            ),
            (
                "suspended",
                Some("user_request"),
                false,
                StatusCategory::Sleeping,
            ),
            (
                "suspended",
                Some("usage_limit_exceeded"),
                false,
                StatusCategory::Suspended,
            ),
            (
                "suspended",
                Some("out_of_credits"),
                false,
                StatusCategory::Suspended,
            ),
            (
                "suspended",
                Some("out_of_quota"),
                false,
                StatusCategory::Suspended,
            ),
            (
                "suspended",
                Some("no_quota_allocation"),
                false,
                StatusCategory::Suspended,
            ),
            (
                "suspended",
                Some("payment_declined"),
                false,
                StatusCategory::Suspended,
            ),
            (
                "suspended",
                Some("org_usage_limit_exceeded"),
                false,
                StatusCategory::Suspended,
            ),
            (
                "suspended",
                Some("user_usage_limit_exceeded"),
                false,
                StatusCategory::Suspended,
            ),
            (
                "suspended",
                Some("total_session_limit_exceeded"),
                false,
                StatusCategory::Suspended,
            ),
            ("suspended", Some("error"), false, StatusCategory::Failed),
            (
                "future",
                Some("future_detail"),
                false,
                StatusCategory::Unknown,
            ),
            ("running", Some("working"), true, StatusCategory::Completed),
        ];

        for (status, detail, archived, expected) in cases {
            assert_eq!(
                super::status_category(status, detail, archived),
                expected,
                "{status} + {detail:?} + archived={archived}"
            );
        }
    }

    #[test]
    fn current_v3_fixtures_preserve_pagination_messages_prs_children_and_metadata() {
        let sessions: serde_json::Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/devin/v3_session_page.json"
        ))
        .unwrap();
        let messages: serde_json::Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/devin/v3_messages_page.json"
        ))
        .unwrap();

        let (summaries, cursor, total, has_next) = super::sessions(&sessions).unwrap();
        let detail = super::detail(&sessions["items"][0]).unwrap();
        let (messages, message_cursor) = super::messages(&messages).unwrap();

        assert_eq!(summaries[0].category, StatusCategory::Waiting);
        assert_eq!(cursor.as_deref(), Some("cursor-fixture"));
        assert_eq!(total, Some(1));
        assert!(has_next);
        assert_eq!(message_cursor.as_deref(), Some("message-cursor-fixture"));
        assert_eq!(messages[0].role, "devin");
        assert_eq!(detail.pull_requests.len(), 2);
        assert_eq!(detail.pull_requests[0].status.as_deref(), Some("open"));
        assert_eq!(detail.children.len(), 2);
        assert_eq!(detail.summary.devin_mode.as_deref(), Some("fast"));
        assert_eq!(
            detail.summary.session_category.as_deref(),
            Some("bug_fixing")
        );
        assert_eq!(
            detail.summary.playbook_id.as_deref(),
            Some("playbook-fixture")
        );
        assert_eq!(
            detail.usage.as_ref().and_then(|usage| usage.acus),
            Some(3.5)
        );
        assert!(detail.summary.structured_output.is_some());
    }

    #[test]
    fn null_structured_output_is_not_presented_as_session_content() {
        let detail = super::detail(&serde_json::json!({
            "session_id": "session-1",
            "structured_output": null
        }))
        .unwrap();

        assert!(detail.summary.structured_output.is_none());
    }

    #[test]
    fn search_normalization_preserves_upstream_identity_status_and_origin() {
        let result = serde_json::json!({
            "structuredContent": {
                "sessions": [{
                    "session_id": "session-1",
                    "title": "Fix the race",
                    "status": "working",
                    "status_detail": "Running tests",
                    "origin": "slack",
                    "repo": "editur/editur",
                    "created_at": "2026-08-15T12:00:00Z",
                    "tags": ["triage"]
                }],
                "next_cursor": "next-page"
            }
        });

        let (sessions, cursor, total, has_next) =
            super::sessions(&super::tool_payload(result).unwrap()).unwrap();

        assert_eq!(
            (
                sessions[0].id.as_str(),
                sessions[0].origin.as_deref(),
                cursor.as_deref()
            ),
            ("session-1", Some("slack"), Some("next-page"))
        );
        assert_eq!(total, None);
        assert!(has_next);
    }

    #[test]
    fn live_mcp_session_search_envelope_normalizes() {
        let result = serde_json::json!({
            "structuredContent": {
                "result": concat!(
                    "More results available. Pass after=cursor-live to fetch the next page.\n\n",
                    "- session-live | Fix live MCP parsing | status=running (working) | ",
                    "created=2026-08-16T19:30:00Z\n",
                    "- session-archived | Old task | follow-up | ",
                    "status=suspended (user_request) [archived] | ",
                    "created=2026-08-13T19:30:00Z\n\n",
                    "Total: 97"
                )
            }
        });

        let (sessions, cursor, total, has_next) =
            super::sessions(&super::tool_payload(result).unwrap()).unwrap();

        assert_eq!(sessions[0].id, "session-live");
        assert_eq!(sessions[0].title, "Fix live MCP parsing");
        assert_eq!(sessions[1].title, "Old task | follow-up");
        assert_eq!(sessions[1].status, "suspended");
        assert_eq!(sessions[1].status_detail.as_deref(), Some("user_request"));
        assert!(sessions[1].archived);
        assert_eq!(sessions[1].category, StatusCategory::Completed);
        assert_eq!(cursor.as_deref(), Some("cursor-live"));
        assert_eq!(total, Some(97));
        assert!(has_next);
    }

    #[test]
    fn live_mcp_session_detail_normalizes() {
        let payload = serde_json::Value::String(
            concat!(
                "Session session-live:\n",
                "  session_id: session-live\n",
                "  title: Fix live MCP parsing\n",
                "  status: running\n",
                "  status_detail: working\n",
                "  url: https://app.devin.ai/sessions/session-live\n",
                "  acus_consumed: 1.5\n",
                "  pull_requests (1):\n",
                "    - https://github.com/editur/editur/pull/1 [open]"
            )
            .into(),
        );

        let detail = super::detail(&payload).unwrap();

        assert_eq!(detail.summary.id, "session-live");
        assert_eq!(detail.summary.title, "Fix live MCP parsing");
        assert_eq!(detail.pull_requests[0].status.as_deref(), Some("open"));
        assert_eq!(detail.usage.unwrap().acus, Some(1.5));
    }

    #[test]
    fn live_mcp_event_page_normalizes() {
        let payload = serde_json::Value::String(
            concat!(
                "More results available. Pass after=event-cursor to fetch the next page.\n\n",
                "[event-live] 2026-08-16 19:30:00 UTC <<< shell_command (shell): Ran tests\n",
                "[event-next] 2026-08-16 19:31:00 UTC <<< user_message (message): Continue\n\n",
                "Total: 2"
            )
            .into(),
        );

        let (activity, cursor) = super::activity(&payload).unwrap();

        assert_eq!(activity[0].id, "event-live");
        assert_eq!(activity[0].category, "shell");
        assert_eq!(activity[0].summary, "Ran tests");
        assert_eq!(cursor.as_deref(), Some("event-cursor"));
    }

    #[test]
    fn live_mcp_json_with_a_trailing_note_normalizes() {
        let result = serde_json::json!({
            "structuredContent": {
                "result": concat!(
                    "{\"integrations\":[{\"name\":\"github\",",
                    "\"display_name\":\"GitHub\",\"is_installed\":true,",
                    "\"settings_url\":\"/settings/integrations/github\"}],",
                    "\"mcp_servers\":[{\"name\":\"Sentry\",\"slug\":\"sentry\",",
                    "\"is_installed\":false,",
                    "\"setup_url\":\"/settings/mcp-marketplace/setup/sentry\"}]}\n\n",
                    "Note: use the settings URL to configure this integration."
                )
            }
        });

        let payload = super::tool_payload(result).unwrap();
        let integrations = super::integrations(&payload);

        assert_eq!(integrations.len(), 2);
        assert_eq!(integrations[0].name, "GitHub");
        assert!(integrations[0].installed);
        assert_eq!(
            integrations[0].url.as_deref(),
            Some("https://app.devin.ai/settings/integrations/github")
        );
        assert_eq!(integrations[1].id, "sentry");
        assert_eq!(integrations[1].kind, "mcp");
    }

    #[test]
    fn attachment_links_keep_required_signatures_but_reject_non_web_schemes() {
        let signed = serde_json::json!({"attachments": [{
            "id": "one",
            "url": "https://example.test/file?signature=required"
        }]});
        let unsafe_url = serde_json::json!({"attachments": [{
            "id": "two",
            "url": "javascript:alert(1)"
        }]});

        assert_eq!(
            super::attachments(&signed)[0].url.as_deref(),
            Some("https://example.test/file?signature=required")
        );
        assert!(super::attachments(&unsafe_url)[0].url.is_none());
    }
}
