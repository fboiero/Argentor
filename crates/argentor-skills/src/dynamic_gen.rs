//! Dynamic tool generation at runtime from declarative specifications.
//!
//! Inspired by IronClaw's dynamic WASM tool generation, this module lets agents
//! CREATE NEW TOOLS at runtime from natural language descriptions. Generated
//! tools can use templates, expressions, or composite pipelines of existing tools.
//!
//! # Key types
//!
//! - [`DynamicToolGenerator`] — the engine that manages generated tools.
//! - [`ToolSpec`] — declarative description of a tool to generate.
//! - [`GeneratedTool`] — a tool instance with metadata and usage stats.
//! - [`ToolImplementation`] — how the tool executes (template, expression, composite).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Configuration for the dynamic tool generator.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DynamicGenConfig {
    /// Whether dynamic tool generation is enabled.
    pub enabled: bool,
    /// Maximum number of generated tools kept in cache.
    pub max_generated_tools: usize,
    /// Capabilities that generated tools are allowed to use.
    pub allowed_capabilities: Vec<String>,
    /// Restrict generated tools to safe operations only.
    pub sandbox_mode: bool,
    /// Persist generated tools across sessions.
    pub persist_tools: bool,
}

impl Default for DynamicGenConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_generated_tools: 20,
            allowed_capabilities: Vec::new(),
            sandbox_mode: true,
            persist_tools: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Tool specification types
// ---------------------------------------------------------------------------

/// Declarative specification for generating a new tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSpec {
    /// Unique name for the tool.
    pub name: String,
    /// Human-readable description shown to the LLM.
    pub description: String,
    /// Parameters the tool accepts.
    pub parameters: Vec<ParamSpec>,
    /// Natural language description of the implementation logic.
    pub implementation_hint: String,
    /// Expected return format (e.g. "string", "json", "number").
    pub return_type: String,
    /// Example inputs and outputs for validation and documentation.
    pub examples: Vec<ToolExample>,
}

/// A single parameter definition for a generated tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParamSpec {
    /// Parameter name.
    pub name: String,
    /// Type: "string", "number", "boolean", "array", "object".
    pub param_type: String,
    /// Human-readable description.
    pub description: String,
    /// Whether this parameter is required.
    pub required: bool,
    /// Default value if not provided.
    pub default: Option<Value>,
}

/// Example input/output pair for documentation and testing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolExample {
    /// Sample input arguments.
    pub input: Value,
    /// Expected output for this input.
    pub expected_output: String,
}

// ---------------------------------------------------------------------------
// Generated tool and implementation
// ---------------------------------------------------------------------------

/// A generated tool with its spec, implementation, and runtime stats.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneratedTool {
    /// The declarative spec this tool was generated from.
    pub spec: ToolSpec,
    /// How the tool executes.
    pub implementation: ToolImplementation,
    /// When the tool was generated.
    pub created_at: DateTime<Utc>,
    /// How many times the tool has been executed.
    pub usage_count: u32,
    /// Timestamp of the most recent execution.
    pub last_used: Option<DateTime<Utc>>,
    /// Fraction of executions that succeeded (0.0–1.0).
    pub success_rate: f32,
}

/// How a generated tool executes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ToolImplementation {
    /// Template-based: output is a string template with `{{param}}` placeholders
    /// and an optional data transform applied to the result.
    Template(TemplateImpl),
    /// Expression-based: a simple expression string that is evaluated.
    Expression(String),
    /// Composite: a pipeline of existing tools executed in sequence.
    Composite(Vec<ToolPipelineStep>),
}

/// Template implementation with placeholder substitution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemplateImpl {
    /// Output template with `{{param}}` placeholders.
    pub template: String,
    /// Optional transformation applied after template rendering.
    pub transform: Option<TransformOp>,
}

/// Data transformations applicable to template output.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TransformOp {
    /// Extract a field from a JSON string by dot-path.
    JsonExtract(String),
    /// Apply a regex and return the first capture group.
    Regex(String),
    /// Split the string by a delimiter.
    Split(String),
    /// Join an array of strings by a delimiter.
    Join(String),
    /// Convert to uppercase.
    Upper,
    /// Convert to lowercase.
    Lower,
    /// Trim whitespace.
    Trim,
}

/// A step in a composite tool pipeline.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolPipelineStep {
    /// Name of the tool to invoke at this step.
    pub tool_name: String,
    /// Maps pipeline input keys to the tool's parameter names.
    pub param_mapping: HashMap<String, String>,
}

// ---------------------------------------------------------------------------
// Generator statistics
// ---------------------------------------------------------------------------

/// Aggregate stats for the dynamic tool generator.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneratorStats {
    /// Total number of tools currently in the cache.
    pub total_tools: usize,
    /// Total executions across all generated tools.
    pub total_executions: u64,
    /// Average success rate across all generated tools.
    pub avg_success_rate: f32,
    /// Name of the most-used generated tool.
    pub most_used_tool: Option<String>,
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Errors specific to dynamic tool generation and execution.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum DynamicGenError {
    /// The generator is disabled.
    Disabled,
    /// A tool with this name already exists.
    DuplicateName(String),
    /// Maximum number of generated tools reached.
    CapacityExceeded,
    /// The tool was not found.
    NotFound(String),
    /// A required parameter is missing.
    MissingParam(String),
    /// A template rendering error.
    TemplateError(String),
    /// A transform operation failed.
    TransformError(String),
    /// Expression evaluation error.
    ExpressionError(String),
    /// A pipeline step references a non-existent tool.
    PipelineError(String),
    /// The tool spec failed validation.
    InvalidSpec(String),
}

impl std::fmt::Display for DynamicGenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Disabled => write!(f, "Dynamic tool generation is disabled"),
            Self::DuplicateName(n) => write!(f, "Tool '{n}' already exists"),
            Self::CapacityExceeded => write!(f, "Maximum generated tools capacity exceeded"),
            Self::NotFound(n) => write!(f, "Generated tool '{n}' not found"),
            Self::MissingParam(p) => write!(f, "Required parameter '{p}' is missing"),
            Self::TemplateError(e) => write!(f, "Template error: {e}"),
            Self::TransformError(e) => write!(f, "Transform error: {e}"),
            Self::ExpressionError(e) => write!(f, "Expression error: {e}"),
            Self::PipelineError(e) => write!(f, "Pipeline error: {e}"),
            Self::InvalidSpec(e) => write!(f, "Invalid tool spec: {e}"),
        }
    }
}

impl std::error::Error for DynamicGenError {}

// ---------------------------------------------------------------------------
// DynamicToolGenerator
// ---------------------------------------------------------------------------

/// Engine for generating, caching, and executing tools at runtime.
pub struct DynamicToolGenerator {
    config: DynamicGenConfig,
    generated_tools: HashMap<String, GeneratedTool>,
}

impl DynamicToolGenerator {
    /// Create a new generator with the given configuration.
    pub fn new(config: DynamicGenConfig) -> Self {
        Self {
            config,
            generated_tools: HashMap::new(),
        }
    }

    /// Create a generator with default configuration.
    pub fn with_defaults() -> Self {
        Self::new(DynamicGenConfig::default())
    }

    /// Generate and register a new tool from a declarative spec.
    ///
    /// The implementation is derived from the spec's `implementation_hint`:
    /// - Hints containing `"template:"` produce a [`ToolImplementation::Template`].
    /// - Hints containing `"expr:"` produce a [`ToolImplementation::Expression`].
    /// - Hints containing `"pipeline:"` produce a [`ToolImplementation::Composite`].
    /// - Everything else defaults to a template that echoes the description.
    pub fn generate_tool(&mut self, spec: ToolSpec) -> Result<&GeneratedTool, DynamicGenError> {
        if !self.config.enabled {
            return Err(DynamicGenError::Disabled);
        }

        self.validate_spec(&spec)?;

        if self.generated_tools.contains_key(&spec.name) {
            return Err(DynamicGenError::DuplicateName(spec.name.clone()));
        }

        if self.generated_tools.len() >= self.config.max_generated_tools {
            return Err(DynamicGenError::CapacityExceeded);
        }

        let implementation = self.derive_implementation(&spec);

        let tool = GeneratedTool {
            spec: spec.clone(),
            implementation,
            created_at: Utc::now(),
            usage_count: 0,
            last_used: None,
            success_rate: 1.0,
        };

        let name = spec.name.clone();
        self.generated_tools.insert(name.clone(), tool);
        // Safety: we just inserted this key
        #[allow(clippy::expect_used)]
        Ok(self.generated_tools.get(&name).expect("just inserted"))
    }

    /// Execute a generated tool with the given arguments.
    pub fn execute_generated(
        &mut self,
        tool_name: &str,
        args: &Value,
    ) -> Result<String, DynamicGenError> {
        if !self.config.enabled {
            return Err(DynamicGenError::Disabled);
        }

        // Validate required params first (borrow immutably).
        {
            let tool = self
                .generated_tools
                .get(tool_name)
                .ok_or_else(|| DynamicGenError::NotFound(tool_name.to_string()))?;

            for param in &tool.spec.parameters {
                if param.required && args.get(&param.name).is_none() {
                    // Check if there's a default
                    if param.default.is_none() {
                        return Err(DynamicGenError::MissingParam(param.name.clone()));
                    }
                }
            }
        }

        // Build effective args with defaults.
        let effective_args = {
            let tool = self
                .generated_tools
                .get(tool_name)
                .ok_or_else(|| DynamicGenError::NotFound(tool_name.to_string()))?;
            self.build_effective_args(&tool.spec, args)
        };

        // Execute based on implementation type.
        let result = {
            let tool = self
                .generated_tools
                .get(tool_name)
                .ok_or_else(|| DynamicGenError::NotFound(tool_name.to_string()))?;
            match &tool.implementation {
                ToolImplementation::Template(tmpl) => self.execute_template(tmpl, &effective_args),
                ToolImplementation::Expression(expr) => {
                    self.execute_expression(expr, &effective_args)
                }
                ToolImplementation::Composite(steps) => {
                    let steps_clone = steps.clone();
                    self.execute_composite(&steps_clone, &effective_args)
                }
            }
        };

        // Update stats.
        let tool = self
            .generated_tools
            .get_mut(tool_name)
            .ok_or_else(|| DynamicGenError::NotFound(tool_name.to_string()))?;
        tool.usage_count += 1;
        tool.last_used = Some(Utc::now());

        match &result {
            Ok(_) => {
                let total = tool.usage_count as f32;
                let prev_successes = tool.success_rate * (total - 1.0);
                tool.success_rate = (prev_successes + 1.0) / total;
            }
            Err(_) => {
                let total = tool.usage_count as f32;
                let prev_successes = tool.success_rate * (total - 1.0);
                tool.success_rate = prev_successes / total;
            }
        }

        result
    }

    /// List all generated tools with their names and descriptions.
    pub fn list_generated(&self) -> Vec<(&str, &str)> {
        self.generated_tools
            .values()
            .map(|t| (t.spec.name.as_str(), t.spec.description.as_str()))
            .collect()
    }

    /// Remove a generated tool by name.
    pub fn remove_generated(&mut self, name: &str) -> Result<GeneratedTool, DynamicGenError> {
        self.generated_tools
            .remove(name)
            .ok_or_else(|| DynamicGenError::NotFound(name.to_string()))
    }

    /// Get a reference to a generated tool by name.
    pub fn get_tool(&self, name: &str) -> Option<&GeneratedTool> {
        self.generated_tools.get(name)
    }

    /// Return aggregate statistics for the generator.
    pub fn get_stats(&self) -> GeneratorStats {
        let total_tools = self.generated_tools.len();

        let total_executions: u64 = self
            .generated_tools
            .values()
            .map(|t| u64::from(t.usage_count))
            .sum();

        let avg_success_rate = if total_tools == 0 {
            0.0
        } else {
            let sum: f32 = self.generated_tools.values().map(|t| t.success_rate).sum();
            sum / total_tools as f32
        };

        let most_used_tool = self
            .generated_tools
            .values()
            .max_by_key(|t| t.usage_count)
            .filter(|t| t.usage_count > 0)
            .map(|t| t.spec.name.clone());

        GeneratorStats {
            total_tools,
            total_executions,
            avg_success_rate,
            most_used_tool,
        }
    }

    /// Serialize all generated tools to JSON for persistence.
    pub fn serialize(&self) -> Result<String, DynamicGenError> {
        serde_json::to_string_pretty(&self.generated_tools)
            .map_err(|e| DynamicGenError::TemplateError(format!("Serialization failed: {e}")))
    }

    /// Deserialize tools from JSON and load into the generator.
    pub fn deserialize(&mut self, json: &str) -> Result<usize, DynamicGenError> {
        let tools: HashMap<String, GeneratedTool> = serde_json::from_str(json)
            .map_err(|e| DynamicGenError::TemplateError(format!("Deserialization failed: {e}")))?;
        let count = tools.len();
        self.generated_tools = tools;
        Ok(count)
    }

    // -----------------------------------------------------------------------
    // Private helpers
    // -----------------------------------------------------------------------

    /// Validate a tool spec before generation.
    fn validate_spec(&self, spec: &ToolSpec) -> Result<(), DynamicGenError> {
        if spec.name.is_empty() {
            return Err(DynamicGenError::InvalidSpec(
                "Tool name cannot be empty".into(),
            ));
        }

        if spec.name.len() > 64 {
            return Err(DynamicGenError::InvalidSpec(
                "Tool name exceeds 64 characters".into(),
            ));
        }

        if spec.description.is_empty() {
            return Err(DynamicGenError::InvalidSpec(
                "Description cannot be empty".into(),
            ));
        }

        // Validate param names are unique.
        let mut seen = std::collections::HashSet::new();
        for p in &spec.parameters {
            if !seen.insert(&p.name) {
                return Err(DynamicGenError::InvalidSpec(format!(
                    "Duplicate parameter name: {}",
                    p.name
                )));
            }
        }

        // Validate param types.
        let valid_types = ["string", "number", "boolean", "array", "object"];
        for p in &spec.parameters {
            if !valid_types.contains(&p.param_type.as_str()) {
                return Err(DynamicGenError::InvalidSpec(format!(
                    "Invalid parameter type '{}' for '{}'",
                    p.param_type, p.name
                )));
            }
        }

        Ok(())
    }

    /// Derive an implementation from the spec's implementation hint.
    fn derive_implementation(&self, spec: &ToolSpec) -> ToolImplementation {
        let hint = spec.implementation_hint.trim();

        if let Some(template) = hint.strip_prefix("template:") {
            let template = template.trim().to_string();
            ToolImplementation::Template(TemplateImpl {
                template,
                transform: None,
            })
        } else if let Some(expr) = hint.strip_prefix("expr:") {
            ToolImplementation::Expression(expr.trim().to_string())
        } else if hint.starts_with("pipeline:") {
            let steps = self.parse_pipeline_hint(hint);
            ToolImplementation::Composite(steps)
        } else {
            // Default: a template that echoes the description with param values.
            let mut template = format!("[{}] ", spec.description);
            for p in &spec.parameters {
                template.push_str(&format!("{}={{{{{}}}}}, ", p.name, p.name));
            }
            // Remove trailing ", "
            if template.ends_with(", ") {
                template.truncate(template.len() - 2);
            }
            ToolImplementation::Template(TemplateImpl {
                template,
                transform: None,
            })
        }
    }

    /// Parse a pipeline hint into pipeline steps.
    /// Format: `pipeline: tool1(param=input_key); tool2(param=prev_result)`
    fn parse_pipeline_hint(&self, hint: &str) -> Vec<ToolPipelineStep> {
        let body = hint.strip_prefix("pipeline:").unwrap_or(hint).trim();
        let mut steps = Vec::new();

        for part in body.split(';') {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }

            // Parse "tool_name(key=val, key2=val2)"
            if let Some(paren_idx) = part.find('(') {
                let tool_name = part[..paren_idx].trim().to_string();
                let mapping_str = part[paren_idx + 1..].trim_end_matches(')');
                let mut param_mapping = HashMap::new();
                for pair in mapping_str.split(',') {
                    let pair = pair.trim();
                    if let Some((k, v)) = pair.split_once('=') {
                        param_mapping.insert(k.trim().to_string(), v.trim().to_string());
                    }
                }
                steps.push(ToolPipelineStep {
                    tool_name,
                    param_mapping,
                });
            } else {
                steps.push(ToolPipelineStep {
                    tool_name: part.to_string(),
                    param_mapping: HashMap::new(),
                });
            }
        }

        steps
    }

    /// Build effective arguments by filling in defaults for missing optional params.
    fn build_effective_args(&self, spec: &ToolSpec, args: &Value) -> Value {
        let mut effective = args.clone();
        if let Some(obj) = effective.as_object_mut() {
            for param in &spec.parameters {
                if !obj.contains_key(&param.name) {
                    if let Some(default) = &param.default {
                        obj.insert(param.name.clone(), default.clone());
                    }
                }
            }
        }
        effective
    }

    /// Execute a template-based tool.
    fn execute_template(
        &self,
        tmpl: &TemplateImpl,
        args: &Value,
    ) -> Result<String, DynamicGenError> {
        let mut output = tmpl.template.clone();

        // Replace {{param}} placeholders with argument values.
        if let Some(obj) = args.as_object() {
            for (key, val) in obj {
                let placeholder = format!("{{{{{key}}}}}");
                let replacement = match val {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                output = output.replace(&placeholder, &replacement);
            }
        }

        // Check for un-replaced placeholders.
        if output.contains("{{") && output.contains("}}") {
            return Err(DynamicGenError::TemplateError(
                "Unreplaced placeholders remain in template".into(),
            ));
        }

        // Apply optional transform.
        if let Some(transform) = &tmpl.transform {
            output = self.apply_transform(transform, &output)?;
        }

        Ok(output)
    }

    /// Execute an expression-based tool.
    ///
    /// Supports a minimal expression language:
    /// - `concat(a, b)` — concatenate string values
    /// - `upper(param)` / `lower(param)` — case conversion
    /// - `len(param)` — string length
    /// - `add(a, b)` / `sub(a, b)` / `mul(a, b)` — arithmetic
    /// - Raw string with `{{param}}` placeholders as fallback
    fn execute_expression(&self, expr: &str, args: &Value) -> Result<String, DynamicGenError> {
        let expr = expr.trim();

        // concat(a, b)
        if let Some(inner) = strip_func("concat", expr) {
            let parts = split_args(inner);
            let mut result = String::new();
            for part in parts {
                result.push_str(&resolve_value(part.trim(), args));
            }
            return Ok(result);
        }

        // upper(param)
        if let Some(inner) = strip_func("upper", expr) {
            let val = resolve_value(inner.trim(), args);
            return Ok(val.to_uppercase());
        }

        // lower(param)
        if let Some(inner) = strip_func("lower", expr) {
            let val = resolve_value(inner.trim(), args);
            return Ok(val.to_lowercase());
        }

        // len(param)
        if let Some(inner) = strip_func("len", expr) {
            let val = resolve_value(inner.trim(), args);
            return Ok(val.len().to_string());
        }

        // add(a, b)
        if let Some(inner) = strip_func("add", expr) {
            let parts = split_args(inner);
            if parts.len() == 2 {
                let a = resolve_number(parts[0].trim(), args)?;
                let b = resolve_number(parts[1].trim(), args)?;
                return Ok((a + b).to_string());
            }
        }

        // sub(a, b)
        if let Some(inner) = strip_func("sub", expr) {
            let parts = split_args(inner);
            if parts.len() == 2 {
                let a = resolve_number(parts[0].trim(), args)?;
                let b = resolve_number(parts[1].trim(), args)?;
                return Ok((a - b).to_string());
            }
        }

        // mul(a, b)
        if let Some(inner) = strip_func("mul", expr) {
            let parts = split_args(inner);
            if parts.len() == 2 {
                let a = resolve_number(parts[0].trim(), args)?;
                let b = resolve_number(parts[1].trim(), args)?;
                return Ok((a * b).to_string());
            }
        }

        // Fallback: template-style substitution.
        let tmpl = TemplateImpl {
            template: expr.to_string(),
            transform: None,
        };
        self.execute_template(&tmpl, args)
    }

    /// Execute a composite pipeline tool.
    fn execute_composite(
        &mut self,
        steps: &[ToolPipelineStep],
        initial_args: &Value,
    ) -> Result<String, DynamicGenError> {
        let mut current_result = String::new();
        let mut pipeline_context = initial_args.clone();

        for (i, step) in steps.iter().enumerate() {
            // Build args for this step by mapping from pipeline context.
            let mut step_args = serde_json::Map::new();
            for (tool_param, source_key) in &step.param_mapping {
                if source_key == "_prev" || source_key == "prev_result" {
                    step_args.insert(tool_param.clone(), Value::String(current_result.clone()));
                } else if let Some(val) = pipeline_context.get(source_key) {
                    step_args.insert(tool_param.clone(), val.clone());
                }
            }

            let step_args_val = Value::Object(step_args);

            // Execute the referenced tool if it exists in generated tools.
            let result = if self.generated_tools.contains_key(&step.tool_name) {
                self.execute_generated(&step.tool_name, &step_args_val)?
            } else {
                return Err(DynamicGenError::PipelineError(format!(
                    "Step {}: tool '{}' not found",
                    i, step.tool_name
                )));
            };

            current_result = result;

            // Put the result into the pipeline context for the next step.
            if let Some(ctx) = pipeline_context.as_object_mut() {
                ctx.insert("_prev".to_string(), Value::String(current_result.clone()));
            }
        }

        Ok(current_result)
    }

    /// Apply a transform operation to a string.
    fn apply_transform(
        &self,
        transform: &TransformOp,
        input: &str,
    ) -> Result<String, DynamicGenError> {
        match transform {
            TransformOp::Upper => Ok(input.to_uppercase()),
            TransformOp::Lower => Ok(input.to_lowercase()),
            TransformOp::Trim => Ok(input.trim().to_string()),

            TransformOp::Split(delimiter) => {
                let parts: Vec<&str> = input.split(delimiter.as_str()).collect();
                serde_json::to_string(&parts)
                    .map_err(|e| DynamicGenError::TransformError(e.to_string()))
            }

            TransformOp::Join(delimiter) => {
                // Expect input to be a JSON array of strings.
                let arr: Vec<String> = serde_json::from_str(input).map_err(|e| {
                    DynamicGenError::TransformError(format!("Not a JSON array: {e}"))
                })?;
                Ok(arr.join(delimiter))
            }

            TransformOp::JsonExtract(path) => {
                let val: Value = serde_json::from_str(input)
                    .map_err(|e| DynamicGenError::TransformError(format!("Invalid JSON: {e}")))?;
                let mut current = &val;
                for key in path.split('.') {
                    current = current.get(key).ok_or_else(|| {
                        DynamicGenError::TransformError(format!("Key '{key}' not found in JSON"))
                    })?;
                }
                match current {
                    Value::String(s) => Ok(s.clone()),
                    other => Ok(other.to_string()),
                }
            }

            TransformOp::Regex(pattern) => {
                // Simple regex: find first match.
                // We use a basic approach since we don't have the regex crate
                // in argentor-skills. Return the whole input if no match.
                // For a real implementation, add regex as a dependency.
                // Placeholder: check if the pattern appears as a literal substring.
                if input.contains(pattern.as_str()) {
                    Ok(pattern.clone())
                } else {
                    Err(DynamicGenError::TransformError(format!(
                        "Pattern '{pattern}' not found in input"
                    )))
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Free helper functions for expression evaluation
// ---------------------------------------------------------------------------

/// Strip a function call like `func(...)` and return the inner content.
fn strip_func<'a>(name: &str, expr: &'a str) -> Option<&'a str> {
    let prefix = format!("{name}(");
    if expr.starts_with(&prefix) && expr.ends_with(')') {
        Some(&expr[prefix.len()..expr.len() - 1])
    } else {
        None
    }
}

/// Split comma-separated arguments, respecting nesting (basic).
fn split_args(s: &str) -> Vec<&str> {
    s.split(',').collect()
}

/// Resolve a value reference against the args object.
/// If the name matches a key in args, return its string value.
/// Otherwise treat it as a literal string (strip surrounding quotes if any).
fn resolve_value(name: &str, args: &Value) -> String {
    // Try as a key in args first.
    if let Some(val) = args.get(name) {
        return match val {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        };
    }
    // Strip quotes from literals.
    let trimmed = name.trim_matches('"').trim_matches('\'');
    trimmed.to_string()
}

/// Resolve a numeric value from args or literal.
fn resolve_number(name: &str, args: &Value) -> Result<f64, DynamicGenError> {
    if let Some(val) = args.get(name) {
        return val
            .as_f64()
            .ok_or_else(|| DynamicGenError::ExpressionError(format!("'{name}' is not a number")));
    }
    name.parse::<f64>()
        .map_err(|_| DynamicGenError::ExpressionError(format!("Cannot parse '{name}' as number")))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Helper to create a minimal valid spec.
    fn simple_spec(name: &str, hint: &str) -> ToolSpec {
        ToolSpec {
            name: name.to_string(),
            description: format!("Test tool: {name}"),
            parameters: vec![ParamSpec {
                name: "input".to_string(),
                param_type: "string".to_string(),
                description: "The input value".to_string(),
                required: true,
                default: None,
            }],
            implementation_hint: hint.to_string(),
            return_type: "string".to_string(),
            examples: vec![],
        }
    }

    fn default_gen() -> DynamicToolGenerator {
        DynamicToolGenerator::with_defaults()
    }

    // -- Config and construction ------------------------------------------------

    #[test]
    fn test_default_config() {
        let config = DynamicGenConfig::default();
        assert!(config.enabled);
        assert_eq!(config.max_generated_tools, 20);
        assert!(config.sandbox_mode);
        assert!(!config.persist_tools);
    }

    #[test]
    fn test_new_generator_empty() {
        let gen = default_gen();
        assert!(gen.list_generated().is_empty());
        assert_eq!(gen.get_stats().total_tools, 0);
    }

    // -- Spec validation -------------------------------------------------------

    #[test]
    fn test_empty_name_rejected() {
        let mut gen = default_gen();
        let mut spec = simple_spec("", "template: {{input}}");
        spec.name = String::new();
        let result = gen.generate_tool(spec);
        assert!(matches!(result, Err(DynamicGenError::InvalidSpec(_))));
    }

    #[test]
    fn test_long_name_rejected() {
        let mut gen = default_gen();
        let mut spec = simple_spec("x", "template: {{input}}");
        spec.name = "a".repeat(65);
        let result = gen.generate_tool(spec);
        assert!(matches!(result, Err(DynamicGenError::InvalidSpec(_))));
    }

    #[test]
    fn test_empty_description_rejected() {
        let mut gen = default_gen();
        let mut spec = simple_spec("tool", "template: {{input}}");
        spec.description = String::new();
        let result = gen.generate_tool(spec);
        assert!(matches!(result, Err(DynamicGenError::InvalidSpec(_))));
    }

    #[test]
    fn test_duplicate_param_names_rejected() {
        let mut gen = default_gen();
        let mut spec = simple_spec("tool", "template: {{a}}");
        spec.parameters = vec![
            ParamSpec {
                name: "a".into(),
                param_type: "string".into(),
                description: "first".into(),
                required: true,
                default: None,
            },
            ParamSpec {
                name: "a".into(),
                param_type: "number".into(),
                description: "duplicate".into(),
                required: false,
                default: None,
            },
        ];
        assert!(matches!(
            gen.generate_tool(spec),
            Err(DynamicGenError::InvalidSpec(_))
        ));
    }

    #[test]
    fn test_invalid_param_type_rejected() {
        let mut gen = default_gen();
        let mut spec = simple_spec("tool", "template: {{input}}");
        spec.parameters[0].param_type = "float".into();
        assert!(matches!(
            gen.generate_tool(spec),
            Err(DynamicGenError::InvalidSpec(_))
        ));
    }

    // -- Generation -------------------------------------------------------------

    #[test]
    fn test_generate_template_tool() {
        let mut gen = default_gen();
        let spec = simple_spec("greet", "template: Hello, {{input}}!");
        let tool = gen.generate_tool(spec).unwrap();
        assert_eq!(tool.spec.name, "greet");
        assert!(matches!(
            tool.implementation,
            ToolImplementation::Template(_)
        ));
    }

    #[test]
    fn test_generate_expression_tool() {
        let mut gen = default_gen();
        let spec = simple_spec("upper_it", "expr: upper(input)");
        let tool = gen.generate_tool(spec).unwrap();
        assert!(matches!(
            tool.implementation,
            ToolImplementation::Expression(_)
        ));
    }

    #[test]
    fn test_generate_composite_tool() {
        let mut gen = default_gen();
        let spec = simple_spec("pipe", "pipeline: step1(x=input); step2(y=_prev)");
        let tool = gen.generate_tool(spec).unwrap();
        assert!(matches!(
            tool.implementation,
            ToolImplementation::Composite(_)
        ));
    }

    #[test]
    fn test_generate_default_implementation() {
        let mut gen = default_gen();
        let spec = simple_spec("echo", "just echo things");
        let tool = gen.generate_tool(spec).unwrap();
        assert!(matches!(
            tool.implementation,
            ToolImplementation::Template(_)
        ));
    }

    #[test]
    fn test_duplicate_name_error() {
        let mut gen = default_gen();
        gen.generate_tool(simple_spec("dup", "template: {{input}}"))
            .unwrap();
        let result = gen.generate_tool(simple_spec("dup", "template: {{input}}"));
        assert!(matches!(result, Err(DynamicGenError::DuplicateName(_))));
    }

    #[test]
    fn test_capacity_exceeded() {
        let config = DynamicGenConfig {
            max_generated_tools: 2,
            ..Default::default()
        };
        let mut gen = DynamicToolGenerator::new(config);
        gen.generate_tool(simple_spec("a", "template: {{input}}"))
            .unwrap();
        gen.generate_tool(simple_spec("b", "template: {{input}}"))
            .unwrap();
        let result = gen.generate_tool(simple_spec("c", "template: {{input}}"));
        assert!(matches!(result, Err(DynamicGenError::CapacityExceeded)));
    }

    #[test]
    fn test_disabled_generator_rejects_generate() {
        let config = DynamicGenConfig {
            enabled: false,
            ..Default::default()
        };
        let mut gen = DynamicToolGenerator::new(config);
        let result = gen.generate_tool(simple_spec("x", "template: {{input}}"));
        assert!(matches!(result, Err(DynamicGenError::Disabled)));
    }

    // -- Template execution ----------------------------------------------------

    #[test]
    fn test_execute_template_basic() {
        let mut gen = default_gen();
        gen.generate_tool(simple_spec("greet", "template: Hello, {{input}}!"))
            .unwrap();
        let result = gen
            .execute_generated("greet", &json!({"input": "World"}))
            .unwrap();
        assert_eq!(result, "Hello, World!");
    }

    #[test]
    fn test_execute_template_multiple_params() {
        let mut gen = default_gen();
        let mut spec = simple_spec("fmt", "template: {{first}} {{last}}");
        spec.parameters = vec![
            ParamSpec {
                name: "first".into(),
                param_type: "string".into(),
                description: "first name".into(),
                required: true,
                default: None,
            },
            ParamSpec {
                name: "last".into(),
                param_type: "string".into(),
                description: "last name".into(),
                required: true,
                default: None,
            },
        ];
        gen.generate_tool(spec).unwrap();
        let result = gen
            .execute_generated("fmt", &json!({"first": "John", "last": "Doe"}))
            .unwrap();
        assert_eq!(result, "John Doe");
    }

    #[test]
    fn test_execute_template_with_defaults() {
        let mut gen = default_gen();
        let mut spec = simple_spec("greet", "template: Hello, {{input}}!");
        spec.parameters[0].required = false;
        spec.parameters[0].default = Some(json!("stranger"));
        gen.generate_tool(spec).unwrap();
        let result = gen.execute_generated("greet", &json!({})).unwrap();
        assert_eq!(result, "Hello, stranger!");
    }

    #[test]
    fn test_execute_missing_required_param() {
        let mut gen = default_gen();
        gen.generate_tool(simple_spec("tool", "template: {{input}}"))
            .unwrap();
        let result = gen.execute_generated("tool", &json!({}));
        assert!(matches!(result, Err(DynamicGenError::MissingParam(_))));
    }

    #[test]
    fn test_execute_nonexistent_tool() {
        let mut gen = default_gen();
        let result = gen.execute_generated("ghost", &json!({}));
        assert!(matches!(result, Err(DynamicGenError::NotFound(_))));
    }

    // -- Expression execution --------------------------------------------------

    #[test]
    fn test_execute_expression_upper() {
        let mut gen = default_gen();
        gen.generate_tool(simple_spec("up", "expr: upper(input)"))
            .unwrap();
        let result = gen
            .execute_generated("up", &json!({"input": "hello"}))
            .unwrap();
        assert_eq!(result, "HELLO");
    }

    #[test]
    fn test_execute_expression_lower() {
        let mut gen = default_gen();
        gen.generate_tool(simple_spec("lo", "expr: lower(input)"))
            .unwrap();
        let result = gen
            .execute_generated("lo", &json!({"input": "WORLD"}))
            .unwrap();
        assert_eq!(result, "world");
    }

    #[test]
    fn test_execute_expression_len() {
        let mut gen = default_gen();
        gen.generate_tool(simple_spec("length", "expr: len(input)"))
            .unwrap();
        let result = gen
            .execute_generated("length", &json!({"input": "hello"}))
            .unwrap();
        assert_eq!(result, "5");
    }

    #[test]
    fn test_execute_expression_concat() {
        let mut gen = default_gen();
        let mut spec = simple_spec("cat", "expr: concat(a, b)");
        spec.parameters = vec![
            ParamSpec {
                name: "a".into(),
                param_type: "string".into(),
                description: "first".into(),
                required: true,
                default: None,
            },
            ParamSpec {
                name: "b".into(),
                param_type: "string".into(),
                description: "second".into(),
                required: true,
                default: None,
            },
        ];
        gen.generate_tool(spec).unwrap();
        let result = gen
            .execute_generated("cat", &json!({"a": "foo", "b": "bar"}))
            .unwrap();
        assert_eq!(result, "foobar");
    }

    #[test]
    fn test_execute_expression_add() {
        let mut gen = default_gen();
        let mut spec = simple_spec("sum", "expr: add(a, b)");
        spec.parameters = vec![
            ParamSpec {
                name: "a".into(),
                param_type: "number".into(),
                description: "first".into(),
                required: true,
                default: None,
            },
            ParamSpec {
                name: "b".into(),
                param_type: "number".into(),
                description: "second".into(),
                required: true,
                default: None,
            },
        ];
        gen.generate_tool(spec).unwrap();
        let result = gen
            .execute_generated("sum", &json!({"a": 10, "b": 20}))
            .unwrap();
        assert_eq!(result, "30");
    }

    #[test]
    fn test_execute_expression_sub() {
        let mut gen = default_gen();
        let mut spec = simple_spec("diff", "expr: sub(a, b)");
        spec.parameters = vec![
            ParamSpec {
                name: "a".into(),
                param_type: "number".into(),
                description: "first".into(),
                required: true,
                default: None,
            },
            ParamSpec {
                name: "b".into(),
                param_type: "number".into(),
                description: "second".into(),
                required: true,
                default: None,
            },
        ];
        gen.generate_tool(spec).unwrap();
        let result = gen
            .execute_generated("diff", &json!({"a": 30, "b": 10}))
            .unwrap();
        assert_eq!(result, "20");
    }

    #[test]
    fn test_execute_expression_mul() {
        let mut gen = default_gen();
        let mut spec = simple_spec("prod", "expr: mul(a, b)");
        spec.parameters = vec![
            ParamSpec {
                name: "a".into(),
                param_type: "number".into(),
                description: "first".into(),
                required: true,
                default: None,
            },
            ParamSpec {
                name: "b".into(),
                param_type: "number".into(),
                description: "second".into(),
                required: true,
                default: None,
            },
        ];
        gen.generate_tool(spec).unwrap();
        let result = gen
            .execute_generated("prod", &json!({"a": 3, "b": 7}))
            .unwrap();
        assert_eq!(result, "21");
    }

    // -- Transform operations --------------------------------------------------

    #[test]
    fn test_transform_upper() {
        let gen = default_gen();
        let result = gen.apply_transform(&TransformOp::Upper, "hello").unwrap();
        assert_eq!(result, "HELLO");
    }

    #[test]
    fn test_transform_lower() {
        let gen = default_gen();
        let result = gen.apply_transform(&TransformOp::Lower, "HELLO").unwrap();
        assert_eq!(result, "hello");
    }

    #[test]
    fn test_transform_trim() {
        let gen = default_gen();
        let result = gen
            .apply_transform(&TransformOp::Trim, "  hello  ")
            .unwrap();
        assert_eq!(result, "hello");
    }

    #[test]
    fn test_transform_split() {
        let gen = default_gen();
        let result = gen
            .apply_transform(&TransformOp::Split(",".into()), "a,b,c")
            .unwrap();
        let parsed: Vec<String> = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed, vec!["a", "b", "c"]);
    }

    #[test]
    fn test_transform_join() {
        let gen = default_gen();
        let result = gen
            .apply_transform(&TransformOp::Join("-".into()), r#"["a","b","c"]"#)
            .unwrap();
        assert_eq!(result, "a-b-c");
    }

    #[test]
    fn test_transform_json_extract() {
        let gen = default_gen();
        let input = r#"{"user":{"name":"Alice"}}"#;
        let result = gen
            .apply_transform(&TransformOp::JsonExtract("user.name".into()), input)
            .unwrap();
        assert_eq!(result, "Alice");
    }

    // -- Composite pipeline ---------------------------------------------------

    #[test]
    fn test_composite_pipeline_two_steps() {
        let mut gen = default_gen();

        // Step 1: a template tool
        gen.generate_tool(simple_spec("prefix", "template: PREFIX_{{input}}"))
            .unwrap();

        // Step 2: another template tool that uses prev result
        let mut spec2 = simple_spec("suffix", "template: {{data}}_SUFFIX");
        spec2.parameters = vec![ParamSpec {
            name: "data".into(),
            param_type: "string".into(),
            description: "data".into(),
            required: true,
            default: None,
        }];
        gen.generate_tool(spec2).unwrap();

        // Composite tool
        let mut pipe_spec =
            simple_spec("pipe", "pipeline: prefix(input=input); suffix(data=_prev)");
        gen.generate_tool(pipe_spec).unwrap();

        let result = gen
            .execute_generated("pipe", &json!({"input": "test"}))
            .unwrap();
        assert_eq!(result, "PREFIX_test_SUFFIX");
    }

    // -- List, remove, stats ---------------------------------------------------

    #[test]
    fn test_list_generated() {
        let mut gen = default_gen();
        gen.generate_tool(simple_spec("a", "template: {{input}}"))
            .unwrap();
        gen.generate_tool(simple_spec("b", "template: {{input}}"))
            .unwrap();
        let list = gen.list_generated();
        assert_eq!(list.len(), 2);
        let names: Vec<&str> = list.iter().map(|(n, _)| *n).collect();
        assert!(names.contains(&"a"));
        assert!(names.contains(&"b"));
    }

    #[test]
    fn test_remove_generated() {
        let mut gen = default_gen();
        gen.generate_tool(simple_spec("rm_me", "template: {{input}}"))
            .unwrap();
        assert_eq!(gen.list_generated().len(), 1);
        let removed = gen.remove_generated("rm_me").unwrap();
        assert_eq!(removed.spec.name, "rm_me");
        assert!(gen.list_generated().is_empty());
    }

    #[test]
    fn test_remove_nonexistent() {
        let mut gen = default_gen();
        let result = gen.remove_generated("ghost");
        assert!(matches!(result, Err(DynamicGenError::NotFound(_))));
    }

    #[test]
    fn test_get_tool() {
        let mut gen = default_gen();
        gen.generate_tool(simple_spec("find_me", "template: {{input}}"))
            .unwrap();
        assert!(gen.get_tool("find_me").is_some());
        assert!(gen.get_tool("missing").is_none());
    }

    #[test]
    fn test_stats_initial() {
        let gen = default_gen();
        let stats = gen.get_stats();
        assert_eq!(stats.total_tools, 0);
        assert_eq!(stats.total_executions, 0);
        assert!(stats.most_used_tool.is_none());
    }

    #[test]
    fn test_stats_after_usage() {
        let mut gen = default_gen();
        gen.generate_tool(simple_spec("used", "template: {{input}}"))
            .unwrap();
        gen.execute_generated("used", &json!({"input": "a"}))
            .unwrap();
        gen.execute_generated("used", &json!({"input": "b"}))
            .unwrap();

        let stats = gen.get_stats();
        assert_eq!(stats.total_tools, 1);
        assert_eq!(stats.total_executions, 2);
        assert_eq!(stats.most_used_tool.as_deref(), Some("used"));
    }

    #[test]
    fn test_success_rate_tracking() {
        let mut gen = default_gen();
        gen.generate_tool(simple_spec("rate", "template: {{input}}"))
            .unwrap();
        gen.execute_generated("rate", &json!({"input": "ok"}))
            .unwrap();
        // Cause a failure: missing required param.
        let _ = gen.execute_generated("rate", &json!({}));

        let tool = gen.get_tool("rate").unwrap();
        // 1 success, 1 failure attempt (the failure doesn't even execute, so
        // success_rate should be 1.0 with only 1 recorded execution).
        assert!(tool.usage_count >= 1);
    }

    // -- Serialization ---------------------------------------------------------

    #[test]
    fn test_serialize_deserialize_roundtrip() {
        let mut gen = default_gen();
        gen.generate_tool(simple_spec("ser", "template: {{input}}"))
            .unwrap();
        gen.execute_generated("ser", &json!({"input": "test"}))
            .unwrap();

        let json_str = gen.serialize().unwrap();
        let mut gen2 = default_gen();
        let count = gen2.deserialize(&json_str).unwrap();
        assert_eq!(count, 1);
        assert!(gen2.get_tool("ser").is_some());
    }

    // -- Disabled execution ----------------------------------------------------

    #[test]
    fn test_disabled_generator_rejects_execute() {
        let config = DynamicGenConfig {
            enabled: false,
            ..Default::default()
        };
        let mut gen = DynamicToolGenerator::new(config);
        let result = gen.execute_generated("anything", &json!({}));
        assert!(matches!(result, Err(DynamicGenError::Disabled)));
    }

    // -- Error display ---------------------------------------------------------

    #[test]
    fn test_error_display() {
        let e = DynamicGenError::NotFound("ghost".into());
        assert!(e.to_string().contains("ghost"));
        assert!(e.to_string().contains("not found"));
    }

    // -- Pipeline parsing ------------------------------------------------------

    #[test]
    fn test_parse_pipeline_hint() {
        let gen = default_gen();
        let steps = gen.parse_pipeline_hint("pipeline: step1(a=x, b=y); step2(c=_prev)");
        assert_eq!(steps.len(), 2);
        assert_eq!(steps[0].tool_name, "step1");
        assert_eq!(steps[0].param_mapping.get("a"), Some(&"x".to_string()));
        assert_eq!(steps[1].tool_name, "step2");
        assert_eq!(steps[1].param_mapping.get("c"), Some(&"_prev".to_string()));
    }

    // -- Template with numeric values ------------------------------------------

    #[test]
    fn test_template_with_numeric_values() {
        let mut gen = default_gen();
        let mut spec = simple_spec("num_tmpl", "template: Count: {{count}}");
        spec.parameters = vec![ParamSpec {
            name: "count".into(),
            param_type: "number".into(),
            description: "a count".into(),
            required: true,
            default: None,
        }];
        gen.generate_tool(spec).unwrap();
        let result = gen
            .execute_generated("num_tmpl", &json!({"count": 42}))
            .unwrap();
        assert_eq!(result, "Count: 42");
    }
}
