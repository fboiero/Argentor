//! OpenAPI 3.0 specification generator for Argentor REST API.
//!
//! Auto-generates an OpenAPI spec from route definitions, making it
//! easy to produce API documentation and client SDKs.
//!
//! # Main types
//!
//! - [`OpenApiGenerator`] — Builds an OpenAPI 3.0 spec.
//! - [`ApiEndpoint`] — Description of a single API endpoint.
//! - [`ApiParameter`] — Query/path/header parameter definition.
//! - [`ApiResponse`] — Response definition for an endpoint.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// HttpMethod
// ---------------------------------------------------------------------------

/// HTTP methods supported by the API.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HttpMethod {
    /// HTTP GET.
    Get,
    /// HTTP POST.
    Post,
    /// HTTP PUT.
    Put,
    /// HTTP DELETE.
    Delete,
    /// HTTP PATCH.
    Patch,
}

impl HttpMethod {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Get => "get",
            Self::Post => "post",
            Self::Put => "put",
            Self::Delete => "delete",
            Self::Patch => "patch",
        }
    }
}

// ---------------------------------------------------------------------------
// ParameterLocation
// ---------------------------------------------------------------------------

/// Where a parameter is located in the request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ParameterLocation {
    /// URL path parameter.
    Path,
    /// URL query parameter.
    Query,
    /// HTTP header.
    Header,
}

// ---------------------------------------------------------------------------
// ApiParameter
// ---------------------------------------------------------------------------

/// Definition of an API parameter.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiParameter {
    /// Parameter name.
    pub name: String,
    /// Where the parameter is located.
    pub location: ParameterLocation,
    /// Whether the parameter is required.
    pub required: bool,
    /// Data type (string, integer, boolean, etc.).
    pub data_type: String,
    /// Description.
    pub description: String,
}

impl ApiParameter {
    /// Create a required path parameter.
    pub fn path(name: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            location: ParameterLocation::Path,
            required: true,
            data_type: "string".to_string(),
            description: description.into(),
        }
    }

    /// Create an optional query parameter.
    pub fn query(
        name: impl Into<String>,
        data_type: impl Into<String>,
        description: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            location: ParameterLocation::Query,
            required: false,
            data_type: data_type.into(),
            description: description.into(),
        }
    }
}

// ---------------------------------------------------------------------------
// ApiResponse
// ---------------------------------------------------------------------------

/// Definition of an API response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiResponse {
    /// HTTP status code.
    pub status_code: u16,
    /// Description of the response.
    pub description: String,
    /// Content type (e.g., "application/json").
    pub content_type: Option<String>,
    /// Example response body (JSON string).
    pub example: Option<String>,
}

impl ApiResponse {
    /// Create a JSON response definition.
    pub fn json(status_code: u16, description: impl Into<String>) -> Self {
        Self {
            status_code,
            description: description.into(),
            content_type: Some("application/json".to_string()),
            example: None,
        }
    }

    /// Add an example response body.
    pub fn with_example(mut self, example: impl Into<String>) -> Self {
        self.example = Some(example.into());
        self
    }

    /// Conventional Argentor error response: `{"error": "message"}`.
    ///
    /// Use for documenting non-200 responses. The `description` should explain
    /// the failure mode; the example is fixed to the wire shape.
    pub fn error(status_code: u16, description: impl Into<String>) -> Self {
        Self {
            status_code,
            description: description.into(),
            content_type: Some("application/json".to_string()),
            example: Some(r#"{"error":"<message>"}"#.to_string()),
        }
    }

    /// Standard 401 Unauthorized response.
    pub fn unauthorized() -> Self {
        Self::error(401, "Authentication required")
            .with_example(r#"{"error":"Authentication required"}"#)
    }

    /// Standard 500 Internal Server Error response.
    pub fn internal_error() -> Self {
        Self::error(500, "Internal server error")
    }

    /// Standard 400 Bad Request response.
    pub fn bad_request(description: impl Into<String>) -> Self {
        Self::error(400, description)
    }
}

// ---------------------------------------------------------------------------
// ApiEndpoint
// ---------------------------------------------------------------------------

/// Description of a single API endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiEndpoint {
    /// URL path (e.g., "/api/v1/sessions").
    pub path: String,
    /// HTTP method.
    pub method: HttpMethod,
    /// Short summary of the endpoint.
    pub summary: String,
    /// Detailed description.
    pub description: String,
    /// Tags for grouping endpoints.
    pub tags: Vec<String>,
    /// Parameters.
    pub parameters: Vec<ApiParameter>,
    /// Response definitions.
    pub responses: Vec<ApiResponse>,
    /// Whether authentication is required.
    pub auth_required: bool,
}

impl ApiEndpoint {
    /// Create a new endpoint.
    pub fn new(method: HttpMethod, path: impl Into<String>, summary: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            method,
            summary: summary.into(),
            description: String::new(),
            tags: Vec::new(),
            parameters: Vec::new(),
            responses: vec![ApiResponse::json(200, "Successful response")],
            auth_required: false,
        }
    }

    /// Set description.
    pub fn with_description(mut self, desc: impl Into<String>) -> Self {
        self.description = desc.into();
        self
    }

    /// Add a tag.
    pub fn with_tag(mut self, tag: impl Into<String>) -> Self {
        self.tags.push(tag.into());
        self
    }

    /// Add a parameter.
    pub fn with_parameter(mut self, param: ApiParameter) -> Self {
        self.parameters.push(param);
        self
    }

    /// Add a response.
    pub fn with_response(mut self, response: ApiResponse) -> Self {
        self.responses.push(response);
        self
    }

    /// Mark as requiring authentication.
    pub fn requires_auth(mut self) -> Self {
        self.auth_required = true;
        self
    }
}

// ---------------------------------------------------------------------------
// OpenApiGenerator
// ---------------------------------------------------------------------------

/// Builds an OpenAPI 3.0 specification from endpoint definitions.
pub struct OpenApiGenerator {
    title: String,
    version: String,
    description: String,
    server_url: String,
    endpoints: Vec<ApiEndpoint>,
}

impl OpenApiGenerator {
    /// Create a new generator.
    pub fn new(title: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            version: version.into(),
            description: String::new(),
            server_url: std::env::var("ARGENTOR_SERVER_URL")
                .unwrap_or_else(|_| "http://localhost:8080".to_string()),
            endpoints: Vec::new(),
        }
    }

    /// Set the API description.
    pub fn with_description(mut self, desc: impl Into<String>) -> Self {
        self.description = desc.into();
        self
    }

    /// Set the server URL.
    pub fn with_server(mut self, url: impl Into<String>) -> Self {
        self.server_url = url.into();
        self
    }

    /// Add an endpoint.
    pub fn add_endpoint(&mut self, endpoint: ApiEndpoint) {
        self.endpoints.push(endpoint);
    }

    /// Generate the OpenAPI 3.0 spec as a JSON value.
    pub fn generate(&self) -> serde_json::Value {
        let mut paths = serde_json::Map::new();

        for endpoint in &self.endpoints {
            let path_entry = paths
                .entry(endpoint.path.clone())
                .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));

            if let Some(obj) = path_entry.as_object_mut() {
                let mut operation = serde_json::Map::new();
                operation.insert(
                    "summary".to_string(),
                    serde_json::Value::String(endpoint.summary.clone()),
                );

                if !endpoint.description.is_empty() {
                    operation.insert(
                        "description".to_string(),
                        serde_json::Value::String(endpoint.description.clone()),
                    );
                }

                if !endpoint.tags.is_empty() {
                    let tags: Vec<serde_json::Value> = endpoint
                        .tags
                        .iter()
                        .map(|t| serde_json::Value::String(t.clone()))
                        .collect();
                    operation.insert("tags".to_string(), serde_json::Value::Array(tags));
                }

                // Parameters
                if !endpoint.parameters.is_empty() {
                    let params: Vec<serde_json::Value> = endpoint
                        .parameters
                        .iter()
                        .map(|p| {
                            let loc = match p.location {
                                ParameterLocation::Path => "path",
                                ParameterLocation::Query => "query",
                                ParameterLocation::Header => "header",
                            };
                            serde_json::json!({
                                "name": p.name,
                                "in": loc,
                                "required": p.required,
                                "description": p.description,
                                "schema": { "type": p.data_type }
                            })
                        })
                        .collect();
                    operation.insert("parameters".to_string(), serde_json::Value::Array(params));
                }

                // Responses
                let mut responses_map = serde_json::Map::new();
                for resp in &endpoint.responses {
                    let mut resp_obj = serde_json::Map::new();
                    resp_obj.insert(
                        "description".to_string(),
                        serde_json::Value::String(resp.description.clone()),
                    );
                    if let Some(ct) = &resp.content_type {
                        let mut content = serde_json::Map::new();
                        let mut media_type = serde_json::Map::new();
                        if let Some(ex) = &resp.example {
                            if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(ex) {
                                let mut ex_obj = serde_json::Map::new();
                                ex_obj.insert("example".to_string(), parsed);
                                media_type = ex_obj;
                            }
                        }
                        content.insert(ct.clone(), serde_json::Value::Object(media_type));
                        resp_obj.insert("content".to_string(), serde_json::Value::Object(content));
                    }
                    responses_map.insert(
                        resp.status_code.to_string(),
                        serde_json::Value::Object(resp_obj),
                    );
                }
                operation.insert(
                    "responses".to_string(),
                    serde_json::Value::Object(responses_map),
                );

                // Security
                if endpoint.auth_required {
                    operation.insert(
                        "security".to_string(),
                        serde_json::json!([{"bearerAuth": []}]),
                    );
                }

                obj.insert(
                    endpoint.method.as_str().to_string(),
                    serde_json::Value::Object(operation),
                );
            }
        }

        serde_json::json!({
            "openapi": "3.0.3",
            "info": {
                "title": self.title,
                "version": self.version,
                "description": self.description
            },
            "servers": [{ "url": self.server_url }],
            "paths": paths,
            "components": {
                "securitySchemes": {
                    "bearerAuth": {
                        "type": "http",
                        "scheme": "bearer",
                        "bearerFormat": "JWT"
                    },
                    "apiKey": {
                        "type": "apiKey",
                        "in": "header",
                        "name": "X-API-Key"
                    }
                },
                "schemas": {
                    "ErrorResponse": {
                        "type": "object",
                        "required": ["error"],
                        "properties": {
                            "error": {
                                "type": "string",
                                "description": "Human-readable error message"
                            }
                        }
                    }
                }
            }
        })
    }

    /// Generate the OpenAPI spec as a pretty-printed JSON string.
    pub fn generate_json(&self) -> String {
        serde_json::to_string_pretty(&self.generate()).unwrap_or_default()
    }

    /// Get the Argentor default API endpoints.
    pub fn argentor_default() -> Self {
        let mut gen = Self::new("Argentor API", "1.0.0")
            .with_description("Argentor — Secure AI Agent Framework API")
            .with_server(
                std::env::var("ARGENTOR_SERVER_URL")
                    .unwrap_or_else(|_| "http://localhost:8080".to_string()),
            );

        // Core endpoints
        gen.add_endpoint(ApiEndpoint::new(HttpMethod::Get, "/health", "Health check"));
        gen.add_endpoint(ApiEndpoint::new(
            HttpMethod::Get,
            "/metrics",
            "Prometheus metrics",
        ));
        gen.add_endpoint(ApiEndpoint::new(
            HttpMethod::Get,
            "/openapi.json",
            "OpenAPI specification (self-reference)",
        ));
        gen.add_endpoint(ApiEndpoint::new(
            HttpMethod::Get,
            "/dashboard",
            "Web dashboard",
        ));
        gen.add_endpoint(ApiEndpoint::new(
            HttpMethod::Get,
            "/dashboard/audit",
            "Audit dashboard",
        ));
        gen.add_endpoint(
            ApiEndpoint::new(
                HttpMethod::Get,
                "/api/v1/enterprise/readiness",
                "Enterprise readiness report",
            )
            .with_description(
                "Summarizes runtime readiness, available enterprise controls, and recommended next actions.",
            )
            .with_tag("Enterprise")
            .with_response(
                ApiResponse::json(200, "Enterprise readiness report").with_example(
                    r#"{"version":"1.3.0","posture":"ready","score":78,"runtime":{"skills_registered":42,"active_connections":0,"active_sessions":0,"uptime_seconds":60},"checks":[],"next_actions":[]}"#,
                ),
            )
            .with_response(ApiResponse::internal_error()),
        );

        // Sessions
        gen.add_endpoint(
            ApiEndpoint::new(HttpMethod::Get, "/api/v1/sessions", "List sessions")
                .with_tag("Sessions")
                .with_response(ApiResponse::internal_error()),
        );
        gen.add_endpoint(
            ApiEndpoint::new(HttpMethod::Post, "/api/v1/sessions", "Create session")
                .with_tag("Sessions")
                .with_response(ApiResponse::bad_request("Invalid session payload"))
                .with_response(ApiResponse::internal_error()),
        );

        // Skills
        gen.add_endpoint(
            ApiEndpoint::new(HttpMethod::Get, "/api/v1/skills", "List skills")
                .with_tag("Skills")
                .with_response(ApiResponse::internal_error()),
        );

        // Audit
        gen.add_endpoint(
            ApiEndpoint::new(HttpMethod::Get, "/api/v1/audit/logs", "List audit log entries")
                .with_description("Returns recent audit JSONL entries. Use limit to cap the response size and cursor to continue from the x-next-cursor response header.")
                .with_parameter(ApiParameter::query("limit", "integer", "Maximum entries to return, capped at 1000"))
                .with_parameter(ApiParameter::query("cursor", "integer", "Byte offset from the x-next-cursor response header"))
                .with_tag("Audit")
                .with_response(ApiResponse::json(400, "Invalid cursor or query parameter").with_example(r#"{"error":"cursor must be an unsigned byte offset"}"#))
                .with_response(ApiResponse::json(500, "Audit log read failure").with_example(r#"{"error":"Failed to read audit log"}"#))
                .with_response(ApiResponse::json(200, "Audit log entries")),
        );
        gen.add_endpoint(
            ApiEndpoint::new(
                HttpMethod::Get,
                "/api/v1/audit/violations",
                "List audit violations",
            )
            .with_description(
                "Returns recent denied policy, guardrail, or violation entries from the audit log. Use cursor to continue from the x-next-cursor response header.",
            )
            .with_parameter(ApiParameter::query("limit", "integer", "Maximum entries to return, capped at 500"))
            .with_parameter(ApiParameter::query("cursor", "integer", "Byte offset from the x-next-cursor response header"))
            .with_tag("Audit")
            .with_response(ApiResponse::json(400, "Invalid cursor or query parameter").with_example(r#"{"error":"cursor must be an unsigned byte offset"}"#))
            .with_response(ApiResponse::json(500, "Audit log read failure").with_example(r#"{"error":"Failed to read audit log"}"#))
            .with_response(ApiResponse::json(200, "Audit violations")),
        );
        gen.add_endpoint(
            ApiEndpoint::new(HttpMethod::Get, "/api/v1/audit/stats", "Audit statistics")
                .with_description(
                    "Returns aggregate audit counts, success rate, and last event timestamp.",
                )
                .with_tag("Audit")
                .with_response(
                    ApiResponse::json(500, "Audit stats read failure")
                        .with_example(r#"{"error":"Failed to read audit stats"}"#),
                )
                .with_response(ApiResponse::json(200, "Audit statistics")),
        );

        // Control plane
        gen.add_endpoint(
            ApiEndpoint::new(
                HttpMethod::Get,
                "/api/v1/control-plane/deployments",
                "List deployments",
            )
            .with_tag("Control Plane")
            .requires_auth()
            .with_response(ApiResponse::unauthorized())
            .with_response(ApiResponse::internal_error()),
        );
        gen.add_endpoint(
            ApiEndpoint::new(
                HttpMethod::Post,
                "/api/v1/control-plane/deployments",
                "Create deployment",
            )
            .with_tag("Control Plane")
            .requires_auth()
            .with_response(ApiResponse::bad_request("Invalid deployment payload"))
            .with_response(ApiResponse::unauthorized())
            .with_response(ApiResponse::internal_error()),
        );

        gen
    }

    /// Get the number of endpoints.
    pub fn endpoint_count(&self) -> usize {
        self.endpoints.len()
    }
}

/// Generate the default Argentor OpenAPI spec as a JSON value.
pub fn argentor_openapi_spec() -> serde_json::Value {
    OpenApiGenerator::argentor_default().generate()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    // 1. Generate basic spec
    #[test]
    fn test_basic_spec() {
        let gen = OpenApiGenerator::new("Test API", "1.0.0");
        let spec = gen.generate();
        assert_eq!(spec["openapi"], "3.0.3");
        assert_eq!(spec["info"]["title"], "Test API");
    }

    // 2. Add endpoint
    #[test]
    fn test_add_endpoint() {
        let mut gen = OpenApiGenerator::new("Test", "1.0");
        gen.add_endpoint(ApiEndpoint::new(HttpMethod::Get, "/test", "Test endpoint"));
        let spec = gen.generate();
        assert!(spec["paths"]["/test"]["get"].is_object());
    }

    // 3. Endpoint with parameters
    #[test]
    fn test_endpoint_with_params() {
        let mut gen = OpenApiGenerator::new("Test", "1.0");
        gen.add_endpoint(
            ApiEndpoint::new(HttpMethod::Get, "/users/{id}", "Get user")
                .with_parameter(ApiParameter::path("id", "User ID"))
                .with_parameter(ApiParameter::query("fields", "string", "Fields to include")),
        );
        let spec = gen.generate();
        let params = spec["paths"]["/users/{id}"]["get"]["parameters"]
            .as_array()
            .unwrap();
        assert_eq!(params.len(), 2);
    }

    // 4. Endpoint with tags
    #[test]
    fn test_endpoint_tags() {
        let mut gen = OpenApiGenerator::new("Test", "1.0");
        gen.add_endpoint(
            ApiEndpoint::new(HttpMethod::Get, "/test", "Test")
                .with_tag("Users")
                .with_tag("Admin"),
        );
        let spec = gen.generate();
        let tags = spec["paths"]["/test"]["get"]["tags"].as_array().unwrap();
        assert_eq!(tags.len(), 2);
    }

    // 5. Auth required
    #[test]
    fn test_auth_required() {
        let mut gen = OpenApiGenerator::new("Test", "1.0");
        gen.add_endpoint(ApiEndpoint::new(HttpMethod::Post, "/secure", "Secure").requires_auth());
        let spec = gen.generate();
        assert!(spec["paths"]["/secure"]["post"]["security"].is_array());
    }

    // 6. Multiple methods on same path
    #[test]
    fn test_multiple_methods() {
        let mut gen = OpenApiGenerator::new("Test", "1.0");
        gen.add_endpoint(ApiEndpoint::new(HttpMethod::Get, "/items", "List items"));
        gen.add_endpoint(ApiEndpoint::new(HttpMethod::Post, "/items", "Create item"));
        let spec = gen.generate();
        assert!(spec["paths"]["/items"]["get"].is_object());
        assert!(spec["paths"]["/items"]["post"].is_object());
    }

    // 7. Server URL
    #[test]
    fn test_server_url() {
        let gen = OpenApiGenerator::new("Test", "1.0").with_server("https://api.example.com");
        let spec = gen.generate();
        assert_eq!(spec["servers"][0]["url"], "https://api.example.com");
    }

    // 8. Security schemes included
    #[test]
    fn test_security_schemes() {
        let gen = OpenApiGenerator::new("Test", "1.0");
        let spec = gen.generate();
        assert!(spec["components"]["securitySchemes"]["bearerAuth"].is_object());
        assert!(spec["components"]["securitySchemes"]["apiKey"].is_object());
    }

    // 9. Generate JSON string
    #[test]
    fn test_generate_json() {
        let gen = OpenApiGenerator::new("Test", "1.0");
        let json = gen.generate_json();
        assert!(json.contains("\"openapi\": \"3.0.3\""));
    }

    // 10. Argentor default spec
    #[test]
    fn test_argentor_default() {
        let gen = OpenApiGenerator::argentor_default();
        assert!(gen.endpoint_count() >= 7);
        let spec = gen.generate();
        assert_eq!(spec["info"]["title"], "Argentor API");
    }

    // 11. Convenience function
    #[test]
    fn test_argentor_openapi_spec() {
        let spec = argentor_openapi_spec();
        assert_eq!(spec["openapi"], "3.0.3");
    }

    // 11b. Every authenticated endpoint documents 401 explicitly.
    #[test]
    fn test_argentor_default_auth_endpoints_document_401() {
        let gen = OpenApiGenerator::argentor_default();
        let auth_endpoints: Vec<_> = gen.endpoints.iter().filter(|e| e.auth_required).collect();
        assert!(
            !auth_endpoints.is_empty(),
            "expected at least one auth-required endpoint"
        );
        for ep in auth_endpoints {
            assert!(
                ep.responses.iter().any(|r| r.status_code == 401),
                "endpoint {} {} requires auth but does not document a 401 response",
                format!("{:?}", ep.method),
                ep.path
            );
        }
    }

    // 11c. Mutating API endpoints document a 500 fallback.
    #[test]
    fn test_argentor_default_mutating_endpoints_document_500() {
        let gen = OpenApiGenerator::argentor_default();
        let mutating: Vec<_> = gen
            .endpoints
            .iter()
            .filter(|e| {
                matches!(
                    e.method,
                    HttpMethod::Post | HttpMethod::Put | HttpMethod::Delete | HttpMethod::Patch
                ) && e.path.starts_with("/api/v1/")
            })
            .collect();
        assert!(
            !mutating.is_empty(),
            "expected at least one mutating /api/v1 endpoint"
        );
        for ep in mutating {
            assert!(
                ep.responses.iter().any(|r| r.status_code == 500),
                "mutating endpoint {} {} does not document a 500 response",
                format!("{:?}", ep.method),
                ep.path
            );
        }
    }

    // 11d. ApiResponse error helpers emit the conventional shape.
    #[test]
    fn test_api_response_error_helpers() {
        let r = ApiResponse::unauthorized();
        assert_eq!(r.status_code, 401);
        assert_eq!(r.content_type.as_deref(), Some("application/json"));
        assert!(r.example.as_deref().unwrap().contains("error"));

        let r = ApiResponse::internal_error();
        assert_eq!(r.status_code, 500);

        let r = ApiResponse::bad_request("nope");
        assert_eq!(r.status_code, 400);
        assert_eq!(r.description, "nope");
    }

    // 12. Response with example
    #[test]
    fn test_response_with_example() {
        let mut gen = OpenApiGenerator::new("Test", "1.0");
        gen.add_endpoint(
            ApiEndpoint::new(HttpMethod::Get, "/status", "Status")
                .with_response(ApiResponse::json(200, "OK").with_example(r#"{"status": "ok"}"#)),
        );
        let spec = gen.generate();
        assert!(spec["paths"]["/status"]["get"]["responses"]["200"].is_object());
    }

    // 13. Endpoint count
    #[test]
    fn test_endpoint_count() {
        let mut gen = OpenApiGenerator::new("Test", "1.0");
        assert_eq!(gen.endpoint_count(), 0);
        gen.add_endpoint(ApiEndpoint::new(HttpMethod::Get, "/a", "A"));
        gen.add_endpoint(ApiEndpoint::new(HttpMethod::Get, "/b", "B"));
        assert_eq!(gen.endpoint_count(), 2);
    }

    // 14. ApiEndpoint serializable
    #[test]
    fn test_endpoint_serializable() {
        let ep = ApiEndpoint::new(HttpMethod::Get, "/test", "Test");
        let json = serde_json::to_string(&ep).unwrap();
        assert!(json.contains("\"method\":\"get\""));
    }

    // 15. HttpMethod as_str
    #[test]
    fn test_http_method() {
        assert_eq!(HttpMethod::Get.as_str(), "get");
        assert_eq!(HttpMethod::Post.as_str(), "post");
        assert_eq!(HttpMethod::Put.as_str(), "put");
        assert_eq!(HttpMethod::Delete.as_str(), "delete");
        assert_eq!(HttpMethod::Patch.as_str(), "patch");
    }

    // 16. Path parameter
    #[test]
    fn test_path_parameter() {
        let p = ApiParameter::path("id", "Resource ID");
        assert_eq!(p.location, ParameterLocation::Path);
        assert!(p.required);
    }

    // 17. Query parameter
    #[test]
    fn test_query_parameter() {
        let p = ApiParameter::query("limit", "integer", "Max results");
        assert_eq!(p.location, ParameterLocation::Query);
        assert!(!p.required);
    }

    // 18. Description
    #[test]
    fn test_description() {
        let gen = OpenApiGenerator::new("Test", "1.0").with_description("My API");
        let spec = gen.generate();
        assert_eq!(spec["info"]["description"], "My API");
    }

    // 19. Endpoint with description
    #[test]
    fn test_endpoint_description() {
        let mut gen = OpenApiGenerator::new("Test", "1.0");
        gen.add_endpoint(
            ApiEndpoint::new(HttpMethod::Get, "/test", "Test")
                .with_description("Detailed description"),
        );
        let spec = gen.generate();
        assert_eq!(
            spec["paths"]["/test"]["get"]["description"],
            "Detailed description"
        );
    }

    // 20. Empty paths when no endpoints
    #[test]
    fn test_empty_paths() {
        let gen = OpenApiGenerator::new("Test", "1.0");
        let spec = gen.generate();
        assert!(spec["paths"].as_object().unwrap().is_empty());
    }
}
