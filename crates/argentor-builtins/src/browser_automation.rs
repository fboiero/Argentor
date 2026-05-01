//! Browser automation skill using WebDriver (fantoccini).
//!
//! This module provides a full browser automation capability for agents, allowing
//! them to navigate pages, take screenshots, extract text from elements, fill forms,
//! click elements, and retrieve page source.
//!
//! The actual browser interaction requires the `browser` feature flag (which pulls in
//! the `fantoccini` crate and a running WebDriver server). Configuration structs,
//! action types, and the skill wrapper are always available.

use argentor_core::{ArgentorError, ArgentorResult, ToolCall, ToolResult};
use argentor_security::Capability;
use argentor_skills::skill::{Skill, SkillDescriptor};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

#[cfg(feature = "browser")]
use {
    std::sync::Arc,
    tracing::{debug, info},
};

// ---------------------------------------------------------------------------
// Configuration (always available)
// ---------------------------------------------------------------------------

/// Configuration for the browser automation WebDriver connection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserConfig {
    /// WebDriver server URL (default: "http://localhost:4444").
    #[serde(default = "default_webdriver_url")]
    pub webdriver_url: String,

    /// Run the browser in headless mode (default: true).
    #[serde(default = "default_headless")]
    pub headless: bool,

    /// Timeout in seconds for browser operations (default: 30).
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,

    /// Directory to save screenshots. If None, screenshots are saved to a temp dir.
    #[serde(default)]
    pub screenshot_dir: Option<String>,
}

fn default_webdriver_url() -> String {
    "http://localhost:4444".to_string()
}

fn default_headless() -> bool {
    true
}

fn default_timeout_secs() -> u64 {
    30
}

impl Default for BrowserConfig {
    fn default() -> Self {
        Self {
            webdriver_url: "http://localhost:4444".to_string(),
            headless: true,
            timeout_secs: 30,
            screenshot_dir: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Action & Result types (always available)
// ---------------------------------------------------------------------------

/// Represents a browser action to perform.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum BrowserAction {
    /// Navigate the browser to a URL.
    Navigate {
        /// Target URL to navigate to.
        url: String,
    },
    /// Take a screenshot of the current page.
    Screenshot,
    /// Extract text content from an element matching the CSS selector.
    ExtractText {
        /// CSS selector for the target element.
        selector: String,
    },
    /// Fill a form field matching the CSS selector with the given value.
    FillForm {
        /// CSS selector for the form field.
        selector: String,
        /// Value to fill into the field.
        value: String,
    },
    /// Click an element matching the CSS selector.
    Click {
        /// CSS selector for the element to click.
        selector: String,
    },
    /// Get the full page source HTML.
    GetPageSource,
}

/// Result of a browser automation action.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserResult {
    /// Whether the action completed successfully.
    pub success: bool,
    /// Text content, file path, page source, or other data returned by the action.
    pub data: Option<String>,
    /// Error message if the action failed.
    pub error: Option<String>,
}

impl BrowserResult {
    /// Create a successful result with the given data.
    #[cfg_attr(not(feature = "browser"), allow(dead_code))]
    fn ok(data: impl Into<String>) -> Self {
        Self {
            success: true,
            data: Some(data.into()),
            error: None,
        }
    }

    /// Create a failed result with the given error message.
    #[cfg_attr(not(feature = "browser"), allow(dead_code))]
    fn err(error: impl Into<String>) -> Self {
        Self {
            success: false,
            data: None,
            error: Some(error.into()),
        }
    }
}

// ---------------------------------------------------------------------------
// BrowserAutomation (requires `browser` feature)
// ---------------------------------------------------------------------------

/// Full browser automation client backed by fantoccini / WebDriver.
#[cfg(feature = "browser")]
pub struct BrowserAutomation {
    pub config: BrowserConfig,
    client: fantoccini::Client,
}

#[cfg(feature = "browser")]
impl BrowserAutomation {
    /// Connect to the WebDriver server and create a new browser session.
    pub async fn new(config: BrowserConfig) -> ArgentorResult<Self> {
        use fantoccini::ClientBuilder;

        let mut caps = serde_json::Map::new();

        // Configure headless mode via browser-specific options.
        if config.headless {
            let args = serde_json::json!(["--headless", "--disable-gpu", "--no-sandbox"]);
            let chrome_opts = serde_json::json!({ "args": args });
            caps.insert("goog:chromeOptions".to_string(), chrome_opts);

            let firefox_opts = serde_json::json!({ "args": ["-headless"] });
            caps.insert("moz:firefoxOptions".to_string(), firefox_opts);
        }

        let client = ClientBuilder::native()
            .capabilities(caps)
            .connect(&config.webdriver_url)
            .await
            .map_err(|e| {
                ArgentorError::Skill(format!(
                    "Failed to connect to WebDriver at {}: {}",
                    config.webdriver_url, e
                ))
            })?;

        info!(webdriver_url = %config.webdriver_url, "Browser automation connected");

        Ok(Self { config, client })
    }

    /// Navigate the browser to the given URL.
    pub async fn navigate(&self, url: &str) -> ArgentorResult<BrowserResult> {
        self.client
            .goto(url)
            .await
            .map_err(|e| ArgentorError::Skill(format!("Navigation to '{}' failed: {}", url, e)))?;

        let current_url = self
            .client
            .current_url()
            .await
            .map(|u| u.to_string())
            .unwrap_or_else(|_| url.to_string());

        debug!(url = %current_url, "Navigated");

        Ok(BrowserResult::ok(format!("Navigated to {}", current_url)))
    }

    /// Take a screenshot and save it. Returns the file path.
    pub async fn screenshot(&self) -> ArgentorResult<BrowserResult> {
        let png_data = self
            .client
            .screenshot()
            .await
            .map_err(|e| ArgentorError::Skill(format!("Screenshot failed: {}", e)))?;

        let dir = self
            .config
            .screenshot_dir
            .clone()
            .unwrap_or_else(|| std::env::temp_dir().to_string_lossy().to_string());

        std::fs::create_dir_all(&dir).map_err(|e| {
            ArgentorError::Skill(format!("Failed to create screenshot dir '{}': {}", dir, e))
        })?;

        let filename = format!(
            "screenshot_{}.png",
            chrono::Utc::now().format("%Y%m%d_%H%M%S")
        );
        let path = std::path::Path::new(&dir).join(&filename);

        std::fs::write(&path, &png_data)
            .map_err(|e| ArgentorError::Skill(format!("Failed to write screenshot: {}", e)))?;

        let path_str = path.to_string_lossy().to_string();
        info!(path = %path_str, "Screenshot saved");

        Ok(BrowserResult::ok(path_str))
    }

    /// Extract the text content of the first element matching the CSS selector.
    pub async fn extract_text(&self, selector: &str) -> ArgentorResult<BrowserResult> {
        use fantoccini::Locator;

        let element = self
            .client
            .find(Locator::Css(selector))
            .await
            .map_err(|e| {
                ArgentorError::Skill(format!(
                    "Element not found for selector '{}': {}",
                    selector, e
                ))
            })?;

        let text = element.text().await.map_err(|e| {
            ArgentorError::Skill(format!("Failed to extract text from '{}': {}", selector, e))
        })?;

        debug!(selector, text_len = text.len(), "Text extracted");

        Ok(BrowserResult::ok(text))
    }

    /// Fill a form field matching the CSS selector with the given value.
    pub async fn fill_form(&self, selector: &str, value: &str) -> ArgentorResult<BrowserResult> {
        use fantoccini::Locator;

        let element = self
            .client
            .find(Locator::Css(selector))
            .await
            .map_err(|e| {
                ArgentorError::Skill(format!(
                    "Form field not found for selector '{}': {}",
                    selector, e
                ))
            })?;

        element.clear().await.map_err(|e| {
            ArgentorError::Skill(format!("Failed to clear field '{}': {}", selector, e))
        })?;

        element.send_keys(value).await.map_err(|e| {
            ArgentorError::Skill(format!(
                "Failed to fill field '{}' with value: {}",
                selector, e
            ))
        })?;

        debug!(selector, "Form field filled");

        Ok(BrowserResult::ok(format!(
            "Filled '{}' with value",
            selector
        )))
    }

    /// Click the first element matching the CSS selector.
    pub async fn click(&self, selector: &str) -> ArgentorResult<BrowserResult> {
        use fantoccini::Locator;

        let element = self
            .client
            .find(Locator::Css(selector))
            .await
            .map_err(|e| {
                ArgentorError::Skill(format!(
                    "Element not found for selector '{}': {}",
                    selector, e
                ))
            })?;

        element
            .click()
            .await
            .map_err(|e| ArgentorError::Skill(format!("Click on '{}' failed: {}", selector, e)))?;

        debug!(selector, "Element clicked");

        Ok(BrowserResult::ok(format!("Clicked '{}'", selector)))
    }

    /// Get the full page source HTML.
    pub async fn get_page_source(&self) -> ArgentorResult<BrowserResult> {
        let source = self
            .client
            .source()
            .await
            .map_err(|e| ArgentorError::Skill(format!("Failed to get page source: {}", e)))?;

        debug!(source_len = source.len(), "Page source retrieved");

        Ok(BrowserResult::ok(source))
    }

    /// Close the browser session and release WebDriver resources.
    pub async fn close(self) -> ArgentorResult<()> {
        self.client
            .close()
            .await
            .map_err(|e| ArgentorError::Skill(format!("Failed to close browser: {}", e)))?;

        info!("Browser session closed");

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Action parser (always available)
// ---------------------------------------------------------------------------

/// Parse a `BrowserAction` from the JSON arguments of a `ToolCall`.
pub fn parse_action(args: &serde_json::Value) -> ArgentorResult<BrowserAction> {
    let action_str = args["action"]
        .as_str()
        .ok_or_else(|| ArgentorError::Skill("Missing or invalid 'action' field".to_string()))?;

    match action_str {
        "navigate" => {
            let url = args["url"]
                .as_str()
                .ok_or_else(|| {
                    ArgentorError::Skill(
                        "Navigate action requires a 'url' field".to_string(),
                    )
                })?
                .to_string();
            Ok(BrowserAction::Navigate { url })
        }
        "screenshot" => Ok(BrowserAction::Screenshot),
        "extract_text" => {
            let selector = args["selector"]
                .as_str()
                .ok_or_else(|| {
                    ArgentorError::Skill(
                        "extract_text action requires a 'selector' field".to_string(),
                    )
                })?
                .to_string();
            Ok(BrowserAction::ExtractText { selector })
        }
        "fill_form" => {
            let selector = args["selector"]
                .as_str()
                .ok_or_else(|| {
                    ArgentorError::Skill(
                        "fill_form action requires a 'selector' field".to_string(),
                    )
                })?
                .to_string();
            let value = args["value"]
                .as_str()
                .ok_or_else(|| {
                    ArgentorError::Skill(
                        "fill_form action requires a 'value' field".to_string(),
                    )
                })?
                .to_string();
            Ok(BrowserAction::FillForm { selector, value })
        }
        "click" => {
            let selector = args["selector"]
                .as_str()
                .ok_or_else(|| {
                    ArgentorError::Skill(
                        "click action requires a 'selector' field".to_string(),
                    )
                })?
                .to_string();
            Ok(BrowserAction::Click { selector })
        }
        "get_page_source" => Ok(BrowserAction::GetPageSource),
        unknown => Err(ArgentorError::Skill(format!(
            "Unknown browser action: '{unknown}'. Valid actions: navigate, screenshot, extract_text, fill_form, click, get_page_source"
        ))),
    }
}

// ---------------------------------------------------------------------------
// BrowserAutomationSkill (Skill trait implementation)
// ---------------------------------------------------------------------------

/// Skill wrapper that exposes browser automation to agents via the Skill trait.
pub struct BrowserAutomationSkill {
    descriptor: SkillDescriptor,
    #[cfg(feature = "browser")]
    client: Arc<tokio::sync::Mutex<Option<fantoccini::Client>>>,
    #[cfg_attr(not(feature = "browser"), allow(dead_code))]
    config: BrowserConfig,
}

impl BrowserAutomationSkill {
    /// Create a new `BrowserAutomationSkill` with the given configuration.
    ///
    /// The actual WebDriver connection is established lazily on first use
    /// (when the `browser` feature is enabled).
    pub fn new(config: BrowserConfig) -> Self {
        Self {
            descriptor: SkillDescriptor {
                name: "browser_automation".to_string(),
                description:
                    "Automate web browser actions: navigate, screenshot, extract text, fill forms, click elements"
                        .to_string(),
                parameters_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "action": {
                            "type": "string",
                            "enum": ["navigate", "screenshot", "extract_text", "fill_form", "click", "get_page_source"]
                        },
                        "url": { "type": "string" },
                        "selector": { "type": "string" },
                        "value": { "type": "string" }
                    },
                    "required": ["action"]
                }),
                required_capabilities: vec![Capability::NetworkAccess {
                    allowed_hosts: vec!["*".into()],
                }],
                requires_approval: false,
            },
            #[cfg(feature = "browser")]
            client: Arc::new(tokio::sync::Mutex::new(None)),
            config,
        }
    }

    /// Ensure a WebDriver client is connected, creating one if needed.
    #[cfg(feature = "browser")]
    async fn ensure_client(
        &self,
    ) -> ArgentorResult<tokio::sync::MutexGuard<'_, Option<fantoccini::Client>>> {
        let mut guard = self.client.lock().await;
        if guard.is_none() {
            use fantoccini::ClientBuilder;

            let mut caps = serde_json::Map::new();

            if self.config.headless {
                let args = serde_json::json!(["--headless", "--disable-gpu", "--no-sandbox"]);
                let chrome_opts = serde_json::json!({ "args": args });
                caps.insert("goog:chromeOptions".to_string(), chrome_opts);

                let firefox_opts = serde_json::json!({ "args": ["-headless"] });
                caps.insert("moz:firefoxOptions".to_string(), firefox_opts);
            }

            let client = ClientBuilder::native()
                .capabilities(caps)
                .connect(&self.config.webdriver_url)
                .await
                .map_err(|e| {
                    ArgentorError::Skill(format!(
                        "Failed to connect to WebDriver at {}: {}",
                        self.config.webdriver_url, e
                    ))
                })?;

            info!(webdriver_url = %self.config.webdriver_url, "Browser automation skill connected");

            *guard = Some(client);
        }
        Ok(guard)
    }
}

#[async_trait]
impl Skill for BrowserAutomationSkill {
    fn descriptor(&self) -> &SkillDescriptor {
        &self.descriptor
    }

    async fn execute(&self, call: ToolCall) -> ArgentorResult<ToolResult> {
        let action = match parse_action(&call.arguments) {
            Ok(a) => a,
            Err(e) => {
                return Ok(ToolResult::error(&call.id, e.to_string()));
            }
        };

        // Without the browser feature, we cannot perform actual browser actions.
        #[cfg(not(feature = "browser"))]
        {
            let _ = action;
            return Ok(ToolResult::error(
                &call.id,
                "Browser automation requires the 'browser' feature flag",
            ));
        }

        #[cfg(feature = "browser")]
        {
            use fantoccini::Locator;

            let guard = self.ensure_client().await?;
            // Safety: ensure_client() guarantees the Option is Some when it
            // returns Ok. If the invariant is somehow broken, surface a clear
            // error instead of panicking.
            let client = guard.as_ref().ok_or_else(|| {
                ArgentorError::Skill(
                    "BrowserAutomationSkill: client missing after ensure_client".to_string(),
                )
            })?;

            let result: BrowserResult = match action {
                BrowserAction::Navigate { url } => match client.goto(&url).await {
                    Ok(()) => {
                        let current = client
                            .current_url()
                            .await
                            .map(|u| u.to_string())
                            .unwrap_or_else(|_| url.clone());
                        BrowserResult::ok(format!("Navigated to {}", current))
                    }
                    Err(e) => BrowserResult::err(format!("Navigation failed: {}", e)),
                },
                BrowserAction::Screenshot => match client.screenshot().await {
                    Ok(png_data) => {
                        let dir =
                            self.config.screenshot_dir.clone().unwrap_or_else(|| {
                                std::env::temp_dir().to_string_lossy().to_string()
                            });

                        if let Err(e) = std::fs::create_dir_all(&dir) {
                            BrowserResult::err(format!("Failed to create screenshot dir: {}", e))
                        } else {
                            let filename = format!(
                                "screenshot_{}.png",
                                chrono::Utc::now().format("%Y%m%d_%H%M%S")
                            );
                            let path = std::path::Path::new(&dir).join(&filename);
                            match std::fs::write(&path, &png_data) {
                                Ok(()) => BrowserResult::ok(path.to_string_lossy().to_string()),
                                Err(e) => {
                                    BrowserResult::err(format!("Failed to write screenshot: {}", e))
                                }
                            }
                        }
                    }
                    Err(e) => BrowserResult::err(format!("Screenshot failed: {}", e)),
                },
                BrowserAction::ExtractText { selector } => {
                    match client.find(Locator::Css(&selector)).await {
                        Ok(elem) => match elem.text().await {
                            Ok(text) => BrowserResult::ok(text),
                            Err(e) => BrowserResult::err(format!("Failed to extract text: {}", e)),
                        },
                        Err(e) => {
                            BrowserResult::err(format!("Element not found '{}': {}", selector, e))
                        }
                    }
                }
                BrowserAction::FillForm { selector, value } => {
                    match client.find(Locator::Css(&selector)).await {
                        Ok(elem) => {
                            if let Err(e) = elem.clear().await {
                                BrowserResult::err(format!("Failed to clear field: {}", e))
                            } else if let Err(e) = elem.send_keys(&value).await {
                                BrowserResult::err(format!("Failed to fill field: {}", e))
                            } else {
                                BrowserResult::ok(format!("Filled '{}' with value", selector))
                            }
                        }
                        Err(e) => BrowserResult::err(format!(
                            "Form field not found '{}': {}",
                            selector, e
                        )),
                    }
                }
                BrowserAction::Click { selector } => {
                    match client.find(Locator::Css(&selector)).await {
                        Ok(elem) => match elem.click().await {
                            Ok(()) => BrowserResult::ok(format!("Clicked '{}'", selector)),
                            Err(e) => BrowserResult::err(format!("Click failed: {}", e)),
                        },
                        Err(e) => {
                            BrowserResult::err(format!("Element not found '{}': {}", selector, e))
                        }
                    }
                }
                BrowserAction::GetPageSource => match client.source().await {
                    Ok(source) => BrowserResult::ok(source),
                    Err(e) => BrowserResult::err(format!("Failed to get page source: {}", e)),
                },
            };

            // Drop the guard before building the response.
            drop(guard);

            let json = serde_json::to_string(&result).map_err(|e| {
                ArgentorError::Skill(format!("Failed to serialize BrowserResult: {}", e))
            })?;

            if result.success {
                Ok(ToolResult::success(&call.id, json))
            } else {
                Ok(ToolResult::error(&call.id, json))
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn test_browser_config_defaults() {
        let config = BrowserConfig::default();
        assert_eq!(config.webdriver_url, "http://localhost:4444");
        assert!(config.headless);
        assert_eq!(config.timeout_secs, 30);
        assert!(config.screenshot_dir.is_none());
    }

    #[test]
    fn test_browser_config_serde_roundtrip() {
        let config = BrowserConfig {
            webdriver_url: "http://localhost:9515".to_string(),
            headless: false,
            timeout_secs: 60,
            screenshot_dir: Some("/tmp/shots".to_string()),
        };

        let json = serde_json::to_string(&config).unwrap();
        let deserialized: BrowserConfig = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.webdriver_url, "http://localhost:9515");
        assert!(!deserialized.headless);
        assert_eq!(deserialized.timeout_secs, 60);
        assert_eq!(deserialized.screenshot_dir, Some("/tmp/shots".to_string()));
    }

    #[test]
    fn test_browser_config_deserialize_with_defaults() {
        let json = r#"{}"#;
        let config: BrowserConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.webdriver_url, "http://localhost:4444");
        assert!(config.headless);
        assert_eq!(config.timeout_secs, 30);
        assert!(config.screenshot_dir.is_none());
    }

    #[test]
    fn test_skill_descriptor() {
        let skill = BrowserAutomationSkill::new(BrowserConfig::default());
        assert_eq!(skill.descriptor().name, "browser_automation");
        assert!(skill
            .descriptor()
            .description
            .contains("Automate web browser"));
        assert_eq!(skill.descriptor().required_capabilities.len(), 1);

        // Validate JSON schema structure.
        let schema = &skill.descriptor().parameters_schema;
        let props = &schema["properties"];
        assert!(props["action"]["type"].as_str() == Some("string"));
        let required = schema["required"].as_array().unwrap();
        assert!(required.contains(&serde_json::json!("action")));
    }

    #[test]
    fn test_parse_navigate_action() {
        let args = serde_json::json!({"action": "navigate", "url": "https://example.com"});
        let action = parse_action(&args).unwrap();
        assert!(matches!(action, BrowserAction::Navigate { url } if url == "https://example.com"));
    }

    #[test]
    fn test_parse_screenshot_action() {
        let args = serde_json::json!({"action": "screenshot"});
        let action = parse_action(&args).unwrap();
        assert!(matches!(action, BrowserAction::Screenshot));
    }

    #[test]
    fn test_parse_extract_text_action() {
        let args = serde_json::json!({"action": "extract_text", "selector": "h1.title"});
        let action = parse_action(&args).unwrap();
        assert!(
            matches!(action, BrowserAction::ExtractText { selector } if selector == "h1.title")
        );
    }

    #[test]
    fn test_parse_fill_form_action() {
        let args = serde_json::json!({"action": "fill_form", "selector": "#email", "value": "test@example.com"});
        let action = parse_action(&args).unwrap();
        assert!(
            matches!(action, BrowserAction::FillForm { selector, value } if selector == "#email" && value == "test@example.com")
        );
    }

    #[test]
    fn test_parse_click_action() {
        let args = serde_json::json!({"action": "click", "selector": "button.submit"});
        let action = parse_action(&args).unwrap();
        assert!(matches!(action, BrowserAction::Click { selector } if selector == "button.submit"));
    }

    #[test]
    fn test_parse_get_page_source_action() {
        let args = serde_json::json!({"action": "get_page_source"});
        let action = parse_action(&args).unwrap();
        assert!(matches!(action, BrowserAction::GetPageSource));
    }

    #[test]
    fn test_parse_unknown_action() {
        let args = serde_json::json!({"action": "fly"});
        assert!(parse_action(&args).is_err());
        let err = parse_action(&args).unwrap_err().to_string();
        assert!(err.contains("Unknown browser action"));
        assert!(err.contains("fly"));
    }

    #[test]
    fn test_parse_missing_action_field() {
        let args = serde_json::json!({"url": "https://example.com"});
        assert!(parse_action(&args).is_err());
    }

    #[test]
    fn test_parse_navigate_missing_url() {
        let args = serde_json::json!({"action": "navigate"});
        assert!(parse_action(&args).is_err());
        let err = parse_action(&args).unwrap_err().to_string();
        assert!(err.contains("url"));
    }

    #[test]
    fn test_parse_extract_text_missing_selector() {
        let args = serde_json::json!({"action": "extract_text"});
        assert!(parse_action(&args).is_err());
        let err = parse_action(&args).unwrap_err().to_string();
        assert!(err.contains("selector"));
    }

    #[test]
    fn test_parse_click_missing_selector() {
        let args = serde_json::json!({"action": "click"});
        assert!(parse_action(&args).is_err());
    }

    #[test]
    fn test_parse_fill_form_missing_selector() {
        let args = serde_json::json!({"action": "fill_form", "value": "hello"});
        assert!(parse_action(&args).is_err());
    }

    #[test]
    fn test_parse_fill_form_missing_value() {
        let args = serde_json::json!({"action": "fill_form", "selector": "#input"});
        assert!(parse_action(&args).is_err());
    }

    #[test]
    fn test_browser_result_ok() {
        let result = BrowserResult::ok("some data");
        assert!(result.success);
        assert_eq!(result.data, Some("some data".to_string()));
        assert!(result.error.is_none());
    }

    #[test]
    fn test_browser_result_err() {
        let result = BrowserResult::err("something failed");
        assert!(!result.success);
        assert!(result.data.is_none());
        assert_eq!(result.error, Some("something failed".to_string()));
    }

    #[test]
    fn test_browser_result_serde_roundtrip() {
        let result = BrowserResult {
            success: true,
            data: Some("page content".to_string()),
            error: None,
        };

        let json = serde_json::to_string(&result).unwrap();
        let deserialized: BrowserResult = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.success, result.success);
        assert_eq!(deserialized.data, result.data);
        assert_eq!(deserialized.error, result.error);
    }

    #[tokio::test]
    async fn test_skill_execute_without_browser_feature() {
        // Without the "browser" feature, execute should return an error
        // telling the user the feature flag is required.
        // Note: this test only exercises the non-browser path;
        // the browser path requires a running WebDriver.
        let skill = BrowserAutomationSkill::new(BrowserConfig::default());
        let call = ToolCall {
            id: "test-1".to_string(),
            name: "browser_automation".to_string(),
            arguments: serde_json::json!({"action": "navigate", "url": "https://example.com"}),
        };

        let result = skill.execute(call).await.unwrap();

        // When built without the feature, we expect an error result.
        #[cfg(not(feature = "browser"))]
        {
            assert!(result.is_error);
            assert!(result.content.contains("browser"));
        }

        // When built with the feature, we expect a connection error
        // (no WebDriver running in tests), which is also an error result.
        #[cfg(feature = "browser")]
        {
            assert!(result.is_error);
        }
    }

    #[tokio::test]
    async fn test_skill_execute_invalid_action() {
        let skill = BrowserAutomationSkill::new(BrowserConfig::default());
        let call = ToolCall {
            id: "test-2".to_string(),
            name: "browser_automation".to_string(),
            arguments: serde_json::json!({"action": "teleport"}),
        };

        let result = skill.execute(call).await.unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("Unknown browser action"));
    }
}
