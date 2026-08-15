use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
};

use serde_json::Value;

use super::state::{
    Activity, Attachment, ChildSession, DevinMessage, PullRequest, SessionDetail, SessionSummary,
    StatusCategory, Usage,
};

const MAX_ITEMS: usize = 512;
const MAX_TEXT_BYTES: usize = 64 * 1024;
const MAX_DETAIL_BYTES: usize = 8 * 1024;

pub(super) fn tool_payload(result: Value) -> Result<Value, String> {
    if result.get("isError").and_then(Value::as_bool) == Some(true) {
        return Err("Devin could not complete the request".into());
    }
    if let Some(structured) = result
        .get("structuredContent")
        .or_else(|| result.get("structured_content"))
    {
        return Ok(structured.clone());
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
        return serde_json::from_str(text)
            .map_err(|_| "Devin returned an unsupported tool response".into());
    }
    Ok(result)
}

pub(super) fn sessions(value: &Value) -> Result<(Vec<SessionSummary>, Option<String>), String> {
    let items = array(value, &["sessions", "results", "items"])
        .ok_or_else(|| "Devin session search did not return a session list".to_owned())?;
    Ok((
        items.iter().take(MAX_ITEMS).filter_map(session).collect(),
        string(value, &["next_cursor", "nextCursor", "cursor"]),
    ))
}

pub(super) fn detail(value: &Value) -> Result<SessionDetail, String> {
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
    let children = array(root, &["child_sessions", "childSessions", "children"])
        .into_iter()
        .flatten()
        .take(MAX_ITEMS)
        .filter_map(child_session)
        .collect();
    let usage_root = object_at(root, &["usage"]);
    let usage = usage_root.map(|usage| Usage {
        acus: number(usage, &["acus", "acu", "used"]),
        limit: number(usage, &["limit", "acu_limit", "acuLimit"]),
    });
    Ok(SessionDetail {
        summary,
        attachments,
        pull_requests,
        children,
        usage,
    })
}

pub(super) fn created_session(value: &Value) -> Result<SessionSummary, String> {
    array(value, &["sessions", "results", "items"])
        .and_then(|sessions| sessions.first())
        .and_then(session)
        .or_else(|| object_at(value, &["session", "result"]).and_then(session))
        .or_else(|| session(value))
        .ok_or_else(|| "Devin session creation did not return a session identity".into())
}

pub(super) fn messages(value: &Value) -> Result<(Vec<DevinMessage>, Option<String>), String> {
    let items = array(value, &["messages", "results", "items"])
        .ok_or_else(|| "Devin did not return a message list".to_owned())?;
    let messages = items
        .iter()
        .take(MAX_ITEMS)
        .filter_map(|item| {
            let role =
                string(item, &["role", "author", "type"]).unwrap_or_else(|| "unknown".into());
            let text = string(item, &["text", "message", "content"])?;
            let timestamp = timestamp(item);
            let id = identity(
                item,
                &["message_id", "messageId", "event_id", "eventId", "id"],
                &[&timestamp, &role, &text],
            );
            let attachment_ids = array(item, &["attachment_ids", "attachmentIds", "attachments"])
                .into_iter()
                .flatten()
                .filter_map(|attachment| {
                    attachment
                        .as_str()
                        .map(str::to_owned)
                        .or_else(|| string(attachment, &["id", "attachment_id"]))
                })
                .take(MAX_ITEMS)
                .collect();
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
        string(value, &["next_cursor", "nextCursor", "cursor"]),
    ))
}

pub(super) fn activity(value: &Value) -> Result<(Vec<Activity>, Option<String>), String> {
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
        string(value, &["next_cursor", "nextCursor", "cursor"]),
    ))
}

pub(super) fn attachments(value: &Value) -> Vec<Attachment> {
    array(value, &["attachments", "results", "items"])
        .into_iter()
        .flatten()
        .take(MAX_ITEMS)
        .filter_map(attachment)
        .collect()
}

fn session(value: &Value) -> Option<SessionSummary> {
    let id = string(value, &["session_id", "sessionId", "id"])?;
    let status = string(value, &["status", "state"]).unwrap_or_else(|| "unknown".into());
    Some(SessionSummary {
        id,
        title: string(value, &["title", "name"]).unwrap_or_else(|| "Untitled session".into()),
        prompt: string(
            value,
            &["prompt", "initial_prompt", "initialPrompt", "task"],
        )
        .map(|value| bounded(value, MAX_DETAIL_BYTES)),
        category: status_category(&status),
        status,
        status_detail: string(value, &["status_detail", "statusDetail"]),
        origin: string(value, &["origin", "source"]),
        repository: string(value, &["repository", "repo", "repo_name", "repoName"]),
        created_at: string(value, &["created_at", "createdAt", "created"]),
        updated_at: string(value, &["updated_at", "updatedAt", "updated"]),
        archived: boolean(value, &["archived", "is_archived", "isArchived"]).unwrap_or(false),
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
    let url = external_url(&string(value, &["url", "html_url", "htmlUrl"])?)?;
    Some(PullRequest {
        id: string(value, &["id", "number"]).unwrap_or_else(|| url.clone()),
        title: string(value, &["title", "name"]).unwrap_or_else(|| "Pull request".into()),
        url,
        status: string(value, &["status", "state"]),
    })
}

fn child_session(value: &Value) -> Option<ChildSession> {
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

fn status_category(status: &str) -> StatusCategory {
    match status.to_ascii_lowercase().as_str() {
        "running" | "working" | "active" | "resumed" => StatusCategory::Active,
        "waiting" | "blocked" | "pending" => StatusCategory::Waiting,
        "sleeping" | "sleep" => StatusCategory::Sleeping,
        "finished" | "completed" | "done" | "terminated" | "archived" => StatusCategory::Completed,
        "failed" | "error" | "errored" => StatusCategory::Failed,
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

#[cfg(test)]
mod tests {
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

        let (sessions, cursor) = super::sessions(&super::tool_payload(result).unwrap()).unwrap();

        assert_eq!(
            (
                sessions[0].id.as_str(),
                sessions[0].origin.as_deref(),
                cursor.as_deref()
            ),
            ("session-1", Some("slack"), Some("next-page"))
        );
    }

    #[test]
    fn create_normalization_returns_the_created_session() {
        let value = serde_json::json!({
            "sessions": [{"session_id": "new", "status": "running"}]
        });

        assert_eq!(super::created_session(&value).unwrap().id, "new");
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
