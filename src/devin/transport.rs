use std::{collections::HashMap, fmt, path::Path};

#[cfg(feature = "network")]
use std::time::Duration;

#[cfg(feature = "network")]
use serde::Deserialize;
use serde_json::{Value, json};

use super::credentials::Credentials;
use super::state::{DevinCapabilities, DevinOrganization, SessionFilters};

pub(super) const ENDPOINT: &str = "https://mcp.devin.ai/mcp";
const API_ENDPOINT: &str = "https://api.devin.ai";
pub(super) const MAX_RESPONSE_BYTES: usize = 4 * 1024 * 1024;

#[derive(Clone, Eq, PartialEq)]
#[cfg_attr(not(feature = "network"), allow(dead_code))]
pub(super) enum TransportError {
    CredentialsMissing,
    OrganizationSelectionRequired(Vec<DevinOrganization>),
    Authentication,
    Forbidden,
    Unavailable,
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
            Self::OrganizationSelectionRequired(organizations) => formatter
                .debug_tuple("OrganizationSelectionRequired")
                .field(&organizations.len())
                .finish(),
            Self::Authentication => formatter.write_str("Authentication"),
            Self::Forbidden => formatter.write_str("Forbidden"),
            Self::Unavailable => formatter.write_str("Unavailable"),
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
    api_endpoint: String,
    #[cfg(feature = "network")]
    org_id: String,
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
    SendMessage {
        message: &'a str,
        attachment_ids: &'a [String],
    },
    Sleep,
    Archive,
    Unarchive,
    Terminate,
}

#[derive(Clone, Copy)]
pub(super) enum V3Method {
    Get,
    Post,
    Put,
    Patch,
    Delete,
}

#[derive(Clone, Copy)]
pub(super) enum ApiVersion {
    V3,
    V3Beta1,
}

impl McpTransport {
    pub(super) fn connect(credentials: Credentials) -> Result<Self, TransportError> {
        Self::connect_endpoints(credentials, ENDPOINT, API_ENDPOINT)
    }

    pub(super) fn connect_endpoint(
        credentials: Credentials,
        endpoint: &str,
    ) -> Result<Self, TransportError> {
        let api_endpoint = endpoint.strip_suffix("/mcp").unwrap_or(endpoint);
        Self::connect_endpoints(credentials, endpoint, api_endpoint)
    }

    fn connect_endpoints(
        credentials: Credentials,
        endpoint: &str,
        api_endpoint: &str,
    ) -> Result<Self, TransportError> {
        #[cfg(not(feature = "network"))]
        let _ = api_endpoint;
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
        #[cfg(feature = "network")]
        let org_id = credentials
            .org_id()
            .map(str::to_owned)
            .map(Ok)
            .unwrap_or_else(|| resolve_org_id(&agent, api_endpoint, &credentials))?;
        #[cfg(feature = "network")]
        if !safe_path_segment(&org_id) {
            return Err(TransportError::Protocol(
                "Devin returned an invalid organization ID".into(),
            ));
        }
        let mut transport = Self {
            #[cfg(feature = "network")]
            endpoint: endpoint.into(),
            #[cfg(feature = "network")]
            api_endpoint: api_endpoint.trim_end_matches('/').into(),
            #[cfg(feature = "network")]
            org_id,
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
        for required in ["devin_session_search", "devin_session_interact"] {
            if !transport.tools.contains_key(required) {
                return Err(TransportError::Protocol(format!(
                    "required tool {required} was not advertised"
                )));
            }
        }
        Ok(transport)
    }

    pub(super) fn capabilities(&self) -> DevinCapabilities {
        DevinCapabilities::new(self.tools.keys().cloned())
    }

    #[cfg(feature = "network")]
    pub(super) fn org_id(&self) -> &str {
        &self.org_id
    }

    #[cfg(not(feature = "network"))]
    pub(super) fn org_id(&self) -> &str {
        ""
    }

    pub(super) fn supports_action(&self, tool: &str, action: &str) -> bool {
        enum_choice(self.schema(tool), &["action", "operation"], &[action]).is_some()
    }

    #[cfg(feature = "network")]
    pub(super) fn upload_attachment(&mut self, path: &Path) -> Result<String, TransportError> {
        let value = self.upload_file(ApiVersion::V3, "/attachments", path)?;
        let uploaded: AttachmentUploadResponse = serde_json::from_value(value)
            .map_err(|error| TransportError::Protocol(error.to_string()))?;
        let url = uploaded.url;
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

    #[cfg(feature = "network")]
    pub(super) fn upload_blueprint_file(
        &mut self,
        blueprint_id: &str,
        path: &Path,
    ) -> Result<Value, TransportError> {
        if !safe_path_segment(blueprint_id) {
            return Err(TransportError::Protocol(
                "invalid Devin blueprint ID".into(),
            ));
        }
        self.upload_file(
            ApiVersion::V3Beta1,
            &format!("/snapshot-setup/blueprints/{blueprint_id}/files"),
            path,
        )
    }

    #[cfg(feature = "network")]
    fn upload_file(
        &mut self,
        version: ApiVersion,
        path: &str,
        file: &Path,
    ) -> Result<Value, TransportError> {
        let form = ureq::unversioned::multipart::Form::new()
            .file("file", file)
            .map_err(|error| TransportError::Protocol(error.to_string()))?;
        let authorization = format!("Bearer {}", self.credentials.api_key());
        let mut request = self
            .agent
            .post(organization_api_url(
                &self.api_endpoint,
                version,
                &self.org_id,
                path,
            )?)
            .header("Authorization", &authorization)
            .header("Accept", "application/json");
        request = request.header("X-Org-Id", &self.org_id);
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
        serde_json::from_slice(&bytes).map_err(|error| TransportError::Protocol(error.to_string()))
    }

    #[cfg(not(feature = "network"))]
    pub(super) fn upload_attachment(&mut self, _path: &Path) -> Result<String, TransportError> {
        Err(TransportError::Offline)
    }

    #[cfg(not(feature = "network"))]
    pub(super) fn upload_blueprint_file(
        &mut self,
        _blueprint_id: &str,
        _path: &Path,
    ) -> Result<Value, TransportError> {
        Err(TransportError::Offline)
    }

    #[cfg(feature = "network")]
    pub(super) fn send_v3_message(
        &mut self,
        session_id: &str,
        message: &str,
        attachment_urls: &[String],
    ) -> Result<(), TransportError> {
        if !safe_path_segment(session_id) {
            return Err(TransportError::Protocol("invalid Devin session ID".into()));
        }
        let authorization = format!("Bearer {}", self.credentials.api_key());
        let body = serde_json::to_vec(&json!({
            "message": message,
            "attachment_urls": attachment_urls,
        }))
        .map_err(|error| TransportError::Protocol(error.to_string()))?;
        let mut response = self
            .agent
            .post(format!(
                "{}/v3/organizations/{}/sessions/{session_id}/messages",
                self.api_endpoint, self.org_id
            ))
            .header("Authorization", &authorization)
            .header("X-Org-Id", &self.org_id)
            .header("Accept", "application/json")
            .header("Content-Type", "application/json")
            .send(&body)
            .map_err(|_| TransportError::Offline)?;
        ensure_success(&response)?;
        let bytes = response
            .body_mut()
            .with_config()
            .limit((MAX_RESPONSE_BYTES + 1) as u64)
            .read_to_vec()
            .map_err(|_| TransportError::Offline)?;
        if bytes.len() > MAX_RESPONSE_BYTES {
            return Err(TransportError::Oversized);
        }
        serde_json::from_slice::<Value>(&bytes)
            .map_err(|error| TransportError::Protocol(error.to_string()))?;
        Ok(())
    }

    #[cfg(not(feature = "network"))]
    pub(super) fn send_v3_message(
        &mut self,
        _session_id: &str,
        _message: &str,
        _attachment_urls: &[String],
    ) -> Result<(), TransportError> {
        Err(TransportError::Offline)
    }

    pub(super) fn search_with(
        &mut self,
        cursor: Option<&str>,
        filters: &SessionFilters,
    ) -> Result<Value, TransportError> {
        let arguments = search_arguments(self.schema("devin_session_search"), cursor, filters);
        self.call_tool("devin_session_search", Value::Object(arguments))
    }

    pub(super) fn gather(&mut self, session_ids: &[String]) -> Result<Value, TransportError> {
        let schema = self.schema("devin_session_gather");
        let mut arguments = serde_json::Map::new();
        insert_named(
            &mut arguments,
            schema,
            &["session_ids", "sessionIds", "ids"],
            json!(session_ids),
        );
        self.call_tool("devin_session_gather", Value::Object(arguments))
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
            InteractAction::SendMessage { .. } => {
                ("send_message", &["send_message", "message", "send"] as &[_])
            }
            InteractAction::Sleep => ("sleep", &["sleep"] as &[_]),
            InteractAction::Archive => ("archive", &["archive"] as &[_]),
            InteractAction::Unarchive => ("unarchive", &["unarchive"] as &[_]),
            InteractAction::Terminate => ("terminate", &["terminate", "stop"] as &[_]),
        };
        let action_value = action_choice(schema, aliases, semantic)?;
        insert_named(
            &mut arguments,
            schema,
            &["action", "operation"],
            json!(action_value),
        );
        if let InteractAction::SendMessage {
            message,
            attachment_ids,
        } = action
        {
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
            let action = action_choice(schema, &["list", "list_events", "summaries"], "list")?;
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

    pub(super) fn event_action(
        &mut self,
        session_id: &str,
        action: &str,
        value: &str,
    ) -> Result<Value, TransportError> {
        let schema = self.schema("devin_session_events");
        let mut arguments = serde_json::Map::new();
        insert_named(
            &mut arguments,
            schema,
            &["session_id", "sessionId", "id"],
            json!(session_id),
        );
        let action_value = if action == "search" {
            action_choice(schema, &["search", "search_events"], action)?
        } else {
            action_choice(
                schema,
                &["get", "get_event", "get_details", "details"],
                action,
            )?
        };
        insert_named(
            &mut arguments,
            schema,
            &["action", "operation"],
            json!(action_value),
        );
        if action == "search" {
            insert_named(
                &mut arguments,
                schema,
                &["query", "search", "text"],
                json!(value),
            );
        } else if let Some(name) = property_name(schema, &["event_ids", "eventIds"]) {
            arguments.insert(name, json!([value]));
        } else {
            insert_named(
                &mut arguments,
                schema,
                &["event_id", "eventId", "id"],
                json!(value),
            );
        }
        self.call_tool("devin_session_events", Value::Object(arguments))
    }

    pub(super) fn list_repositories(&mut self) -> Result<Value, TransportError> {
        self.call_tool("list_available_repos", json!({}))
    }

    pub(super) fn list_integrations(&mut self) -> Result<Value, TransportError> {
        self.call_tool("list_integrations", json!({}))
    }

    pub(super) fn wiki_structure(&mut self, repository: &str) -> Result<Value, TransportError> {
        self.call_repository_tool("read_wiki_structure", repository, None)
    }

    pub(super) fn wiki_contents(&mut self, repository: &str) -> Result<Value, TransportError> {
        self.call_repository_tool("read_wiki_contents", repository, None)
    }

    pub(super) fn ask_question(
        &mut self,
        repositories: &[String],
        question: &str,
    ) -> Result<Value, TransportError> {
        let schema = self.schema("ask_question");
        let mut arguments = serde_json::Map::new();
        insert_named(
            &mut arguments,
            schema,
            &["repoName", "repositories", "repos", "repo_names"],
            json!(repositories),
        );
        insert_named(
            &mut arguments,
            schema,
            &["question", "query"],
            json!(question),
        );
        self.call_tool("ask_question", Value::Object(arguments))
    }

    pub(super) fn manage(
        &mut self,
        tool: &str,
        action: &str,
        mut arguments: serde_json::Map<String, Value>,
    ) -> Result<Value, TransportError> {
        let schema = self.schema(tool);
        let action = action_choice(schema, &[action], action)?;
        insert_named(
            &mut arguments,
            schema,
            &["action", "operation"],
            json!(action),
        );
        self.call_tool(tool, Value::Object(arguments))
    }

    pub(super) fn v3(
        &mut self,
        method: V3Method,
        path: &str,
        body: Option<&Value>,
    ) -> Result<Value, TransportError> {
        self.organization_api(ApiVersion::V3, method, path, body)
    }

    pub(super) fn v3beta(
        &mut self,
        method: V3Method,
        path: &str,
        body: Option<&Value>,
    ) -> Result<Value, TransportError> {
        self.organization_api(ApiVersion::V3Beta1, method, path, body)
    }

    fn organization_api(
        &mut self,
        version: ApiVersion,
        method: V3Method,
        path: &str,
        body: Option<&Value>,
    ) -> Result<Value, TransportError> {
        #[cfg(not(feature = "network"))]
        {
            let _ = (version, method, path, body);
            return Err(TransportError::Offline);
        }
        #[cfg(feature = "network")]
        {
            let url = organization_api_url(&self.api_endpoint, version, &self.org_id, path)?;
            let authorization = format!("Bearer {}", self.credentials.api_key());
            let bytes = body
                .map(serde_json::to_vec)
                .transpose()
                .map_err(|error| TransportError::Protocol(error.to_string()))?;
            let mut response = match method {
                V3Method::Get => self
                    .agent
                    .get(&url)
                    .header("Authorization", &authorization)
                    .header("X-Org-Id", &self.org_id)
                    .header("Accept", "application/json")
                    .call(),
                V3Method::Post => self
                    .agent
                    .post(&url)
                    .header("Authorization", &authorization)
                    .header("X-Org-Id", &self.org_id)
                    .header("Accept", "application/json")
                    .header("Content-Type", "application/json")
                    .send(bytes.as_deref().unwrap_or(b"{}")),
                V3Method::Put => self
                    .agent
                    .put(&url)
                    .header("Authorization", &authorization)
                    .header("X-Org-Id", &self.org_id)
                    .header("Accept", "application/json")
                    .header("Content-Type", "application/json")
                    .send(bytes.as_deref().unwrap_or(b"{}")),
                V3Method::Patch => self
                    .agent
                    .patch(&url)
                    .header("Authorization", &authorization)
                    .header("X-Org-Id", &self.org_id)
                    .header("Accept", "application/json")
                    .header("Content-Type", "application/json")
                    .send(bytes.as_deref().unwrap_or(b"{}")),
                V3Method::Delete => self
                    .agent
                    .delete(&url)
                    .header("Authorization", &authorization)
                    .header("X-Org-Id", &self.org_id)
                    .header("Accept", "application/json")
                    .call(),
            }
            .map_err(|_| TransportError::Offline)?;
            ensure_success(&response)?;
            if matches!(response.status().as_u16(), 202 | 204) {
                return Ok(Value::Null);
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
            if bytes.is_empty() {
                Ok(Value::Null)
            } else {
                serde_json::from_slice(&bytes)
                    .map_err(|error| TransportError::Protocol(error.to_string()))
            }
        }
    }

    fn call_repository_tool(
        &mut self,
        tool: &str,
        repository: &str,
        extra: Option<(&str, Value)>,
    ) -> Result<Value, TransportError> {
        let schema = self.schema(tool);
        let mut arguments = serde_json::Map::new();
        insert_named(
            &mut arguments,
            schema,
            &["repoName", "repository", "repo", "repo_name"],
            json!(repository),
        );
        if let Some((name, value)) = extra {
            arguments.insert(name.to_owned(), value);
        }
        self.call_tool(tool, Value::Object(arguments))
    }

    fn schema(&self, tool: &str) -> &Value {
        self.tools
            .get(tool)
            .and_then(|tool| tool.get("inputSchema").or_else(|| tool.get("input_schema")))
            .unwrap_or(&Value::Null)
    }

    fn call_tool(&mut self, name: &str, arguments: Value) -> Result<Value, TransportError> {
        if !self.tools.contains_key(name) {
            return Err(TransportError::Unavailable);
        }
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
    pub(super) fn fetch_session_attachment(
        &mut self,
        attachment_id: &str,
        name: &str,
    ) -> Result<Vec<u8>, TransportError> {
        if !safe_path_segment(attachment_id)
            || name.is_empty()
            || name.len() > 4_096
            || name.chars().any(char::is_control)
        {
            return Err(TransportError::Protocol(
                "invalid Devin attachment path".into(),
            ));
        }
        let path = format!(
            "/attachments/{}/{}",
            attachment_id,
            encode_path_segment(name)
        );
        let url = organization_api_url(&self.api_endpoint, ApiVersion::V3, &self.org_id, &path)?;
        let authorization = format!("Bearer {}", self.credentials.api_key());
        let mut response = self
            .agent
            .get(url)
            .header("Authorization", &authorization)
            .header("X-Org-Id", &self.org_id)
            .header("Accept", "*/*")
            .call()
            .map_err(|_| TransportError::Offline)?;
        ensure_success(&response)?;
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
    pub(super) fn fetch_session_attachment(
        &mut self,
        _attachment_id: &str,
        _name: &str,
    ) -> Result<Vec<u8>, TransportError> {
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
        request = request.header("X-Org-Id", &self.org_id);
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

#[cfg(feature = "network")]
#[derive(Deserialize)]
struct SelfResponse {
    org_id: Option<String>,
}

#[cfg(feature = "network")]
#[derive(Deserialize)]
struct AttachmentUploadResponse {
    #[allow(dead_code)]
    attachment_id: String,
    #[allow(dead_code)]
    name: String,
    url: String,
}

#[cfg(feature = "network")]
fn resolve_org_id(
    agent: &ureq::Agent,
    api_endpoint: &str,
    credentials: &Credentials,
) -> Result<String, TransportError> {
    let authorization = format!("Bearer {}", credentials.api_key());
    let mut response = agent
        .get(format!("{}/v3/self", api_endpoint.trim_end_matches('/')))
        .header("Authorization", &authorization)
        .header("Accept", "application/json")
        .call()
        .map_err(|_| TransportError::Offline)?;
    ensure_success(&response)?;
    let bytes = response
        .body_mut()
        .with_config()
        .limit((MAX_RESPONSE_BYTES + 1) as u64)
        .read_to_vec()
        .map_err(|_| TransportError::Offline)?;
    if bytes.len() > MAX_RESPONSE_BYTES {
        return Err(TransportError::Oversized);
    }
    serde_json::from_slice::<SelfResponse>(&bytes)
        .map_err(|error| TransportError::Protocol(error.to_string()))?
        .org_id
        .map(Ok)
        .unwrap_or_else(|| discover_organizations(agent, api_endpoint, credentials))
}

#[cfg(feature = "network")]
fn discover_organizations(
    agent: &ureq::Agent,
    api_endpoint: &str,
    credentials: &Credentials,
) -> Result<String, TransportError> {
    let authorization = format!("Bearer {}", credentials.api_key());
    let mut response = agent
        .get(format!(
            "{}/v3/enterprise/organizations",
            api_endpoint.trim_end_matches('/')
        ))
        .header("Authorization", &authorization)
        .header("Accept", "application/json")
        .call()
        .map_err(|_| TransportError::Offline)?;
    ensure_success(&response)?;
    let bytes = response
        .body_mut()
        .with_config()
        .limit((MAX_RESPONSE_BYTES + 1) as u64)
        .read_to_vec()
        .map_err(|_| TransportError::Offline)?;
    if bytes.len() > MAX_RESPONSE_BYTES {
        return Err(TransportError::Oversized);
    }
    let value: Value = serde_json::from_slice(&bytes)
        .map_err(|error| TransportError::Protocol(error.to_string()))?;
    let items = value
        .as_array()
        .or_else(|| value.get("organizations").and_then(Value::as_array))
        .or_else(|| value.get("items").and_then(Value::as_array))
        .ok_or_else(|| TransportError::Protocol("organization list was invalid".into()))?;
    let organizations = items
        .iter()
        .take(1_000)
        .filter_map(|organization| {
            let id = organization
                .get("org_id")
                .or_else(|| organization.get("id"))?
                .as_str()?;
            safe_path_segment(id).then(|| DevinOrganization {
                id: id.into(),
                name: organization
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("Devin organization")
                    .chars()
                    .take(256)
                    .collect(),
            })
        })
        .collect::<Vec<_>>();
    match organizations.as_slice() {
        [organization] => Ok(organization.id.clone()),
        [] => Err(TransportError::Protocol(
            "No Devin organizations are available to this credential".into(),
        )),
        _ => Err(TransportError::OrganizationSelectionRequired(organizations)),
    }
}

#[cfg(feature = "network")]
fn ensure_success<T>(response: &ureq::http::Response<T>) -> Result<(), TransportError> {
    let status = response.status().as_u16();
    match status {
        200..=299 => Ok(()),
        401 => Err(TransportError::Authentication),
        403 => Err(TransportError::Forbidden),
        429 => Err(TransportError::RateLimited(
            response
                .headers()
                .get("Retry-After")
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.parse().ok()),
        )),
        500..=599 => Err(TransportError::Offline),
        _ => Err(TransportError::Remote(format!("HTTP {status}"))),
    }
}

fn organization_api_url(
    api_endpoint: &str,
    version: ApiVersion,
    org_id: &str,
    path: &str,
) -> Result<String, TransportError> {
    if !safe_path_segment(org_id)
        || path.len() > 8 * 1024
        || !path.starts_with('/')
        || path.contains("..")
        || path.chars().any(char::is_control)
    {
        return Err(TransportError::Protocol("invalid Devin API path".into()));
    }
    let version = match version {
        ApiVersion::V3 => "v3",
        ApiVersion::V3Beta1 => "v3beta1",
    };
    Ok(format!(
        "{}/{version}/organizations/{org_id}{path}",
        api_endpoint.trim_end_matches('/')
    ))
}

fn safe_path_segment(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 1_024
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

pub(super) fn encode_path_segment(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            encoded.push(char::from(byte));
        } else {
            use std::fmt::Write as _;
            let _ = write!(encoded, "%{byte:02X}");
        }
    }
    encoded
}

fn search_arguments(
    schema: &Value,
    cursor: Option<&str>,
    filters: &SessionFilters,
) -> serde_json::Map<String, Value> {
    let mut arguments = serde_json::Map::new();
    insert_if_property(
        schema,
        &mut arguments,
        &["first", "limit", "page_size", "pageSize"],
        json!(100),
    );
    if let Some(cursor) = cursor {
        insert_if_property(
            schema,
            &mut arguments,
            &["after", "cursor", "page_cursor", "pageCursor"],
            json!(cursor),
        );
    }

    let nested = schema
        .get("properties")
        .and_then(|properties| properties.get("filters"));
    let filter_schema = nested.unwrap_or(schema);
    let mut values = serde_json::Map::new();
    for (candidates, value) in [
        (
            &["origins", "origin", "session_origin"] as &[_],
            filters.origin.as_str(),
        ),
        (
            &["repository", "repo", "repo_name"],
            filters.repository.as_str(),
        ),
        (&["playbook_id", "playbookId"], filters.playbook_id.as_str()),
        (&["schedule_id", "scheduleId"], filters.schedule_id.as_str()),
        (&["user_ids", "user_id", "userId"], filters.user_id.as_str()),
        (
            &["parent_session_id", "parentSessionId", "parent_id"],
            filters.parent_session_id.as_str(),
        ),
        (&["category"], filters.category.as_str()),
        (&["status"], filters.status.as_str()),
        (
            &["created_after", "createdAfter"],
            filters.created_after.as_str(),
        ),
        (
            &["created_before", "createdBefore"],
            filters.created_before.as_str(),
        ),
        (
            &["updated_after", "updatedAfter"],
            filters.updated_after.as_str(),
        ),
        (
            &["updated_before", "updatedBefore"],
            filters.updated_before.as_str(),
        ),
    ] {
        if !value.is_empty() {
            insert_filter_property(filter_schema, &mut values, candidates, value);
        }
    }
    if !filters.tags.is_empty() {
        insert_if_property(filter_schema, &mut values, &["tags"], json!(filters.tags));
    }
    if nested.is_some() {
        if !values.is_empty() {
            arguments.insert("filters".into(), Value::Object(values));
        }
    } else {
        arguments.extend(values);
    }
    arguments
}

fn insert_filter_property(
    schema: &Value,
    target: &mut serde_json::Map<String, Value>,
    candidates: &[&str],
    value: &str,
) {
    let Some(name) = property_name(schema, candidates) else {
        return;
    };
    let kind = properties(schema)
        .and_then(|properties| properties.get(&name))
        .and_then(|property| property.get("type"))
        .and_then(Value::as_str);
    let value = match kind {
        Some("array") => json!([value]),
        Some("integer") => value
            .parse::<i64>()
            .map(Value::from)
            .unwrap_or_else(|_| json!(value)),
        _ => json!(value),
    };
    target.insert(name, value);
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
    let values = property_enum(schema, properties)?;
    choices.iter().find_map(|choice| {
        values
            .iter()
            .filter_map(Value::as_str)
            .find(|value| normalized(value) == normalized(choice))
            .map(str::to_owned)
    })
}

fn property_enum<'a>(schema: &'a Value, properties: &[&str]) -> Option<&'a Vec<Value>> {
    let name = property_name(schema, properties)?;
    schema.get("properties")?.get(name)?.get("enum")?.as_array()
}

fn action_choice(
    schema: &Value,
    aliases: &[&str],
    fallback: &str,
) -> Result<String, TransportError> {
    enum_choice(schema, &["action", "operation"], aliases)
        .map(Ok)
        .unwrap_or_else(|| {
            if property_enum(schema, &["action", "operation"]).is_some() {
                Err(TransportError::Unavailable)
            } else {
                Ok(fallback.into())
            }
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
            Self::OrganizationSelectionRequired(_) => "Choose a Devin organization to continue",
            Self::Authentication => "Devin authentication failed",
            Self::Forbidden => "This credential does not have permission for that Devin action",
            Self::Unavailable => "This Devin feature is not advertised for this organization",
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
    fn advertised_action_rejects_an_action_missing_from_a_known_enum() {
        let schema = serde_json::json!({
            "properties": { "action": { "enum": ["list", "create"] } }
        });

        assert_eq!(
            super::action_choice(&schema, &["delete"], "delete"),
            Err(super::TransportError::Unavailable)
        );
        assert_eq!(
            super::action_choice(&serde_json::Value::Null, &["delete"], "delete"),
            Ok("delete".into())
        );
    }

    #[test]
    fn documented_mcp_fixture_keeps_every_sidebar_tool_route_visible() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/devin/documented_mcp_tools.json"
        ))
        .unwrap();
        let tools = fixture["tools"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|tool| tool["name"].as_str())
            .collect::<std::collections::BTreeSet<_>>();

        for required in [
            "read_wiki_structure",
            "read_wiki_contents",
            "ask_question",
            "list_available_repos",
            "devin_session_create",
            "devin_session_search",
            "devin_session_interact",
            "devin_session_events",
            "devin_session_gather",
            "devin_playbook_manage",
            "devin_knowledge_manage",
            "devin_schedule_manage",
            "list_integrations",
        ] {
            assert!(
                tools.contains(required),
                "missing documented MCP tool {required}"
            );
        }
    }
    #[cfg(feature = "network")]
    #[test]
    fn current_attachment_upload_fixture_has_the_typed_v3_shape() {
        let upload: super::AttachmentUploadResponse = serde_json::from_str(include_str!(
            "../../tests/fixtures/devin/v3_attachment_upload.json"
        ))
        .unwrap();

        assert_eq!(upload.attachment_id, "attachment-sanitized-1");
        assert_eq!(upload.name, "mock.png");
        assert!(upload.url.starts_with("https://"));
    }
    #[test]
    fn organization_api_urls_keep_stable_and_beta_resources_in_their_documented_namespaces() {
        assert_eq!(
            super::organization_api_url(
                "https://api.devin.ai",
                super::ApiVersion::V3,
                "org_123",
                "/sessions"
            )
            .unwrap(),
            "https://api.devin.ai/v3/organizations/org_123/sessions"
        );
        assert_eq!(
            super::organization_api_url(
                "https://api.devin.ai",
                super::ApiVersion::V3Beta1,
                "org_123",
                "/snapshot-setup/builds"
            )
            .unwrap(),
            "https://api.devin.ai/v3beta1/organizations/org_123/snapshot-setup/builds"
        );
        assert!(
            super::organization_api_url(
                "https://api.devin.ai",
                super::ApiVersion::V3,
                "org_123",
                "/sessions/../secrets"
            )
            .is_err()
        );
    }

    #[test]
    fn session_search_forwards_only_documented_server_filter_fields() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "cursor": {"type": "string"},
                "limit": {"type": "integer"},
                "filters": {
                    "type": "object",
                    "properties": {
                        "repository": {"type": "string"},
                        "tags": {"type": "array"},
                        "status": {"type": "string"}
                    }
                }
            }
        });
        let filters = super::SessionFilters {
            repository: "openai/editur".into(),
            tags: vec!["editor".into()],
            status: "running".into(),
            origin: "must-not-be-invented".into(),
            ..super::SessionFilters::default()
        };

        assert_eq!(
            super::search_arguments(&schema, Some("page-2"), &filters),
            serde_json::json!({
                "cursor": "page-2",
                "limit": 100,
                "filters": {
                    "repository": "openai/editur",
                    "tags": ["editor"],
                    "status": "running"
                }
            })
            .as_object()
            .unwrap()
            .clone()
        );
    }

    #[test]
    fn current_session_search_schema_receives_pagination_and_typed_filters() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "first": {"type": "integer"},
                "after": {"type": "string"},
                "origins": {"type": "array"},
                "user_ids": {"type": "array"},
                "created_after": {"type": "integer"}
            }
        });
        let filters = super::SessionFilters {
            origin: "slack".into(),
            user_id: "user-123".into(),
            created_after: "1786906800".into(),
            ..super::SessionFilters::default()
        };

        assert_eq!(
            super::search_arguments(&schema, Some("page-2"), &filters),
            serde_json::json!({
                "first": 100,
                "after": "page-2",
                "origins": ["slack"],
                "user_ids": ["user-123"],
                "created_after": 1786906800
            })
            .as_object()
            .unwrap()
            .clone()
        );
    }

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

    #[test]
    #[ignore = "requires DEVIN_API_KEY and live Devin access"]
    fn live_credentials_connect_and_search_sessions() {
        let credentials = crate::devin::credentials::Credentials::load()
            .unwrap()
            .expect("DEVIN_API_KEY is required");
        let mut transport = super::McpTransport::connect(credentials).unwrap();
        let result = transport
            .search_with(None, &super::SessionFilters::default())
            .unwrap();
        let payload = crate::devin::normalize::tool_payload(result).unwrap();
        let (sessions, _, total, _) = crate::devin::normalize::sessions(&payload).unwrap();

        assert!(total.is_some_and(|total| total >= sessions.len()));
        let session = sessions.first().expect("a live session is required");
        let result = transport
            .v3(
                super::V3Method::Get,
                &format!("/sessions/{}", session.id),
                None,
            )
            .unwrap();
        let detail = crate::devin::normalize::detail(&result).unwrap();
        assert_eq!(detail.summary.id, session.id);

        let archived = sessions.iter().filter(|session| session.archived).take(5);
        let mut checked = 0;
        for session in archived {
            let result = transport
                .v3(
                    super::V3Method::Get,
                    &format!("/sessions/{}", session.id),
                    None,
                )
                .unwrap();
            let detail = crate::devin::normalize::detail(&result).unwrap();
            assert!(detail.summary.archived);
            assert_eq!(session.status, detail.summary.status);
            assert_eq!(session.status_detail, detail.summary.status_detail);
            checked += 1;
        }
        assert!(checked > 0, "a live archived session is required");
    }
}
