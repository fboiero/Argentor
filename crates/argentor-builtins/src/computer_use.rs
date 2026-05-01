// SPDX-License-Identifier: AGPL-3.0-only
//! Computer-use skill — screenshot, mouse, keyboard, scroll via platform tools.
//!
//! Supported actions (matches Claude `computer_20241022` tool contract):
//! - `screenshot` — capture full screen, return base64 PNG
//! - `mouse_move` — move cursor to `[x, y]`
//! - `left_click` — left-click at `[x, y]`
//! - `right_click` — right-click at `[x, y]`
//! - `double_click` — double-click at `[x, y]`
//! - `type` — type a string
//! - `key` — press a key combination (e.g. `"Return"`, `"ctrl+c"`)
//! - `scroll_up` / `scroll_down` — scroll at `[x, y]`
//! - `cursor_position` — return current `[x, y]` cursor position
//!
//! # Platform support
//!
//! | Action          | macOS             | Linux                   |
//! |-----------------|-------------------|-------------------------|
//! | screenshot      | `screencapture`   | `scrot` / `gnome-screenshot` |
//! | mouse / click   | `cliclick`        | `xdotool`               |
//! | type / key      | `cliclick`        | `xdotool`               |
//! | scroll          | `cliclick`        | `xdotool`               |
//!
//! # Safety
//!
//! - **Rate limit**: max [`ComputerUseConfig::max_actions_per_second`] calls per second
//!   (default 10). Excess calls return an error without executing.
//! - **Allowed regions**: if [`ComputerUseConfig::allowed_regions`] is non-empty,
//!   any coordinate-bearing action whose target falls outside every listed region
//!   is rejected.
//!
//! Enable with the `computer-use` crate feature.

use argentor_core::{ArgentorError, ArgentorResult, ToolCall, ToolResult};
use argentor_security::Capability;
use argentor_skills::skill::{Skill, SkillDescriptor};
use async_trait::async_trait;
use base64::Engine as _;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::process::Command;
use tokio::sync::Mutex;
use tracing::{debug, info};

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// A screen region expressed as `(x, y, width, height)` in pixels.
#[derive(Debug, Clone)]
pub struct ScreenRegion {
    /// Left edge (pixels from left of screen).
    pub x: i32,
    /// Top edge (pixels from top of screen).
    pub y: i32,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
}

impl ScreenRegion {
    /// Return `true` if the point `(px, py)` falls within this region.
    pub fn contains(&self, px: i32, py: i32) -> bool {
        px >= self.x
            && py >= self.y
            && px < self.x + self.width as i32
            && py < self.y + self.height as i32
    }
}

/// Configuration for [`ComputerUseSkill`].
#[derive(Debug, Clone)]
pub struct ComputerUseConfig {
    /// Maximum number of actions allowed per second (default: 10).
    pub max_actions_per_second: u32,
    /// Optional list of allowed screen regions.
    /// If empty, the entire screen is allowed.
    pub allowed_regions: Vec<ScreenRegion>,
}

impl Default for ComputerUseConfig {
    fn default() -> Self {
        Self {
            max_actions_per_second: 10,
            allowed_regions: Vec::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// ScreenController trait
// ---------------------------------------------------------------------------

/// Platform-agnostic screen controller.
///
/// Each method maps to a single computer-use action. Implementations call
/// native platform tools and return output as strings (or base64 for
/// screenshots).
#[async_trait]
pub trait ScreenController: Send + Sync {
    /// Capture the full screen and return base64-encoded PNG bytes.
    async fn screenshot(&self) -> ArgentorResult<String>;

    /// Move the cursor to `(x, y)`.
    async fn mouse_move(&self, x: i32, y: i32) -> ArgentorResult<()>;

    /// Left-click at `(x, y)`.
    async fn left_click(&self, x: i32, y: i32) -> ArgentorResult<()>;

    /// Right-click at `(x, y)`.
    async fn right_click(&self, x: i32, y: i32) -> ArgentorResult<()>;

    /// Double-click at `(x, y)`.
    async fn double_click(&self, x: i32, y: i32) -> ArgentorResult<()>;

    /// Type the given text string.
    async fn type_text(&self, text: &str) -> ArgentorResult<()>;

    /// Press a key or key combination (e.g. `"Return"`, `"ctrl+c"`).
    async fn key(&self, key: &str) -> ArgentorResult<()>;

    /// Scroll up at `(x, y)`.
    async fn scroll_up(&self, x: i32, y: i32) -> ArgentorResult<()>;

    /// Scroll down at `(x, y)`.
    async fn scroll_down(&self, x: i32, y: i32) -> ArgentorResult<()>;

    /// Return the current cursor position as `[x, y]`.
    async fn cursor_position(&self) -> ArgentorResult<(i32, i32)>;
}

// ---------------------------------------------------------------------------
// macOS implementation (screencapture + cliclick)
// ---------------------------------------------------------------------------

/// macOS [`ScreenController`] using `screencapture` and `cliclick`.
pub struct MacOsController;

#[async_trait]
impl ScreenController for MacOsController {
    async fn screenshot(&self) -> ArgentorResult<String> {
        // Write to a temp file, read back, base64-encode, remove.
        let path = std::env::temp_dir().join("argentor_screenshot.png");
        let path_str = path.to_string_lossy().to_string();

        let status = Command::new("screencapture")
            .args(["-x", "-t", "png", &path_str])
            .status()
            .await
            .map_err(|e| ArgentorError::Agent(format!("screencapture failed: {e}")))?;

        if !status.success() {
            return Err(ArgentorError::Agent(format!(
                "screencapture exited with {:?}",
                status.code()
            )));
        }

        let bytes = tokio::fs::read(&path)
            .await
            .map_err(|e| ArgentorError::Agent(format!("read screenshot file: {e}")))?;
        let _ = tokio::fs::remove_file(&path).await;

        Ok(base64::engine::general_purpose::STANDARD.encode(&bytes))
    }

    async fn mouse_move(&self, x: i32, y: i32) -> ArgentorResult<()> {
        run_cmd("cliclick", &[format!("m:{x},{y}")]).await
    }

    async fn left_click(&self, x: i32, y: i32) -> ArgentorResult<()> {
        run_cmd("cliclick", &[format!("c:{x},{y}")]).await
    }

    async fn right_click(&self, x: i32, y: i32) -> ArgentorResult<()> {
        run_cmd("cliclick", &[format!("rc:{x},{y}")]).await
    }

    async fn double_click(&self, x: i32, y: i32) -> ArgentorResult<()> {
        run_cmd("cliclick", &[format!("dc:{x},{y}")]).await
    }

    async fn type_text(&self, text: &str) -> ArgentorResult<()> {
        run_cmd("cliclick", &[format!("t:{text}")]).await
    }

    async fn key(&self, key: &str) -> ArgentorResult<()> {
        // cliclick uses kp: for key presses, with X11-like key names.
        run_cmd("cliclick", &[format!("kp:{key}")]).await
    }

    async fn scroll_up(&self, x: i32, y: i32) -> ArgentorResult<()> {
        // cliclick su: scrolls up N clicks at position
        run_cmd("cliclick", &[format!("su:10"), format!("m:{x},{y}")]).await
    }

    async fn scroll_down(&self, x: i32, y: i32) -> ArgentorResult<()> {
        run_cmd("cliclick", &[format!("sd:10"), format!("m:{x},{y}")]).await
    }

    async fn cursor_position(&self) -> ArgentorResult<(i32, i32)> {
        let out = run_cmd_output("cliclick", &["p:."]).await?;
        parse_xy_output(&out)
    }
}

// ---------------------------------------------------------------------------
// Linux implementation (scrot / gnome-screenshot + xdotool)
// ---------------------------------------------------------------------------

/// Linux [`ScreenController`] using `scrot` (or `gnome-screenshot`) and `xdotool`.
pub struct LinuxController;

#[async_trait]
impl ScreenController for LinuxController {
    async fn screenshot(&self) -> ArgentorResult<String> {
        let path = std::env::temp_dir().join("argentor_screenshot.png");
        let path_str = path.to_string_lossy().to_string();

        // Try scrot first, fall back to gnome-screenshot.
        let status = Command::new("scrot").args([&path_str]).status().await;

        let ok = match status {
            Ok(s) => s.success(),
            Err(_) => false,
        };

        if !ok {
            let status2 = Command::new("gnome-screenshot")
                .args(["-f", &path_str])
                .status()
                .await
                .map_err(|e| ArgentorError::Agent(format!("gnome-screenshot failed: {e}")))?;
            if !status2.success() {
                return Err(ArgentorError::Agent(
                    "Neither scrot nor gnome-screenshot succeeded".into(),
                ));
            }
        }

        let bytes = tokio::fs::read(&path)
            .await
            .map_err(|e| ArgentorError::Agent(format!("read screenshot file: {e}")))?;
        let _ = tokio::fs::remove_file(&path).await;

        Ok(base64::engine::general_purpose::STANDARD.encode(&bytes))
    }

    async fn mouse_move(&self, x: i32, y: i32) -> ArgentorResult<()> {
        run_cmd("xdotool", &[format!("mousemove {x} {y}")]).await
    }

    async fn left_click(&self, x: i32, y: i32) -> ArgentorResult<()> {
        run_cmd("xdotool", &[format!("mousemove {x} {y}"), "click 1".into()]).await
    }

    async fn right_click(&self, x: i32, y: i32) -> ArgentorResult<()> {
        run_cmd("xdotool", &[format!("mousemove {x} {y}"), "click 3".into()]).await
    }

    async fn double_click(&self, x: i32, y: i32) -> ArgentorResult<()> {
        run_cmd(
            "xdotool",
            &[format!("mousemove {x} {y}"), "click --repeat 2 1".into()],
        )
        .await
    }

    async fn type_text(&self, text: &str) -> ArgentorResult<()> {
        run_cmd("xdotool", &[format!("type -- {text}")]).await
    }

    async fn key(&self, key: &str) -> ArgentorResult<()> {
        run_cmd("xdotool", &[format!("key {key}")]).await
    }

    async fn scroll_up(&self, x: i32, y: i32) -> ArgentorResult<()> {
        run_cmd(
            "xdotool",
            &[format!("mousemove {x} {y}"), "click --repeat 3 4".into()],
        )
        .await
    }

    async fn scroll_down(&self, x: i32, y: i32) -> ArgentorResult<()> {
        run_cmd(
            "xdotool",
            &[format!("mousemove {x} {y}"), "click --repeat 3 5".into()],
        )
        .await
    }

    async fn cursor_position(&self) -> ArgentorResult<(i32, i32)> {
        let out = run_cmd_output("xdotool", &["getmouselocation"]).await?;
        // output: "x:123 y:456 screen:0 window:..."
        let x = out
            .split_whitespace()
            .find_map(|t| t.strip_prefix("x:").and_then(|v| v.parse().ok()))
            .unwrap_or(0);
        let y = out
            .split_whitespace()
            .find_map(|t| t.strip_prefix("y:").and_then(|v| v.parse().ok()))
            .unwrap_or(0);
        Ok((x, y))
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Run a command, discarding stdout, returning an error on non-zero exit.
async fn run_cmd(program: &str, args: &[String]) -> ArgentorResult<()> {
    let mut cmd = Command::new(program);
    for arg in args {
        // Each arg may be a single "word word" string for tools like xdotool —
        // we pass them via sh to preserve spaces.
        cmd.arg(arg);
    }
    let status = cmd
        .status()
        .await
        .map_err(|e| ArgentorError::Agent(format!("{program} failed to start: {e}")))?;

    if !status.success() {
        return Err(ArgentorError::Agent(format!(
            "{program} exited with {:?}",
            status.code()
        )));
    }
    Ok(())
}

/// Run a command and return its stdout as a String.
async fn run_cmd_output(program: &str, args: &[&str]) -> ArgentorResult<String> {
    let output = Command::new(program)
        .args(args)
        .output()
        .await
        .map_err(|e| ArgentorError::Agent(format!("{program} failed to start: {e}")))?;

    if !output.status.success() {
        return Err(ArgentorError::Agent(format!(
            "{program} exited with {:?}",
            output.status.code()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Parse `"x,y"` or `"x y"` formatted output into `(x, y)`.
fn parse_xy_output(s: &str) -> ArgentorResult<(i32, i32)> {
    let parts: Vec<&str> = if s.contains(',') {
        s.splitn(2, ',').collect()
    } else {
        s.split_whitespace().collect()
    };

    let parse = |v: &str| {
        v.trim()
            .parse::<i32>()
            .map_err(|e| ArgentorError::Agent(format!("parse coord '{v}': {e}")))
    };

    match parts.as_slice() {
        [xs, ys] => Ok((parse(xs)?, parse(ys)?)),
        _ => Err(ArgentorError::Agent(format!(
            "unexpected cursor output: '{s}'"
        ))),
    }
}

// ---------------------------------------------------------------------------
// Rate limiter
// ---------------------------------------------------------------------------

/// Token-bucket style rate limiter (max N actions per second).
struct RateLimiter {
    max_per_second: u32,
    last_reset: Instant,
    count_in_window: u32,
}

impl RateLimiter {
    fn new(max_per_second: u32) -> Self {
        Self {
            max_per_second,
            last_reset: Instant::now(),
            count_in_window: 0,
        }
    }

    /// Returns `true` if the action is allowed, `false` if rate-limited.
    fn allow(&mut self) -> bool {
        let now = Instant::now();
        if now.duration_since(self.last_reset) >= Duration::from_secs(1) {
            self.last_reset = now;
            self.count_in_window = 0;
        }
        if self.count_in_window < self.max_per_second {
            self.count_in_window += 1;
            true
        } else {
            false
        }
    }
}

// ---------------------------------------------------------------------------
// ComputerUseSkill
// ---------------------------------------------------------------------------

/// Skill that exposes computer-use actions to agents.
///
/// Registered as `computer_use`. Feature-gated behind the `computer-use` flag.
pub struct ComputerUseSkill {
    descriptor: SkillDescriptor,
    controller: Arc<dyn ScreenController>,
    config: ComputerUseConfig,
    rate_limiter: Arc<Mutex<RateLimiter>>,
}

impl ComputerUseSkill {
    /// Create a new skill using a custom [`ScreenController`] and config.
    pub fn new(controller: Arc<dyn ScreenController>, config: ComputerUseConfig) -> Self {
        let rl = Arc::new(Mutex::new(RateLimiter::new(config.max_actions_per_second)));
        Self {
            descriptor: SkillDescriptor {
                name: "computer_use".to_string(),
                description: "Control the computer: take screenshots, move/click the mouse, type text, press keys, and scroll. Mirrors the Claude computer_20241022 tool contract.".to_string(),
                parameters_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "action": {
                            "type": "string",
                            "enum": [
                                "screenshot",
                                "mouse_move",
                                "left_click",
                                "right_click",
                                "double_click",
                                "type",
                                "key",
                                "scroll_up",
                                "scroll_down",
                                "cursor_position"
                            ],
                            "description": "The computer action to perform"
                        },
                        "coordinate": {
                            "type": "array",
                            "items": {"type": "integer"},
                            "minItems": 2,
                            "maxItems": 2,
                            "description": "[x, y] screen coordinate for mouse/scroll actions"
                        },
                        "text": {
                            "type": "string",
                            "description": "Text to type (required for action=type)"
                        },
                        "key": {
                            "type": "string",
                            "description": "Key or key combination to press (required for action=key)"
                        }
                    },
                    "required": ["action"]
                }),
                required_capabilities: vec![Capability::ShellExec {
                    allowed_commands: vec![
                        "screencapture".to_string(),
                        "cliclick".to_string(),
                        "scrot".to_string(),
                        "gnome-screenshot".to_string(),
                        "xdotool".to_string(),
                    ],
                }],
                requires_approval: false,
            },
            controller,
            config,
            rate_limiter: rl,
        }
    }

    /// Build a [`ComputerUseSkill`] for the current platform with default config.
    #[cfg(target_os = "macos")]
    pub fn for_platform() -> Self {
        Self::new(Arc::new(MacOsController), ComputerUseConfig::default())
    }

    /// Build a [`ComputerUseSkill`] for the current platform with default config.
    #[cfg(target_os = "linux")]
    pub fn for_platform() -> Self {
        Self::new(Arc::new(LinuxController), ComputerUseConfig::default())
    }

    /// Check rate limit and region guard before executing any action.
    async fn check_guards(&self, coordinate: Option<(i32, i32)>) -> ArgentorResult<()> {
        // Rate limit
        {
            let mut rl = self.rate_limiter.lock().await;
            if !rl.allow() {
                return Err(ArgentorError::Security(format!(
                    "computer_use rate limit exceeded (max {} actions/sec)",
                    self.config.max_actions_per_second
                )));
            }
        }

        // Region guard
        if let Some((x, y)) = coordinate {
            if !self.config.allowed_regions.is_empty() {
                let allowed = self.config.allowed_regions.iter().any(|r| r.contains(x, y));
                if !allowed {
                    return Err(ArgentorError::Security(format!(
                        "coordinate ({x},{y}) is outside all allowed screen regions"
                    )));
                }
            }
        }

        Ok(())
    }
}

#[async_trait]
impl Skill for ComputerUseSkill {
    fn descriptor(&self) -> &SkillDescriptor {
        &self.descriptor
    }

    async fn execute(&self, call: ToolCall) -> ArgentorResult<ToolResult> {
        let action = call
            .arguments
            .get("action")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        if action.is_empty() {
            return Ok(ToolResult::error(
                &call.id,
                "Missing required field: action",
            ));
        }

        // Parse optional coordinate
        let coordinate: Option<(i32, i32)> = call
            .arguments
            .get("coordinate")
            .and_then(|v| v.as_array())
            .and_then(|arr| {
                if arr.len() == 2 {
                    let x = arr[0].as_i64()? as i32;
                    let y = arr[1].as_i64()? as i32;
                    Some((x, y))
                } else {
                    None
                }
            });

        self.check_guards(coordinate).await?;

        info!(action = %action, coordinate = ?coordinate, "computer_use action");

        match action.as_str() {
            "screenshot" => {
                debug!("Taking screenshot");
                let b64 = self.controller.screenshot().await?;
                Ok(ToolResult::success(
                    &call.id,
                    serde_json::json!({
                        "type": "base64",
                        "media_type": "image/png",
                        "data": b64,
                    })
                    .to_string(),
                ))
            }

            "mouse_move" => {
                let (x, y) = coordinate.ok_or_else(|| {
                    ArgentorError::Agent("mouse_move requires coordinate [x, y]".into())
                })?;
                self.controller.mouse_move(x, y).await?;
                Ok(ToolResult::success(&call.id, format!("moved to ({x},{y})")))
            }

            "left_click" => {
                let (x, y) = coordinate.ok_or_else(|| {
                    ArgentorError::Agent("left_click requires coordinate [x, y]".into())
                })?;
                self.controller.left_click(x, y).await?;
                Ok(ToolResult::success(&call.id, format!("clicked ({x},{y})")))
            }

            "right_click" => {
                let (x, y) = coordinate.ok_or_else(|| {
                    ArgentorError::Agent("right_click requires coordinate [x, y]".into())
                })?;
                self.controller.right_click(x, y).await?;
                Ok(ToolResult::success(
                    &call.id,
                    format!("right-clicked ({x},{y})"),
                ))
            }

            "double_click" => {
                let (x, y) = coordinate.ok_or_else(|| {
                    ArgentorError::Agent("double_click requires coordinate [x, y]".into())
                })?;
                self.controller.double_click(x, y).await?;
                Ok(ToolResult::success(
                    &call.id,
                    format!("double-clicked ({x},{y})"),
                ))
            }

            "type" => {
                let text = call
                    .arguments
                    .get("text")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                if text.is_empty() {
                    return Ok(ToolResult::error(
                        &call.id,
                        "type action requires non-empty 'text' field",
                    ));
                }
                self.controller.type_text(&text).await?;
                Ok(ToolResult::success(
                    &call.id,
                    format!("typed {} chars", text.len()),
                ))
            }

            "key" => {
                let key = call
                    .arguments
                    .get("key")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                if key.is_empty() {
                    return Ok(ToolResult::error(
                        &call.id,
                        "key action requires non-empty 'key' field",
                    ));
                }
                self.controller.key(&key).await?;
                Ok(ToolResult::success(&call.id, format!("pressed key: {key}")))
            }

            "scroll_up" => {
                let (x, y) = coordinate.unwrap_or((0, 0));
                self.controller.scroll_up(x, y).await?;
                Ok(ToolResult::success(
                    &call.id,
                    format!("scrolled up at ({x},{y})"),
                ))
            }

            "scroll_down" => {
                let (x, y) = coordinate.unwrap_or((0, 0));
                self.controller.scroll_down(x, y).await?;
                Ok(ToolResult::success(
                    &call.id,
                    format!("scrolled down at ({x},{y})"),
                ))
            }

            "cursor_position" => {
                let (x, y) = self.controller.cursor_position().await?;
                Ok(ToolResult::success(
                    &call.id,
                    serde_json::json!({"x": x, "y": y}).to_string(),
                ))
            }

            other => Ok(ToolResult::error(
                &call.id,
                format!("Unknown computer_use action: '{other}'"),
            )),
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use argentor_core::ToolCall;
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};

    // -----------------------------------------------------------------------
    // Mock controller
    // -----------------------------------------------------------------------

    struct MockController {
        screenshot_calls: AtomicUsize,
        mouse_move_calls: AtomicUsize,
        left_click_calls: AtomicUsize,
        right_click_calls: AtomicUsize,
        double_click_calls: AtomicUsize,
        type_calls: AtomicUsize,
        key_calls: AtomicUsize,
        scroll_up_calls: AtomicUsize,
        scroll_down_calls: AtomicUsize,
        cursor_calls: AtomicUsize,
        last_x: std::sync::atomic::AtomicI32,
        last_y: std::sync::atomic::AtomicI32,
    }

    impl MockController {
        fn new() -> Self {
            Self {
                screenshot_calls: AtomicUsize::new(0),
                mouse_move_calls: AtomicUsize::new(0),
                left_click_calls: AtomicUsize::new(0),
                right_click_calls: AtomicUsize::new(0),
                double_click_calls: AtomicUsize::new(0),
                type_calls: AtomicUsize::new(0),
                key_calls: AtomicUsize::new(0),
                scroll_up_calls: AtomicUsize::new(0),
                scroll_down_calls: AtomicUsize::new(0),
                cursor_calls: AtomicUsize::new(0),
                last_x: std::sync::atomic::AtomicI32::new(0),
                last_y: std::sync::atomic::AtomicI32::new(0),
            }
        }
    }

    #[async_trait]
    impl ScreenController for MockController {
        async fn screenshot(&self) -> ArgentorResult<String> {
            self.screenshot_calls.fetch_add(1, Ordering::SeqCst);
            // Tiny 1×1 PNG (base64)
            Ok("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYGBgAAAABQABDQottAAAAABJRU5ErkJggg==".into())
        }
        async fn mouse_move(&self, x: i32, y: i32) -> ArgentorResult<()> {
            self.mouse_move_calls.fetch_add(1, Ordering::SeqCst);
            self.last_x.store(x, Ordering::SeqCst);
            self.last_y.store(y, Ordering::SeqCst);
            Ok(())
        }
        async fn left_click(&self, x: i32, y: i32) -> ArgentorResult<()> {
            self.left_click_calls.fetch_add(1, Ordering::SeqCst);
            self.last_x.store(x, Ordering::SeqCst);
            self.last_y.store(y, Ordering::SeqCst);
            Ok(())
        }
        async fn right_click(&self, x: i32, y: i32) -> ArgentorResult<()> {
            self.right_click_calls.fetch_add(1, Ordering::SeqCst);
            self.last_x.store(x, Ordering::SeqCst);
            self.last_y.store(y, Ordering::SeqCst);
            Ok(())
        }
        async fn double_click(&self, x: i32, y: i32) -> ArgentorResult<()> {
            self.double_click_calls.fetch_add(1, Ordering::SeqCst);
            self.last_x.store(x, Ordering::SeqCst);
            self.last_y.store(y, Ordering::SeqCst);
            Ok(())
        }
        async fn type_text(&self, _text: &str) -> ArgentorResult<()> {
            self.type_calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        async fn key(&self, _key: &str) -> ArgentorResult<()> {
            self.key_calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        async fn scroll_up(&self, x: i32, y: i32) -> ArgentorResult<()> {
            self.scroll_up_calls.fetch_add(1, Ordering::SeqCst);
            self.last_x.store(x, Ordering::SeqCst);
            self.last_y.store(y, Ordering::SeqCst);
            Ok(())
        }
        async fn scroll_down(&self, x: i32, y: i32) -> ArgentorResult<()> {
            self.scroll_down_calls.fetch_add(1, Ordering::SeqCst);
            self.last_x.store(x, Ordering::SeqCst);
            self.last_y.store(y, Ordering::SeqCst);
            Ok(())
        }
        async fn cursor_position(&self) -> ArgentorResult<(i32, i32)> {
            self.cursor_calls.fetch_add(1, Ordering::SeqCst);
            Ok((42, 99))
        }
    }

    fn make_skill(config: ComputerUseConfig) -> (ComputerUseSkill, Arc<MockController>) {
        let mc = Arc::new(MockController::new());
        let skill = ComputerUseSkill::new(mc.clone() as Arc<dyn ScreenController>, config);
        (skill, mc)
    }

    fn call(action: &str, extra: serde_json::Value) -> ToolCall {
        let mut args = json!({"action": action});
        if let (Some(obj), Some(ext)) = (args.as_object_mut(), extra.as_object()) {
            for (k, v) in ext {
                obj.insert(k.clone(), v.clone());
            }
        }
        ToolCall {
            id: "test-call".into(),
            name: "computer_use".into(),
            arguments: args,
        }
    }

    // -----------------------------------------------------------------------
    // Tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_screenshot() {
        let (skill, mc) = make_skill(ComputerUseConfig::default());
        let result = skill.execute(call("screenshot", json!({}))).await.unwrap();
        assert!(result.content.contains("base64"));
        assert_eq!(mc.screenshot_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_mouse_move() {
        let (skill, mc) = make_skill(ComputerUseConfig::default());
        let result = skill
            .execute(call("mouse_move", json!({"coordinate": [100, 200]})))
            .await
            .unwrap();
        assert!(result.content.contains("100"));
        assert_eq!(mc.mouse_move_calls.load(Ordering::SeqCst), 1);
        assert_eq!(mc.last_x.load(Ordering::SeqCst), 100);
        assert_eq!(mc.last_y.load(Ordering::SeqCst), 200);
    }

    #[tokio::test]
    async fn test_left_click() {
        let (skill, mc) = make_skill(ComputerUseConfig::default());
        skill
            .execute(call("left_click", json!({"coordinate": [10, 20]})))
            .await
            .unwrap();
        assert_eq!(mc.left_click_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_right_click() {
        let (skill, mc) = make_skill(ComputerUseConfig::default());
        skill
            .execute(call("right_click", json!({"coordinate": [10, 20]})))
            .await
            .unwrap();
        assert_eq!(mc.right_click_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_double_click() {
        let (skill, mc) = make_skill(ComputerUseConfig::default());
        skill
            .execute(call("double_click", json!({"coordinate": [10, 20]})))
            .await
            .unwrap();
        assert_eq!(mc.double_click_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_type_text() {
        let (skill, mc) = make_skill(ComputerUseConfig::default());
        let result = skill
            .execute(call("type", json!({"text": "hello world"})))
            .await
            .unwrap();
        assert!(result.content.contains("11"));
        assert_eq!(mc.type_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_type_empty_error() {
        let (skill, _) = make_skill(ComputerUseConfig::default());
        let result = skill
            .execute(call("type", json!({"text": ""})))
            .await
            .unwrap();
        assert!(result.is_error);
    }

    #[tokio::test]
    async fn test_key() {
        let (skill, mc) = make_skill(ComputerUseConfig::default());
        skill
            .execute(call("key", json!({"key": "Return"})))
            .await
            .unwrap();
        assert_eq!(mc.key_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_scroll_up() {
        let (skill, mc) = make_skill(ComputerUseConfig::default());
        skill
            .execute(call("scroll_up", json!({"coordinate": [50, 50]})))
            .await
            .unwrap();
        assert_eq!(mc.scroll_up_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_scroll_down() {
        let (skill, mc) = make_skill(ComputerUseConfig::default());
        skill
            .execute(call("scroll_down", json!({"coordinate": [50, 50]})))
            .await
            .unwrap();
        assert_eq!(mc.scroll_down_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_cursor_position() {
        let (skill, mc) = make_skill(ComputerUseConfig::default());
        let result = skill
            .execute(call("cursor_position", json!({})))
            .await
            .unwrap();
        assert!(result.content.contains("42"));
        assert_eq!(mc.cursor_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_unknown_action() {
        let (skill, _) = make_skill(ComputerUseConfig::default());
        let result = skill.execute(call("explode", json!({}))).await.unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("Unknown"));
    }

    #[tokio::test]
    async fn test_missing_action() {
        let (skill, _) = make_skill(ComputerUseConfig::default());
        let tc = ToolCall {
            id: "x".into(),
            name: "computer_use".into(),
            arguments: json!({}),
        };
        let result = skill.execute(tc).await.unwrap();
        assert!(result.is_error);
    }

    #[tokio::test]
    async fn test_region_guard_blocks_out_of_bounds() {
        let config = ComputerUseConfig {
            max_actions_per_second: 100,
            allowed_regions: vec![ScreenRegion {
                x: 0,
                y: 0,
                width: 100,
                height: 100,
            }],
        };
        let (skill, mc) = make_skill(config);
        // Coordinate outside region
        let err = skill
            .execute(call("left_click", json!({"coordinate": [500, 500]})))
            .await;
        assert!(err.is_err());
        assert_eq!(mc.left_click_calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn test_region_guard_allows_inside() {
        let config = ComputerUseConfig {
            max_actions_per_second: 100,
            allowed_regions: vec![ScreenRegion {
                x: 0,
                y: 0,
                width: 1920,
                height: 1080,
            }],
        };
        let (skill, mc) = make_skill(config);
        skill
            .execute(call("left_click", json!({"coordinate": [100, 100]})))
            .await
            .unwrap();
        assert_eq!(mc.left_click_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_rate_limit() {
        // Allow only 2 actions/sec
        let config = ComputerUseConfig {
            max_actions_per_second: 2,
            allowed_regions: vec![],
        };
        let (skill, mc) = make_skill(config);

        // First two should succeed
        skill.execute(call("screenshot", json!({}))).await.unwrap();
        skill.execute(call("screenshot", json!({}))).await.unwrap();

        // Third should be rate-limited
        let err = skill.execute(call("screenshot", json!({}))).await;
        assert!(err.is_err());
        let msg = err.unwrap_err().to_string();
        assert!(msg.contains("rate limit") || msg.contains("exceeded"));

        // Only 2 actual calls reached the controller
        assert_eq!(mc.screenshot_calls.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn test_parse_xy_comma() {
        assert_eq!(parse_xy_output("123,456").unwrap(), (123, 456));
    }

    #[test]
    fn test_parse_xy_space() {
        assert_eq!(parse_xy_output("10 20").unwrap(), (10, 20));
    }

    #[test]
    fn test_screen_region_contains() {
        let r = ScreenRegion {
            x: 10,
            y: 10,
            width: 100,
            height: 100,
        };
        assert!(r.contains(10, 10));
        assert!(r.contains(50, 50));
        assert!(r.contains(109, 109));
        assert!(!r.contains(110, 110));
        assert!(!r.contains(9, 10));
    }
}
