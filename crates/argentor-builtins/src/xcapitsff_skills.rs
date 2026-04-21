//! HTTP-based skills for querying the XcapitSFF backend.
//!
//! Provides five skills that interact with the XcapitSFF REST API:
//!
//! - [`XcapitSearchSkill`] — Full-text search across entities.
//! - [`XcapitLeadInfoSkill`] — Retrieve lead details by ID.
//! - [`XcapitTicketInfoSkill`] — Retrieve ticket details by ID.
//! - [`XcapitKbSearchSkill`] — Search the knowledge base for a ticket's subject/description.
//! - [`XcapitCustomer360Skill`] — Retrieve the 360-degree customer view.
//!
//! All skills share a single `reqwest::Client` with a 10-second timeout and
//! target the base URL configured via the `XCAPITSFF_URL` environment variable
//! (defaults to `http://localhost:8000`).

use argentor_core::{ArgentorResult, ToolCall, ToolResult};
use argentor_security::{Capability, PermissionSet};
use argentor_skills::skill::{Skill, SkillDescriptor};
use argentor_skills::SkillRegistry;
use async_trait::async_trait;
use std::sync::Arc;
use std::time::Duration;
use tracing::info;

/// Default base URL used when the `XCAPITSFF_URL` environment variable is not set.
const DEFAULT_BASE_URL: &str = "http://localhost:8000";

/// Build a shared `reqwest::Client` with a 10-second timeout.
///
/// # Panics
///
/// Panics if the TLS backend cannot be initialized (fundamentally broken
/// environment with no recovery path).
fn build_xcapitsff_client() -> reqwest::Client {
    #[allow(clippy::expect_used)]
    reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .expect("Failed to create HTTP client -- TLS backend unavailable")
}

/// Resolve the XcapitSFF base URL from the environment or a provided override.
///
/// Priority: `base_url` parameter > `XCAPITSFF_URL` env var > [`DEFAULT_BASE_URL`].
fn resolve_base_url(base_url: &str) -> String {
    if !base_url.is_empty() {
        return base_url.trim_end_matches('/').to_string();
    }
    std::env::var("XCAPITSFF_URL")
        .unwrap_or_else(|_| DEFAULT_BASE_URL.to_string())
        .trim_end_matches('/')
        .to_string()
}

/// Extract the host portion from a base URL for capability declarations.
fn host_from_url(base_url: &str) -> String {
    reqwest::Url::parse(base_url)
        .ok()
        .and_then(|u| u.host_str().map(String::from))
        .unwrap_or_else(|| "localhost".to_string())
}

// ---------------------------------------------------------------------------
// XcapitSearchSkill
// ---------------------------------------------------------------------------

/// Full-text search across XcapitSFF entities.
///
/// Issues a GET request to `/api/v1/search/?q={query}` and returns the JSON
/// response body.
pub struct XcapitSearchSkill {
    /// Skill metadata.
    descriptor: SkillDescriptor,
    /// Shared HTTP client.
    client: reqwest::Client,
    /// Resolved base URL (no trailing slash).
    base_url: String,
}

impl XcapitSearchSkill {
    /// Create a new `XcapitSearchSkill`.
    ///
    /// `base_url` overrides `XCAPITSFF_URL`; pass `""` to use the env/default.
    pub fn new(base_url: &str, client: reqwest::Client) -> Self {
        let base = resolve_base_url(base_url);
        let host = host_from_url(&base);
        Self {
            descriptor: SkillDescriptor {
                name: "xcapitsff_search".to_string(),
                description: "Search across XcapitSFF entities by query string.".to_string(),
                parameters_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "query": {
                            "type": "string",
                            "description": "The search query string"
                        }
                    },
                    "required": ["query"]
                }),
                required_capabilities: vec![Capability::NetworkAccess {
                    allowed_hosts: vec![host],
                }],
            },
            client,
            base_url: base,
        }
    }
}

#[async_trait]
impl Skill for XcapitSearchSkill {
    fn descriptor(&self) -> &SkillDescriptor {
        &self.descriptor
    }

    fn validate_arguments(
        &self,
        call: &ToolCall,
        _permissions: &PermissionSet,
    ) -> ArgentorResult<()> {
        let query = call.arguments.get("query").and_then(|v| v.as_str());
        if query.is_none() || query.is_some_and(str::is_empty) {
            return Err(argentor_core::ArgentorError::Skill(
                "Missing or empty 'query' parameter".to_string(),
            ));
        }
        Ok(())
    }

    async fn execute(&self, call: ToolCall) -> ArgentorResult<ToolResult> {
        let query = call.arguments["query"]
            .as_str()
            .unwrap_or_default()
            .to_string();

        if query.is_empty() {
            return Ok(ToolResult::error(
                &call.id,
                "Missing or empty 'query' parameter",
            ));
        }

        let mut parsed = reqwest::Url::parse(&format!("{}/api/v1/search/", self.base_url))
            .map_err(|e| argentor_core::ArgentorError::Skill(format!("Invalid base URL: {e}")))?;
        parsed.query_pairs_mut().append_pair("q", &query);
        let url = parsed.to_string();
        info!(skill = "xcapitsff_search", url = %url, "XcapitSFF search");

        match self.client.get(&url).send().await {
            Ok(resp) => {
                let status = resp.status().as_u16();
                let body = resp.text().await.unwrap_or_default();
                if (200..400).contains(&status) {
                    Ok(ToolResult::success(&call.id, body))
                } else {
                    Ok(ToolResult::error(
                        &call.id,
                        format!("HTTP {status}: {body}"),
                    ))
                }
            }
            Err(e) => Ok(ToolResult::error(&call.id, format!("Request failed: {e}"))),
        }
    }
}

// ---------------------------------------------------------------------------
// XcapitLeadInfoSkill
// ---------------------------------------------------------------------------

/// Retrieve lead details by ID from XcapitSFF.
///
/// Issues a GET request to `/api/v1/leads/{lead_id}`.
pub struct XcapitLeadInfoSkill {
    /// Skill metadata.
    descriptor: SkillDescriptor,
    /// Shared HTTP client.
    client: reqwest::Client,
    /// Resolved base URL (no trailing slash).
    base_url: String,
}

impl XcapitLeadInfoSkill {
    /// Create a new `XcapitLeadInfoSkill`.
    pub fn new(base_url: &str, client: reqwest::Client) -> Self {
        let base = resolve_base_url(base_url);
        let host = host_from_url(&base);
        Self {
            descriptor: SkillDescriptor {
                name: "xcapitsff_lead_info".to_string(),
                description: "Retrieve lead details by ID from XcapitSFF.".to_string(),
                parameters_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "lead_id": {
                            "type": "integer",
                            "description": "The lead ID to retrieve"
                        }
                    },
                    "required": ["lead_id"]
                }),
                required_capabilities: vec![Capability::NetworkAccess {
                    allowed_hosts: vec![host],
                }],
            },
            client,
            base_url: base,
        }
    }
}

#[async_trait]
impl Skill for XcapitLeadInfoSkill {
    fn descriptor(&self) -> &SkillDescriptor {
        &self.descriptor
    }

    fn validate_arguments(
        &self,
        call: &ToolCall,
        _permissions: &PermissionSet,
    ) -> ArgentorResult<()> {
        let lead_id = call.arguments.get("lead_id");
        if lead_id.is_none() || !lead_id.is_some_and(|v| v.is_i64() || v.is_u64()) {
            return Err(argentor_core::ArgentorError::Skill(
                "Missing or non-integer 'lead_id' parameter".to_string(),
            ));
        }
        Ok(())
    }

    async fn execute(&self, call: ToolCall) -> ArgentorResult<ToolResult> {
        let lead_id = match call.arguments["lead_id"].as_i64() {
            Some(id) => id,
            None => {
                return Ok(ToolResult::error(
                    &call.id,
                    "Missing or non-integer 'lead_id' parameter",
                ));
            }
        };

        let url = format!("{}/api/v1/leads/{}", self.base_url, lead_id);
        info!(skill = "xcapitsff_lead_info", url = %url, "XcapitSFF lead info");

        match self.client.get(&url).send().await {
            Ok(resp) => {
                let status = resp.status().as_u16();
                let body = resp.text().await.unwrap_or_default();
                if (200..400).contains(&status) {
                    Ok(ToolResult::success(&call.id, body))
                } else {
                    Ok(ToolResult::error(
                        &call.id,
                        format!("HTTP {status}: {body}"),
                    ))
                }
            }
            Err(e) => Ok(ToolResult::error(&call.id, format!("Request failed: {e}"))),
        }
    }
}

// ---------------------------------------------------------------------------
// XcapitTicketInfoSkill
// ---------------------------------------------------------------------------

/// Retrieve ticket details by ID from XcapitSFF.
///
/// Issues a GET request to `/api/v1/tickets/{ticket_id}`.
pub struct XcapitTicketInfoSkill {
    /// Skill metadata.
    descriptor: SkillDescriptor,
    /// Shared HTTP client.
    client: reqwest::Client,
    /// Resolved base URL (no trailing slash).
    base_url: String,
}

impl XcapitTicketInfoSkill {
    /// Create a new `XcapitTicketInfoSkill`.
    pub fn new(base_url: &str, client: reqwest::Client) -> Self {
        let base = resolve_base_url(base_url);
        let host = host_from_url(&base);
        Self {
            descriptor: SkillDescriptor {
                name: "xcapitsff_ticket_info".to_string(),
                description: "Retrieve ticket details by ID from XcapitSFF.".to_string(),
                parameters_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "ticket_id": {
                            "type": "integer",
                            "description": "The ticket ID to retrieve"
                        }
                    },
                    "required": ["ticket_id"]
                }),
                required_capabilities: vec![Capability::NetworkAccess {
                    allowed_hosts: vec![host],
                }],
            },
            client,
            base_url: base,
        }
    }
}

#[async_trait]
impl Skill for XcapitTicketInfoSkill {
    fn descriptor(&self) -> &SkillDescriptor {
        &self.descriptor
    }

    fn validate_arguments(
        &self,
        call: &ToolCall,
        _permissions: &PermissionSet,
    ) -> ArgentorResult<()> {
        let ticket_id = call.arguments.get("ticket_id");
        if ticket_id.is_none() || !ticket_id.is_some_and(|v| v.is_i64() || v.is_u64()) {
            return Err(argentor_core::ArgentorError::Skill(
                "Missing or non-integer 'ticket_id' parameter".to_string(),
            ));
        }
        Ok(())
    }

    async fn execute(&self, call: ToolCall) -> ArgentorResult<ToolResult> {
        let ticket_id = match call.arguments["ticket_id"].as_i64() {
            Some(id) => id,
            None => {
                return Ok(ToolResult::error(
                    &call.id,
                    "Missing or non-integer 'ticket_id' parameter",
                ));
            }
        };

        let url = format!("{}/api/v1/tickets/{}", self.base_url, ticket_id);
        info!(skill = "xcapitsff_ticket_info", url = %url, "XcapitSFF ticket info");

        match self.client.get(&url).send().await {
            Ok(resp) => {
                let status = resp.status().as_u16();
                let body = resp.text().await.unwrap_or_default();
                if (200..400).contains(&status) {
                    Ok(ToolResult::success(&call.id, body))
                } else {
                    Ok(ToolResult::error(
                        &call.id,
                        format!("HTTP {status}: {body}"),
                    ))
                }
            }
            Err(e) => Ok(ToolResult::error(&call.id, format!("Request failed: {e}"))),
        }
    }
}

// ---------------------------------------------------------------------------
// XcapitKbSearchSkill
// ---------------------------------------------------------------------------

/// Search the XcapitSFF knowledge base for articles matching a ticket's
/// subject and description.
///
/// Issues a GET request to
/// `/api/v1/knowledge/for-ticket?subject={subject}&description={description}`.
pub struct XcapitKbSearchSkill {
    /// Skill metadata.
    descriptor: SkillDescriptor,
    /// Shared HTTP client.
    client: reqwest::Client,
    /// Resolved base URL (no trailing slash).
    base_url: String,
}

impl XcapitKbSearchSkill {
    /// Create a new `XcapitKbSearchSkill`.
    pub fn new(base_url: &str, client: reqwest::Client) -> Self {
        let base = resolve_base_url(base_url);
        let host = host_from_url(&base);
        Self {
            descriptor: SkillDescriptor {
                name: "xcapitsff_kb_search".to_string(),
                description: "Search the knowledge base for articles matching a ticket subject and description.".to_string(),
                parameters_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "subject": {
                            "type": "string",
                            "description": "The ticket subject to search for"
                        },
                        "description": {
                            "type": "string",
                            "description": "The ticket description to search for"
                        }
                    },
                    "required": ["subject", "description"]
                }),
                required_capabilities: vec![Capability::NetworkAccess {
                    allowed_hosts: vec![host],
                }],
            },
            client,
            base_url: base,
        }
    }
}

#[async_trait]
impl Skill for XcapitKbSearchSkill {
    fn descriptor(&self) -> &SkillDescriptor {
        &self.descriptor
    }

    fn validate_arguments(
        &self,
        call: &ToolCall,
        _permissions: &PermissionSet,
    ) -> ArgentorResult<()> {
        let subject = call.arguments.get("subject").and_then(|v| v.as_str());
        let description = call.arguments.get("description").and_then(|v| v.as_str());

        if subject.is_none() || subject.is_some_and(str::is_empty) {
            return Err(argentor_core::ArgentorError::Skill(
                "Missing or empty 'subject' parameter".to_string(),
            ));
        }
        if description.is_none() || description.is_some_and(str::is_empty) {
            return Err(argentor_core::ArgentorError::Skill(
                "Missing or empty 'description' parameter".to_string(),
            ));
        }
        Ok(())
    }

    async fn execute(&self, call: ToolCall) -> ArgentorResult<ToolResult> {
        let subject = call.arguments["subject"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let description = call.arguments["description"]
            .as_str()
            .unwrap_or_default()
            .to_string();

        if subject.is_empty() {
            return Ok(ToolResult::error(
                &call.id,
                "Missing or empty 'subject' parameter",
            ));
        }
        if description.is_empty() {
            return Ok(ToolResult::error(
                &call.id,
                "Missing or empty 'description' parameter",
            ));
        }

        let mut parsed =
            reqwest::Url::parse(&format!("{}/api/v1/knowledge/for-ticket", self.base_url))
                .map_err(|e| {
                    argentor_core::ArgentorError::Skill(format!("Invalid base URL: {e}"))
                })?;
        parsed
            .query_pairs_mut()
            .append_pair("subject", &subject)
            .append_pair("description", &description);
        let url = parsed.to_string();
        info!(skill = "xcapitsff_kb_search", url = %url, "XcapitSFF KB search");

        match self.client.get(&url).send().await {
            Ok(resp) => {
                let status = resp.status().as_u16();
                let body = resp.text().await.unwrap_or_default();
                if (200..400).contains(&status) {
                    Ok(ToolResult::success(&call.id, body))
                } else {
                    Ok(ToolResult::error(
                        &call.id,
                        format!("HTTP {status}: {body}"),
                    ))
                }
            }
            Err(e) => Ok(ToolResult::error(&call.id, format!("Request failed: {e}"))),
        }
    }
}

// ---------------------------------------------------------------------------
// XcapitCustomer360Skill
// ---------------------------------------------------------------------------

/// Retrieve a 360-degree customer view from XcapitSFF.
///
/// Issues a GET request to `/api/v1/customers/{customer_id}/360`.
pub struct XcapitCustomer360Skill {
    /// Skill metadata.
    descriptor: SkillDescriptor,
    /// Shared HTTP client.
    client: reqwest::Client,
    /// Resolved base URL (no trailing slash).
    base_url: String,
}

impl XcapitCustomer360Skill {
    /// Create a new `XcapitCustomer360Skill`.
    pub fn new(base_url: &str, client: reqwest::Client) -> Self {
        let base = resolve_base_url(base_url);
        let host = host_from_url(&base);
        Self {
            descriptor: SkillDescriptor {
                name: "xcapitsff_customer360".to_string(),
                description: "Retrieve the 360-degree customer view by customer ID from XcapitSFF."
                    .to_string(),
                parameters_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "customer_id": {
                            "type": "integer",
                            "description": "The customer ID to retrieve the 360 view for"
                        }
                    },
                    "required": ["customer_id"]
                }),
                required_capabilities: vec![Capability::NetworkAccess {
                    allowed_hosts: vec![host],
                }],
            },
            client,
            base_url: base,
        }
    }
}

#[async_trait]
impl Skill for XcapitCustomer360Skill {
    fn descriptor(&self) -> &SkillDescriptor {
        &self.descriptor
    }

    fn validate_arguments(
        &self,
        call: &ToolCall,
        _permissions: &PermissionSet,
    ) -> ArgentorResult<()> {
        let customer_id = call.arguments.get("customer_id");
        if customer_id.is_none() || !customer_id.is_some_and(|v| v.is_i64() || v.is_u64()) {
            return Err(argentor_core::ArgentorError::Skill(
                "Missing or non-integer 'customer_id' parameter".to_string(),
            ));
        }
        Ok(())
    }

    async fn execute(&self, call: ToolCall) -> ArgentorResult<ToolResult> {
        let customer_id = match call.arguments["customer_id"].as_i64() {
            Some(id) => id,
            None => {
                return Ok(ToolResult::error(
                    &call.id,
                    "Missing or non-integer 'customer_id' parameter",
                ));
            }
        };

        let url = format!("{}/api/v1/customers/{}/360", self.base_url, customer_id);
        info!(skill = "xcapitsff_customer360", url = %url, "XcapitSFF customer 360");

        match self.client.get(&url).send().await {
            Ok(resp) => {
                let status = resp.status().as_u16();
                let body = resp.text().await.unwrap_or_default();
                if (200..400).contains(&status) {
                    Ok(ToolResult::success(&call.id, body))
                } else {
                    Ok(ToolResult::error(
                        &call.id,
                        format!("HTTP {status}: {body}"),
                    ))
                }
            }
            Err(e) => Ok(ToolResult::error(&call.id, format!("Request failed: {e}"))),
        }
    }
}

// ---------------------------------------------------------------------------
// Registration helper
// ---------------------------------------------------------------------------

/// Register all five XcapitSFF skills into the given registry.
///
/// A single `reqwest::Client` with a 10-second timeout is shared across all
/// skills. `base_url` overrides the `XCAPITSFF_URL` env var; pass `""` to
/// use the environment variable or the default (`http://localhost:8000`).
pub fn register_xcapitsff_skills(registry: &SkillRegistry, base_url: &str) {
    let client = build_xcapitsff_client();
    registry.register(Arc::new(XcapitSearchSkill::new(base_url, client.clone())));
    registry.register(Arc::new(XcapitLeadInfoSkill::new(base_url, client.clone())));
    registry.register(Arc::new(XcapitTicketInfoSkill::new(
        base_url,
        client.clone(),
    )));
    registry.register(Arc::new(XcapitKbSearchSkill::new(base_url, client.clone())));
    registry.register(Arc::new(XcapitCustomer360Skill::new(base_url, client)));
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use argentor_security::PermissionSet;
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// Helper: create a ToolCall with the given name and arguments.
    fn make_call(name: &str, args: serde_json::Value) -> ToolCall {
        ToolCall {
            id: format!("test_{name}"),
            name: name.to_string(),
            arguments: args,
        }
    }

    // -- Descriptor name tests -----------------------------------------------

    #[test]
    fn test_search_descriptor_name() {
        let client = build_xcapitsff_client();
        let skill = XcapitSearchSkill::new("http://example.com", client);
        assert_eq!(skill.descriptor().name, "xcapitsff_search");
    }

    #[test]
    fn test_lead_info_descriptor_name() {
        let client = build_xcapitsff_client();
        let skill = XcapitLeadInfoSkill::new("http://example.com", client);
        assert_eq!(skill.descriptor().name, "xcapitsff_lead_info");
    }

    #[test]
    fn test_ticket_info_descriptor_name() {
        let client = build_xcapitsff_client();
        let skill = XcapitTicketInfoSkill::new("http://example.com", client);
        assert_eq!(skill.descriptor().name, "xcapitsff_ticket_info");
    }

    #[test]
    fn test_kb_search_descriptor_name() {
        let client = build_xcapitsff_client();
        let skill = XcapitKbSearchSkill::new("http://example.com", client);
        assert_eq!(skill.descriptor().name, "xcapitsff_kb_search");
    }

    #[test]
    fn test_customer360_descriptor_name() {
        let client = build_xcapitsff_client();
        let skill = XcapitCustomer360Skill::new("http://example.com", client);
        assert_eq!(skill.descriptor().name, "xcapitsff_customer360");
    }

    // -- Descriptor capability tests -----------------------------------------

    #[test]
    fn test_descriptor_capabilities_contain_host() {
        let client = build_xcapitsff_client();
        let skill = XcapitSearchSkill::new("http://my-backend:9000", client);
        let caps = &skill.descriptor().required_capabilities;
        assert_eq!(caps.len(), 1);
        match &caps[0] {
            Capability::NetworkAccess { allowed_hosts } => {
                assert_eq!(allowed_hosts, &["my-backend".to_string()]);
            }
            other => panic!("Expected NetworkAccess, got {other:?}"),
        }
    }

    // -- Parameter validation tests ------------------------------------------

    #[test]
    fn test_search_validate_missing_query() {
        let client = build_xcapitsff_client();
        let skill = XcapitSearchSkill::new("http://x", client);
        let call = make_call("xcapitsff_search", serde_json::json!({}));
        let perms = PermissionSet::new();
        assert!(skill.validate_arguments(&call, &perms).is_err());
    }

    #[test]
    fn test_search_validate_empty_query() {
        let client = build_xcapitsff_client();
        let skill = XcapitSearchSkill::new("http://x", client);
        let call = make_call("xcapitsff_search", serde_json::json!({"query": ""}));
        let perms = PermissionSet::new();
        assert!(skill.validate_arguments(&call, &perms).is_err());
    }

    #[test]
    fn test_lead_validate_missing_id() {
        let client = build_xcapitsff_client();
        let skill = XcapitLeadInfoSkill::new("http://x", client);
        let call = make_call("xcapitsff_lead_info", serde_json::json!({}));
        let perms = PermissionSet::new();
        assert!(skill.validate_arguments(&call, &perms).is_err());
    }

    #[test]
    fn test_lead_validate_non_integer_id() {
        let client = build_xcapitsff_client();
        let skill = XcapitLeadInfoSkill::new("http://x", client);
        let call = make_call("xcapitsff_lead_info", serde_json::json!({"lead_id": "abc"}));
        let perms = PermissionSet::new();
        assert!(skill.validate_arguments(&call, &perms).is_err());
    }

    #[test]
    fn test_ticket_validate_missing_id() {
        let client = build_xcapitsff_client();
        let skill = XcapitTicketInfoSkill::new("http://x", client);
        let call = make_call("xcapitsff_ticket_info", serde_json::json!({}));
        let perms = PermissionSet::new();
        assert!(skill.validate_arguments(&call, &perms).is_err());
    }

    #[test]
    fn test_customer360_validate_missing_id() {
        let client = build_xcapitsff_client();
        let skill = XcapitCustomer360Skill::new("http://x", client);
        let call = make_call("xcapitsff_customer360", serde_json::json!({}));
        let perms = PermissionSet::new();
        assert!(skill.validate_arguments(&call, &perms).is_err());
    }

    #[test]
    fn test_kb_validate_missing_subject() {
        let client = build_xcapitsff_client();
        let skill = XcapitKbSearchSkill::new("http://x", client);
        let call = make_call(
            "xcapitsff_kb_search",
            serde_json::json!({"description": "desc"}),
        );
        let perms = PermissionSet::new();
        assert!(skill.validate_arguments(&call, &perms).is_err());
    }

    #[test]
    fn test_kb_validate_missing_description() {
        let client = build_xcapitsff_client();
        let skill = XcapitKbSearchSkill::new("http://x", client);
        let call = make_call(
            "xcapitsff_kb_search",
            serde_json::json!({"subject": "subj"}),
        );
        let perms = PermissionSet::new();
        assert!(skill.validate_arguments(&call, &perms).is_err());
    }

    // -- Execute with mock server tests --------------------------------------

    #[tokio::test]
    async fn test_search_execute_success() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/search/"))
            .and(query_param("q", "test query"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({"results": []})),
            )
            .mount(&server)
            .await;

        let client = build_xcapitsff_client();
        let skill = XcapitSearchSkill::new(&server.uri(), client);
        let call = make_call(
            "xcapitsff_search",
            serde_json::json!({"query": "test query"}),
        );
        let result = skill.execute(call).await.unwrap();
        assert!(
            !result.is_error,
            "Expected success, got: {}",
            result.content
        );
        assert!(result.content.contains("results"));
    }

    #[tokio::test]
    async fn test_lead_info_execute_success() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/leads/42"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"id": 42, "name": "John Doe"})),
            )
            .mount(&server)
            .await;

        let client = build_xcapitsff_client();
        let skill = XcapitLeadInfoSkill::new(&server.uri(), client);
        let call = make_call("xcapitsff_lead_info", serde_json::json!({"lead_id": 42}));
        let result = skill.execute(call).await.unwrap();
        assert!(
            !result.is_error,
            "Expected success, got: {}",
            result.content
        );
        assert!(result.content.contains("John Doe"));
    }

    #[tokio::test]
    async fn test_ticket_info_execute_success() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/tickets/99"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"id": 99, "status": "open"})),
            )
            .mount(&server)
            .await;

        let client = build_xcapitsff_client();
        let skill = XcapitTicketInfoSkill::new(&server.uri(), client);
        let call = make_call(
            "xcapitsff_ticket_info",
            serde_json::json!({"ticket_id": 99}),
        );
        let result = skill.execute(call).await.unwrap();
        assert!(
            !result.is_error,
            "Expected success, got: {}",
            result.content
        );
        assert!(result.content.contains("open"));
    }

    #[tokio::test]
    async fn test_kb_search_execute_success() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/knowledge/for-ticket"))
            .and(query_param("subject", "login issue"))
            .and(query_param("description", "cannot login"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"articles": [{"title": "Reset password"}]})),
            )
            .mount(&server)
            .await;

        let client = build_xcapitsff_client();
        let skill = XcapitKbSearchSkill::new(&server.uri(), client);
        let call = make_call(
            "xcapitsff_kb_search",
            serde_json::json!({"subject": "login issue", "description": "cannot login"}),
        );
        let result = skill.execute(call).await.unwrap();
        assert!(
            !result.is_error,
            "Expected success, got: {}",
            result.content
        );
        assert!(result.content.contains("Reset password"));
    }

    #[tokio::test]
    async fn test_customer360_execute_success() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/customers/7/360"))
            .respond_with(ResponseTemplate::new(200).set_body_json(
                serde_json::json!({"customer_id": 7, "name": "Acme Corp", "tickets": 3}),
            ))
            .mount(&server)
            .await;

        let client = build_xcapitsff_client();
        let skill = XcapitCustomer360Skill::new(&server.uri(), client);
        let call = make_call(
            "xcapitsff_customer360",
            serde_json::json!({"customer_id": 7}),
        );
        let result = skill.execute(call).await.unwrap();
        assert!(
            !result.is_error,
            "Expected success, got: {}",
            result.content
        );
        assert!(result.content.contains("Acme Corp"));
    }

    #[tokio::test]
    async fn test_execute_returns_error_on_404() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/leads/999"))
            .respond_with(ResponseTemplate::new(404).set_body_string("not found"))
            .mount(&server)
            .await;

        let client = build_xcapitsff_client();
        let skill = XcapitLeadInfoSkill::new(&server.uri(), client);
        let call = make_call("xcapitsff_lead_info", serde_json::json!({"lead_id": 999}));
        let result = skill.execute(call).await.unwrap();
        assert!(result.is_error, "Expected error on 404");
        assert!(result.content.contains("404"));
    }

    #[tokio::test]
    async fn test_execute_returns_error_on_500() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/tickets/1"))
            .respond_with(ResponseTemplate::new(500).set_body_string("internal server error"))
            .mount(&server)
            .await;

        let client = build_xcapitsff_client();
        let skill = XcapitTicketInfoSkill::new(&server.uri(), client);
        let call = make_call("xcapitsff_ticket_info", serde_json::json!({"ticket_id": 1}));
        let result = skill.execute(call).await.unwrap();
        assert!(result.is_error, "Expected error on 500");
        assert!(result.content.contains("500"));
    }

    #[tokio::test]
    async fn test_search_execute_empty_query_returns_error() {
        let client = build_xcapitsff_client();
        let skill = XcapitSearchSkill::new("http://unused", client);
        let call = make_call("xcapitsff_search", serde_json::json!({"query": ""}));
        let result = skill.execute(call).await.unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("query"));
    }

    // -- Registration test ---------------------------------------------------

    #[test]
    fn test_register_xcapitsff_skills_registers_all_five() {
        let registry = SkillRegistry::new();
        register_xcapitsff_skills(&registry, "http://test-backend:8000");
        assert!(registry.get("xcapitsff_search").is_some());
        assert!(registry.get("xcapitsff_lead_info").is_some());
        assert!(registry.get("xcapitsff_ticket_info").is_some());
        assert!(registry.get("xcapitsff_kb_search").is_some());
        assert!(registry.get("xcapitsff_customer360").is_some());
    }

    // -- Base URL resolution tests -------------------------------------------

    #[test]
    fn test_resolve_base_url_uses_provided() {
        let url = resolve_base_url("http://custom:9000/");
        assert_eq!(url, "http://custom:9000");
    }

    #[test]
    fn test_resolve_base_url_strips_trailing_slash() {
        let url = resolve_base_url("http://example.com/");
        assert_eq!(url, "http://example.com");
    }

    #[test]
    fn test_host_from_url_extracts_host() {
        assert_eq!(host_from_url("http://my-host:8000"), "my-host");
        assert_eq!(
            host_from_url("https://api.example.com/path"),
            "api.example.com"
        );
    }
}
