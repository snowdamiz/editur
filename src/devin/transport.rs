use std::{collections::HashMap, fmt, path::Path};

#[cfg(feature = "network")]
use std::time::Duration;

use serde_json::{Value, json};

use super::credentials::Credentials;

pub(super) const ENDPOINT: &str = "https://mcp.devin.ai/mcp";
const ATTACHMENTS_ENDPOINT: &str = "https://api.devin.ai/v1/attachments";
pub(super) const MAX_RESPONSE_BYTES: usize = 4 * 1024 * 1024;

#[derive(Clone, Eq, PartialEq)]
#[cfg_attr(not(feature = "network"), allow(dead_code))]
pub(super) enum TransportError {
    CredentialsMissing,
    Authentication,
    Forbidden,
    RateLimited(Option<u64>),
    Offline,
    Oversized,
    Protocol(String),
    Remote(String),
}

impl fmt::Debug for TransportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CredentialsMissing => formatter.write_str("CredentialsMissing"),
            Self::Authentication => formatter.write_str("Authentication"),
            Self::Forbidden => formatter.write_str("Forbidden"),
            Self::RateLimited(retry) => formatter.debug_tuple("RateLimited").field(retry).finish(),
            Self::Offline => formatter.write_str("Offline"),
            Self::Oversized => formatter.write_str("Oversized"),
            Self::Protocol(_) => formatter.write_str("Protocol([REDACTED])"),
            Self::Remote(_) => formatter.write_str("Remote([REDACTED])"),
        }
    }
}

pub(super) struct McpTransport {
    #[cfg(feature = "network")]
    endpoint: String,
    #[cfg(feature = "network")]
    attachments_endpoint: String,
    #[cfg(feature = "network")]
    credentials: Credentials,
    #[cfg(feature = "network")]
    session_id: Option<String>,
    next_id: u64,
    tools: HashMap<String, Value>,
    #[cfg(feature = "network")]
    agent: ureq::Agent,
}

#[derive(Clone, Copy)]
pub(super) enum InteractAction<'a> {
    Status,
    Messages {
        cursor: Option<&'a str>,
    },
    Attachments,
    SendMessage {
        message: &'a str,
        attachment_ids: &'a [String],
    },
    Sleep,
    Archive,
    Unarchive,
    Terminate,
}

impl McpTransport {
    pub(super) fn connect(credentials: Credentials) -> Result<Self, TransportError> {
        Self::connect_endpoints(credentials, ENDPOINT, ATTACHMENTS_ENDPOINT)
    }

    pub(super) fn connect_endpoint(
        credentials: Credentials,
        endpoint: &str,
    ) -> Result<Self, TransportError> {
        let attachments_endpoint = format!(
            "{}/v1/attachments",
            endpoint.strip_suffix("/mcp").unwrap_or(endpoint)
        );
        Self::connect_endpoints(credentials, endpoint, &attachments_endpoint)
    }

    fn connect_endpoints(
        credentials: Credentials,
        endpoint: &str,
        attachments_endpoint: &str,
    ) -> Result<Self, TransportError> {
        #[cfg(not(feature = "network"))]
        let _ = attachments_endpoint;
        #[cfg(feature = "network")]
        let agent = ureq::Agent::config_builder()
            .user_agent(format!("editur/{}", env!("CARGO_PKG_VERSION")))
            .timeout_connect(Some(Duration::from_secs(15)))
            .timeout_global(Some(Duration::from_secs(45)))
            .http_status_as_error(false)
            .build()
            .into();
        #[cfg(not(feature = "network"))]
        {
            let _ = endpoint;
            drop(credentials);
        }
        let mut transport = Self {
            #[cfg(feature = "network")]
            endpoint: endpoint.into(),
            #[cfg(feature = "network")]
            attachments_endpoint: attachments_endpoint.into(),
            #[cfg(feature = "network")]
            credentials,
            #[cfg(feature = "network")]
            session_id: None,
            next_id: 1,
            tools: HashMap::new(),
            #[cfg(feature = "network")]
            agent,
        };
        let initialized = transport.rpc(
            "initialize",
            json!({
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": {
                    "name": "Editur",
                    "version": env!("CARGO_PKG_VERSION")
                }
            }),
        )?;
        if initialized
            .get("protocolVersion")
            .and_then(Value::as_str)
            .is_none()
        {
            return Err(TransportError::Protocol(
                "initialize response omitted protocolVersion".into(),
            ));
        }
        transport.notify("notifications/initialized", json!({}))?;
        let listed = transport.rpc("tools/list", json!({}))?;
        transport.tools = listed
            .get("tools")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|tool| Some((tool.get("name")?.as_str()?.to_owned(), tool.clone())))
            .collect();
        for required in [
            "devin_session_search",
            "devin_session_create",
            "devin_session_interact",
            "devin_session_events",
            "devin_session_gather",
        ] {
            if !transport.tools.contains_key(required) {
                return Err(TransportError::Protocol(format!(
                    "required tool {required} was not advertised"
                )));
            }
        }
        Ok(transport)
    }

    #[cfg(feature = "network")]
    pub(super) fn upload_attachment(&mut self, path: &Path) -> Result<String, TransportError> {
        let form = ureq::unversioned::multipart::Form::new()
            .file("file", path)
            .map_err(|error| TransportError::Protocol(error.to_string()))?;
        let authorization = format!("Bearer {}", self.credentials.api_key());
        let mut request = self
            .agent
            .post(&self.attachments_endpoint)
            .header("Authorization", &authorization)
            .header("Accept", "application/json");
        if let Some(org_id) = self.credentials.org_id() {
            request = request.header("X-Org-Id", org_id);
        }
        let mut response = request.send(form).map_err(|_| TransportError::Offline)?;
        let status = response.status().as_u16();
        match status {
            200..=299 => {}
            401 => return Err(TransportError::Authentication),
            403 => return Err(TransportError::Forbidden),
            429 => return Err(TransportError::RateLimited(None)),
            500..=599 => return Err(TransportError::Offline),
            _ => return Err(TransportError::Remote(format!("HTTP {status}"))),
        }
        let bytes = response
            .body_mut()
            .with_config()
            .limit((MAX_RESPONSE_BYTES + 1) as u64)
            .read_to_vec()
            .map_err(|_| TransportError::Offline)?;
        if bytes.len() > MAX_RESPONSE_BYTES {
            return Err(TransportError::Oversized);
        }
        let url: String = serde_json::from_slice(&bytes)
            .map_err(|error| TransportError::Protocol(error.to_string()))?;
        if !url.starts_with("https://")
            || url
                .chars()
                .any(|character| character.is_control() || character == '"')
        {
            return Err(TransportError::Protocol(
                "attachment upload returned an invalid URL".into(),
            ));
        }
        Ok(url)
    }

    #[cfg(not(feature = "network"))]
    pub(super) fn upload_attachment(&mut self, _path: &Path) -> Result<String, TransportError> {
        Err(TransportError::Offline)
    }

    pub(super) fn search(&mut self, cursor: Option<&str>) -> Result<Value, TransportError> {
        let mut arguments = serde_json::Map::new();
        insert_if_property(
            self.schema("devin_session_search"),
            &mut arguments,
            &["limit", "page_size", "pageSize"],
            json!(100),
        );
        if let Some(cursor) = cursor {
            insert_if_property(
                self.schema("devin_session_search"),
                &mut arguments,
                &["cursor", "page_cursor", "pageCursor"],
                json!(cursor),
            );
        }
        self.call_tool("devin_session_search", Value::Object(arguments))
    }

    pub(super) fn create(
        &mut self,
        repository: &str,
        prompt: &str,
    ) -> Result<Value, TransportError> {
        let schema = self.schema("devin_session_create");
        let mut session = serde_json::Map::new();
        insert_named(
            &mut session,
            schema,
            &["prompt", "message", "task"],
            json!(prompt),
        );
        insert_named(
            &mut session,
            schema,
            &["repository", "repo", "repo_name", "repoName"],
            json!(repository),
        );
        insert_named(&mut session, schema, &["tags"], json!(["editur"]));
        let wrapper = property_name(schema, &["sessions", "tasks", "requests"]);
        let arguments = if let Some(wrapper) = wrapper {
            json!({ wrapper: [Value::Object(session)] })
        } else {
            Value::Object(session)
        };
        self.call_tool("devin_session_create", arguments)
    }

    pub(super) fn interact(
        &mut self,
        session_id: &str,
        action: InteractAction<'_>,
    ) -> Result<Value, TransportError> {
        let schema = self.schema("devin_session_interact");
        let mut arguments = serde_json::Map::new();
        insert_named(
            &mut arguments,
            schema,
            &["session_id", "sessionId", "id"],
            json!(session_id),
        );
        let (semantic, aliases) = match action {
            InteractAction::Status => ("get_status", &["get_status", "status", "get"] as &[_]),
            InteractAction::Messages { .. } => (
                "get_messages",
                &["get_messages", "messages", "list_messages"] as &[_],
            ),
            InteractAction::Attachments => (
                "get_attachments",
                &["get_attachments", "attachments", "list_attachments"] as &[_],
            ),
            InteractAction::SendMessage { .. } => {
                ("send_message", &["send_message", "message", "send"] as &[_])
            }
            InteractAction::Sleep => ("sleep", &["sleep"] as &[_]),
            InteractAction::Archive => ("archive", &["archive"] as &[_]),
            InteractAction::Unarchive => ("unarchive", &["unarchive"] as &[_]),
            InteractAction::Terminate => ("terminate", &["terminate", "stop"] as &[_]),
        };
        let action_value = enum_choice(schema, &["action", "operation"], aliases)
            .unwrap_or_else(|| semantic.to_owned());
        insert_named(
            &mut arguments,
            schema,
            &["action", "operation"],
            json!(action_value),
        );
        match action {
            InteractAction::Messages {
                cursor: Some(cursor),
            } => insert_if_property(
                schema,
                &mut arguments,
                &["cursor", "page_cursor", "pageCursor"],
                json!(cursor),
            ),
            InteractAction::SendMessage {
                message,
                attachment_ids,
            } => {
                insert_named(
                    &mut arguments,
                    schema,
                    &["message", "text", "prompt"],
                    json!(message),
                );
                if !attachment_ids.is_empty() {
                    insert_if_property(
                        schema,
                        &mut arguments,
                        &["attachment_ids", "attachmentIds", "attachments"],
                        json!(attachment_ids),
                    );
                }
            }
            _ => {}
        }
        self.call_tool("devin_session_interact", Value::Object(arguments))
    }

    pub(super) fn events(
        &mut self,
        session_id: &str,
        cursor: Option<&str>,
    ) -> Result<Value, TransportError> {
        let schema = self.schema("devin_session_events");
        let mut arguments = serde_json::Map::new();
        insert_named(
            &mut arguments,
            schema,
            &["session_id", "sessionId", "id"],
            json!(session_id),
        );
        if property_name(schema, &["action", "operation"]).is_some() {
            let action = enum_choice(
                schema,
                &["action", "operation"],
                &["list", "list_events", "summaries"],
            )
            .unwrap_or_else(|| "list".into());
            insert_named(
                &mut arguments,
                schema,
                &["action", "operation"],
                json!(action),
            );
        }
        insert_if_property(
            schema,
            &mut arguments,
            &["limit", "page_size", "pageSize"],
            json!(100),
        );
        if let Some(cursor) = cursor {
            insert_if_property(
                schema,
                &mut arguments,
                &["cursor", "page_cursor", "pageCursor", "after"],
                json!(cursor),
            );
        }
        self.call_tool("devin_session_events", Value::Object(arguments))
    }

    fn schema(&self, tool: &str) -> &Value {
        self.tools
            .get(tool)
            .and_then(|tool| tool.get("inputSchema").or_else(|| tool.get("input_schema")))
            .unwrap_or(&Value::Null)
    }

    fn call_tool(&mut self, name: &str, arguments: Value) -> Result<Value, TransportError> {
        self.rpc(
            "tools/call",
            json!({
                "name": name,
                "arguments": arguments
            }),
        )
    }

    fn rpc(&mut self, method: &str, params: Value) -> Result<Value, TransportError> {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1).max(1);
        let body = serde_json::to_vec(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params
        }))
        .map_err(|error| TransportError::Protocol(error.to_string()))?;
        let (content_type, response) = self.post(&body, true)?;
        parse_response(&content_type, &response, id)
    }

    fn notify(&mut self, method: &str, params: Value) -> Result<(), TransportError> {
        let body = serde_json::to_vec(&json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params
        }))
        .map_err(|error| TransportError::Protocol(error.to_string()))?;
        self.post(&body, false).map(|_| ())
    }

    /// Downloads attachment bytes from a signed URL. The URL carries its own
    /// authorization, so no credentials are attached — sending the API key to
    /// an arbitrary storage host would leak it.
    #[cfg(feature = "network")]
    pub(super) fn fetch_attachment(&mut self, url: &str) -> Result<Vec<u8>, TransportError> {
        if !url.starts_with("https://") {
            return Err(TransportError::Protocol(
                "attachment downloads require https".into(),
            ));
        }
        let mut response = self
            .agent
            .get(url)
            .header("Accept", "*/*")
            .call()
            .map_err(|_| TransportError::Offline)?;
        let status = response.status().as_u16();
        match status {
            200..=299 => {}
            401 => return Err(TransportError::Authentication),
            403 => return Err(TransportError::Forbidden),
            429 => return Err(TransportError::RateLimited(None)),
            500..=599 => return Err(TransportError::Offline),
            _ => return Err(TransportError::Remote(format!("HTTP {status}"))),
        }
        let bytes = response
            .body_mut()
            .with_config()
            .limit((MAX_RESPONSE_BYTES + 1) as u64)
            .read_to_vec()
            .map_err(|_| TransportError::Offline)?;
        if bytes.len() > MAX_RESPONSE_BYTES {
            return Err(TransportError::Oversized);
        }
        Ok(bytes)
    }

    #[cfg(not(feature = "network"))]
    pub(super) fn fetch_attachment(&mut self, _url: &str) -> Result<Vec<u8>, TransportError> {
        Err(TransportError::Offline)
    }

    #[cfg(feature = "network")]
    fn post(
        &mut self,
        body: &[u8],
        expects_response: bool,
    ) -> Result<(String, Vec<u8>), TransportError> {
        let authorization = format!("Bearer {}", self.credentials.api_key());
        let mut request = self
            .agent
            .post(&self.endpoint)
            .header("Authorization", &authorization)
            .header("Accept", "application/json, text/event-stream")
            .header("Content-Type", "application/json")
            .header("MCP-Protocol-Version", "2025-06-18");
        if let Some(org_id) = self.credentials.org_id() {
            request = request.header("X-Org-Id", org_id);
        }
        if let Some(session_id) = self.session_id.as_deref() {
            request = request.header("Mcp-Session-Id", session_id);
        }
        let mut response = request.send(body).map_err(|_| TransportError::Offline)?;
        let status = response.status().as_u16();
        if let Some(session_id) = response
            .headers()
            .get("Mcp-Session-Id")
            .and_then(|value| value.to_str().ok())
        {
            self.session_id = Some(session_id.to_owned());
        }
        match status {
            200..=299 => {}
            401 => return Err(TransportError::Authentication),
            403 => return Err(TransportError::Forbidden),
            429 => {
                let retry_after = response
                    .headers()
                    .get("Retry-After")
                    .and_then(|value| value.to_str().ok())
                    .and_then(|value| value.parse().ok());
                return Err(TransportError::RateLimited(retry_after));
            }
            500..=599 => return Err(TransportError::Offline),
            _ => return Err(TransportError::Remote(format!("HTTP {status}"))),
        }
        let content_type = response
            .headers()
            .get("Content-Type")
            .and_then(|value| value.to_str().ok())
            .unwrap_or("application/json")
            .to_owned();
        if !expects_response || status == 202 || status == 204 {
            return Ok((content_type, Vec::new()));
        }
        let bytes = response
            .body_mut()
            .with_config()
            .limit((MAX_RESPONSE_BYTES + 1) as u64)
            .read_to_vec()
            .map_err(|_| TransportError::Offline)?;
        if bytes.len() > MAX_RESPONSE_BYTES {
            return Err(TransportError::Oversized);
        }
        Ok((content_type, bytes))
    }

    #[cfg(not(feature = "network"))]
    fn post(
        &mut self,
        _body: &[u8],
        _expects_response: bool,
    ) -> Result<(String, Vec<u8>), TransportError> {
        Err(TransportError::Offline)
    }
}

fn properties(schema: &Value) -> Option<&serde_json::Map<String, Value>> {
    schema.get("properties").and_then(Value::as_object)
}

fn property_name(schema: &Value, candidates: &[&str]) -> Option<String> {
    let properties = properties(schema)?;
    candidates
        .iter()
        .find(|candidate| properties.contains_key(**candidate))
        .map(|candidate| (*candidate).to_owned())
}

fn insert_named(
    target: &mut serde_json::Map<String, Value>,
    schema: &Value,
    candidates: &[&str],
    value: Value,
) {
    let name = property_name(schema, candidates).unwrap_or_else(|| candidates[0].to_owned());
    target.insert(name, value);
}

fn insert_if_property(
    schema: &Value,
    target: &mut serde_json::Map<String, Value>,
    candidates: &[&str],
    value: Value,
) {
    if let Some(name) = property_name(schema, candidates) {
        target.insert(name, value);
    }
}

fn enum_choice(schema: &Value, properties: &[&str], choices: &[&str]) -> Option<String> {
    let name = property_name(schema, properties)?;
    let values = schema
        .get("properties")?
        .get(name)?
        .get("enum")?
        .as_array()?;
    choices.iter().find_map(|choice| {
        values
            .iter()
            .filter_map(Value::as_str)
            .find(|value| normalized(value) == normalized(choice))
            .map(str::to_owned)
    })
}

fn normalized(value: &str) -> String {
    value
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

impl TransportError {
    pub(super) fn user_message(&self) -> &'static str {
        match self {
            Self::CredentialsMissing => "Connect to Devin to continue",
            Self::Authentication => "Devin authentication failed",
            Self::Forbidden => "This credential does not have permission for that Devin action",
            Self::RateLimited(_) => "Devin is rate limiting requests",
            Self::Offline => "Devin is unavailable; check your connection",
            Self::Oversized => "Devin returned more data than Editur can safely display",
            Self::Protocol(_) => "Devin returned an unsupported response",
            Self::Remote(_) => "Devin could not complete the request",
        }
    }

    pub(super) const fn retry_after_seconds(&self) -> Option<u64> {
        match self {
            Self::RateLimited(retry_after) => *retry_after,
            _ => None,
        }
    }
}

pub(super) fn parse_response(
    content_type: &str,
    bytes: &[u8],
    expected_id: u64,
) -> Result<Value, TransportError> {
    if bytes.len() > MAX_RESPONSE_BYTES {
        return Err(TransportError::Oversized);
    }
    let messages = if content_type
        .split(';')
        .next()
        .is_some_and(|value| value.trim().eq_ignore_ascii_case("text/event-stream"))
    {
        parse_sse(bytes)?
    } else {
        vec![
            serde_json::from_slice(bytes)
                .map_err(|error| TransportError::Protocol(error.to_string()))?,
        ]
    };
    let message = messages
        .into_iter()
        .find(|message| message.get("id").and_then(Value::as_u64) == Some(expected_id))
        .ok_or_else(|| TransportError::Protocol("response id did not match request".into()))?;
    if let Some(error) = message.get("error") {
        return Err(TransportError::Remote(
            error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("remote tool error")
                .chars()
                .take(256)
                .collect(),
        ));
    }
    message
        .get("result")
        .cloned()
        .ok_or_else(|| TransportError::Protocol("response did not contain a result".into()))
}

fn parse_sse(bytes: &[u8]) -> Result<Vec<Value>, TransportError> {
    let text =
        std::str::from_utf8(bytes).map_err(|error| TransportError::Protocol(error.to_string()))?;
    let mut messages = Vec::new();
    let mut data = String::new();
    for line in text.lines().chain(std::iter::once("")) {
        if line.is_empty() {
            if !data.is_empty() && data != "[DONE]" {
                messages.push(
                    serde_json::from_str(&data)
                        .map_err(|error| TransportError::Protocol(error.to_string()))?,
                );
            }
            data.clear();
        } else if let Some(value) = line.strip_prefix("data:") {
            if !data.is_empty() {
                data.push('\n');
            }
            data.push_str(value.trim_start());
        }
    }
    Ok(messages)
}

#[cfg(test)]
mod tests {
    #[test]
    fn streamable_http_parser_accepts_json_and_sse_without_logging_payloads() {
        let json = br#"{"jsonrpc":"2.0","id":7,"result":{"ok":true}}"#;
        let sse =
            b"event: message\ndata: {\"jsonrpc\":\"2.0\",\"id\":7,\"result\":{\"ok\":true}}\n\n";

        assert_eq!(
            super::parse_response("application/json", json, 7).unwrap(),
            super::parse_response("text/event-stream", sse, 7).unwrap()
        );
    }

    #[test]
    fn transport_errors_do_not_expose_remote_response_text() {
        let secret = "cog_remote-echoed-secret";
        let error = super::TransportError::Remote(secret.into());

        assert!(!format!("{error:?}").contains(secret));
    }

    #[test]
    fn malformed_and_oversized_responses_fail_before_entering_state() {
        assert!(matches!(
            super::parse_response("application/json", b"not json", 1),
            Err(super::TransportError::Protocol(_))
        ));
        assert_eq!(
            super::parse_response(
                "application/json",
                &vec![b'x'; super::MAX_RESPONSE_BYTES + 1],
                1,
            ),
            Err(super::TransportError::Oversized)
        );
    }
}
