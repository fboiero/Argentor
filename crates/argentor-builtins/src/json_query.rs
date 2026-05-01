//! JSON query and manipulation skill.
//!
//! Provides a comprehensive set of JSON operations including path-based access,
//! deep merge, diff, filtering, sorting, flattening, and basic schema validation.
//!
//! Inspired by LangChain `JsonGetValueTool`/`JsonListKeysTool` and AutoGPT JSON blocks.

use argentor_core::{ArgentorResult, ToolCall, ToolResult};
use argentor_skills::skill::{Skill, SkillDescriptor};
use async_trait::async_trait;
use serde_json::{Map, Value};
use std::cmp::Ordering;

/// Skill that exposes JSON manipulation operations to the agent.
///
/// Supported operations: `get`, `set`, `delete`, `keys`, `values`, `length`,
/// `flatten`, `merge`, `diff`, `filter`, `sort`, `pick`, `omit`, `type_of`, `validate`.
pub struct JsonQuerySkill {
    descriptor: SkillDescriptor,
}

impl JsonQuerySkill {
    /// Create a new `JsonQuerySkill` instance.
    pub fn new() -> Self {
        Self {
            descriptor: SkillDescriptor {
                name: "json_query".to_string(),
                description: "Manipulate and query JSON data. Supports get, set, delete, keys, \
                              values, length, flatten, merge, diff, filter, sort, pick, omit, \
                              type_of, and validate operations."
                    .to_string(),
                parameters_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "operation": {
                            "type": "string",
                            "enum": [
                                "get", "set", "delete", "keys", "values", "length",
                                "flatten", "merge", "diff", "filter", "sort", "pick",
                                "omit", "type_of", "validate"
                            ],
                            "description": "The JSON operation to perform"
                        },
                        "data": {
                            "description": "The JSON value to operate on"
                        },
                        "path": {
                            "type": "string",
                            "description": "Dot-notation path (e.g. 'users.0.name')"
                        },
                        "value": {
                            "description": "Value to set (for 'set' operation) or compare (for 'filter')"
                        },
                        "other": {
                            "description": "Second JSON value (for 'merge' and 'diff')"
                        },
                        "key": {
                            "type": "string",
                            "description": "Key name for 'filter' and 'sort'"
                        },
                        "keys": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "List of keys for 'pick' and 'omit'"
                        },
                        "operator": {
                            "type": "string",
                            "enum": ["eq", "ne", "gt", "lt", "gte", "lte", "contains"],
                            "description": "Comparison operator for 'filter'"
                        },
                        "order": {
                            "type": "string",
                            "enum": ["asc", "desc"],
                            "description": "Sort order (default: 'asc')"
                        },
                        "schema": {
                            "type": "object",
                            "description": "JSON schema for 'validate' (supports type and required)"
                        },
                        "prefix": {
                            "type": "string",
                            "description": "Key prefix for 'flatten'"
                        }
                    },
                    "required": ["operation"]
                }),
                required_capabilities: vec![],
                requires_approval: false,
            },
        }
    }
}

impl Default for JsonQuerySkill {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Skill for JsonQuerySkill {
    fn descriptor(&self) -> &SkillDescriptor {
        &self.descriptor
    }

    async fn execute(&self, call: ToolCall) -> ArgentorResult<ToolResult> {
        let args = &call.arguments;

        let operation = match args.get("operation").and_then(Value::as_str) {
            Some(op) => op,
            None => {
                return Ok(ToolResult::error(
                    &call.id,
                    "Missing required parameter: 'operation'",
                ));
            }
        };

        let result = match operation {
            "get" => op_get(args),
            "set" => op_set(args),
            "delete" => op_delete(args),
            "keys" => op_keys(args),
            "values" => op_values(args),
            "length" => op_length(args),
            "flatten" => op_flatten(args),
            "merge" => op_merge(args),
            "diff" => op_diff(args),
            "filter" => op_filter(args),
            "sort" => op_sort(args),
            "pick" => op_pick(args),
            "omit" => op_omit(args),
            "type_of" => op_type_of(args),
            "validate" => op_validate(args),
            _ => Err(format!("Unknown operation: '{operation}'")),
        };

        match result {
            Ok(value) => Ok(ToolResult::success(&call.id, value.to_string())),
            Err(msg) => Ok(ToolResult::error(&call.id, msg)),
        }
    }
}

// ---------------------------------------------------------------------------
// Helper: require a field from the arguments
// ---------------------------------------------------------------------------

fn require_data(args: &Value) -> Result<&Value, String> {
    args.get("data")
        .ok_or_else(|| "Missing required parameter: 'data'".to_string())
}

fn require_path(args: &Value) -> Result<&str, String> {
    args.get("path")
        .and_then(Value::as_str)
        .ok_or_else(|| "Missing required parameter: 'path'".to_string())
}

// ---------------------------------------------------------------------------
// Path resolution helpers
// ---------------------------------------------------------------------------

/// Resolve a dot-notation path against a JSON value, returning a reference.
fn resolve_path<'a>(root: &'a Value, path: &str) -> Option<&'a Value> {
    if path.is_empty() {
        return Some(root);
    }
    let mut current = root;
    for segment in path.split('.') {
        current = match current {
            Value::Object(map) => map.get(segment)?,
            Value::Array(arr) => {
                let idx: usize = segment.parse().ok()?;
                arr.get(idx)?
            }
            _ => return None,
        };
    }
    Some(current)
}

/// Set a value at a dot-notation path, creating intermediate objects/arrays as needed.
/// Returns the modified root value.
fn set_at_path(root: &Value, path: &str, value: Value) -> Result<Value, String> {
    let segments: Vec<&str> = path.split('.').collect();
    if segments.is_empty() {
        return Ok(value);
    }
    set_recursive(root, &segments, value)
}

fn set_recursive(current: &Value, segments: &[&str], value: Value) -> Result<Value, String> {
    if segments.is_empty() {
        return Ok(value);
    }

    let key = segments[0];
    let rest = &segments[1..];

    match current {
        Value::Object(map) => {
            let mut new_map = map.clone();
            let child = map.get(key).unwrap_or(&Value::Null);
            let new_child = if rest.is_empty() {
                value
            } else {
                // If next segment is a number and child is null, create an array
                let default_child = if rest.first().is_some_and(|s| s.parse::<usize>().is_ok()) {
                    if child.is_null() {
                        Value::Array(vec![])
                    } else {
                        child.clone()
                    }
                } else if child.is_null() {
                    Value::Object(Map::new())
                } else {
                    child.clone()
                };
                set_recursive(&default_child, rest, value)?
            };
            new_map.insert(key.to_string(), new_child);
            Ok(Value::Object(new_map))
        }
        Value::Array(arr) => {
            let idx: usize = key
                .parse()
                .map_err(|_| format!("Expected array index, got '{key}'"))?;
            let mut new_arr = arr.clone();
            // Extend with nulls if index is beyond current length
            while new_arr.len() <= idx {
                new_arr.push(Value::Null);
            }
            let child = &new_arr[idx];
            let new_child = if rest.is_empty() {
                value
            } else {
                let default_child = if child.is_null() {
                    Value::Object(Map::new())
                } else {
                    child.clone()
                };
                set_recursive(&default_child, rest, value)?
            };
            new_arr[idx] = new_child;
            Ok(Value::Array(new_arr))
        }
        _ => {
            // Current node is a scalar; need to replace with object or array
            if let Ok(idx) = key.parse::<usize>() {
                let mut arr = vec![Value::Null; idx + 1];
                let new_child = if rest.is_empty() {
                    value
                } else {
                    set_recursive(&Value::Object(Map::new()), rest, value)?
                };
                arr[idx] = new_child;
                Ok(Value::Array(arr))
            } else {
                let mut map = Map::new();
                let new_child = if rest.is_empty() {
                    value
                } else {
                    set_recursive(&Value::Object(Map::new()), rest, value)?
                };
                map.insert(key.to_string(), new_child);
                Ok(Value::Object(map))
            }
        }
    }
}

/// Delete a value at a dot-notation path.
fn delete_at_path(root: &Value, path: &str) -> Result<Value, String> {
    let segments: Vec<&str> = path.split('.').collect();
    if segments.is_empty() {
        return Ok(Value::Null);
    }
    delete_recursive(root, &segments)
}

fn delete_recursive(current: &Value, segments: &[&str]) -> Result<Value, String> {
    if segments.is_empty() {
        return Ok(current.clone());
    }

    let key = segments[0];
    let rest = &segments[1..];

    match current {
        Value::Object(map) => {
            let mut new_map = map.clone();
            if rest.is_empty() {
                new_map.remove(key);
            } else if let Some(child) = map.get(key) {
                new_map.insert(key.to_string(), delete_recursive(child, rest)?);
            }
            Ok(Value::Object(new_map))
        }
        Value::Array(arr) => {
            let idx: usize = key
                .parse()
                .map_err(|_| format!("Expected array index, got '{key}'"))?;
            if idx >= arr.len() {
                return Ok(current.clone());
            }
            let mut new_arr = arr.clone();
            if rest.is_empty() {
                new_arr.remove(idx);
            } else {
                new_arr[idx] = delete_recursive(&arr[idx], rest)?;
            }
            Ok(Value::Array(new_arr))
        }
        _ => Ok(current.clone()),
    }
}

// ---------------------------------------------------------------------------
// Operations
// ---------------------------------------------------------------------------

fn op_get(args: &Value) -> Result<Value, String> {
    let data = require_data(args)?;
    let path = require_path(args)?;
    match resolve_path(data, path) {
        Some(v) => Ok(serde_json::json!({ "value": v })),
        None => Err(format!("Path '{path}' not found")),
    }
}

fn op_set(args: &Value) -> Result<Value, String> {
    let data = require_data(args)?;
    let path = require_path(args)?;
    let value = args
        .get("value")
        .ok_or_else(|| "Missing required parameter: 'value'".to_string())?
        .clone();
    let result = set_at_path(data, path, value)?;
    Ok(serde_json::json!({ "result": result }))
}

fn op_delete(args: &Value) -> Result<Value, String> {
    let data = require_data(args)?;
    let path = require_path(args)?;
    let result = delete_at_path(data, path)?;
    Ok(serde_json::json!({ "result": result }))
}

fn op_keys(args: &Value) -> Result<Value, String> {
    let data = require_data(args)?;
    match data {
        Value::Object(map) => {
            let keys: Vec<&str> = map.keys().map(String::as_str).collect();
            Ok(serde_json::json!({ "keys": keys }))
        }
        _ => Err("'keys' operation requires an object".to_string()),
    }
}

fn op_values(args: &Value) -> Result<Value, String> {
    let data = require_data(args)?;
    match data {
        Value::Object(map) => {
            let vals: Vec<&Value> = map.values().collect();
            Ok(serde_json::json!({ "values": vals }))
        }
        _ => Err("'values' operation requires an object".to_string()),
    }
}

fn op_length(args: &Value) -> Result<Value, String> {
    let data = require_data(args)?;
    let len = match data {
        Value::Array(arr) => arr.len(),
        Value::Object(map) => map.len(),
        Value::String(s) => s.len(),
        _ => return Err("'length' requires an array, object, or string".to_string()),
    };
    Ok(serde_json::json!({ "length": len }))
}

fn op_flatten(args: &Value) -> Result<Value, String> {
    let data = require_data(args)?;
    let prefix = args
        .get("prefix")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    match data {
        Value::Object(_) => {
            let mut result = Map::new();
            flatten_value(data, &prefix, &mut result);
            Ok(serde_json::json!({ "result": Value::Object(result) }))
        }
        _ => Err("'flatten' operation requires an object".to_string()),
    }
}

fn flatten_value(value: &Value, prefix: &str, out: &mut Map<String, Value>) {
    match value {
        Value::Object(map) => {
            for (k, v) in map {
                let new_key = if prefix.is_empty() {
                    k.clone()
                } else {
                    format!("{prefix}.{k}")
                };
                flatten_value(v, &new_key, out);
            }
        }
        Value::Array(arr) => {
            for (i, v) in arr.iter().enumerate() {
                let new_key = if prefix.is_empty() {
                    i.to_string()
                } else {
                    format!("{prefix}.{i}")
                };
                flatten_value(v, &new_key, out);
            }
        }
        _ => {
            out.insert(prefix.to_string(), value.clone());
        }
    }
}

fn op_merge(args: &Value) -> Result<Value, String> {
    let data = require_data(args)?;
    let other = args
        .get("other")
        .ok_or_else(|| "Missing required parameter: 'other'".to_string())?;
    match (data, other) {
        (Value::Object(_), Value::Object(_)) => {
            let merged = deep_merge(data, other);
            Ok(serde_json::json!({ "result": merged }))
        }
        _ => Err("'merge' operation requires two objects".to_string()),
    }
}

fn deep_merge(base: &Value, overlay: &Value) -> Value {
    match (base, overlay) {
        (Value::Object(base_map), Value::Object(overlay_map)) => {
            let mut result = base_map.clone();
            for (k, v) in overlay_map {
                let merged = if let Some(existing) = result.get(k) {
                    deep_merge(existing, v)
                } else {
                    v.clone()
                };
                result.insert(k.clone(), merged);
            }
            Value::Object(result)
        }
        (_, overlay) => overlay.clone(),
    }
}

fn op_diff(args: &Value) -> Result<Value, String> {
    let data = require_data(args)?;
    let other = args
        .get("other")
        .ok_or_else(|| "Missing required parameter: 'other'".to_string())?;
    match (data, other) {
        (Value::Object(a), Value::Object(b)) => {
            let mut added = Map::new();
            let mut removed = Map::new();
            let mut changed = Map::new();

            // Keys in b but not in a -> added
            for (k, v) in b {
                if !a.contains_key(k) {
                    added.insert(k.clone(), v.clone());
                }
            }

            // Keys in a but not in b -> removed; keys in both but different -> changed
            for (k, v) in a {
                match b.get(k) {
                    None => {
                        removed.insert(k.clone(), v.clone());
                    }
                    Some(bv) if bv != v => {
                        changed.insert(
                            k.clone(),
                            serde_json::json!({
                                "from": v,
                                "to": bv
                            }),
                        );
                    }
                    _ => {}
                }
            }

            Ok(serde_json::json!({
                "added": Value::Object(added),
                "removed": Value::Object(removed),
                "changed": Value::Object(changed)
            }))
        }
        _ => Err("'diff' operation requires two objects".to_string()),
    }
}

fn op_filter(args: &Value) -> Result<Value, String> {
    let data = require_data(args)?;
    let key = args
        .get("key")
        .and_then(Value::as_str)
        .ok_or_else(|| "Missing required parameter: 'key'".to_string())?;
    let operator = args
        .get("operator")
        .and_then(Value::as_str)
        .ok_or_else(|| "Missing required parameter: 'operator'".to_string())?;
    let filter_value = args
        .get("value")
        .ok_or_else(|| "Missing required parameter: 'value'".to_string())?;

    let arr = data
        .as_array()
        .ok_or_else(|| "'filter' operation requires an array of objects".to_string())?;

    let filtered: Vec<&Value> = arr
        .iter()
        .filter(|item| {
            let field = match item.get(key) {
                Some(v) => v,
                None => return false,
            };
            compare_values(field, operator, filter_value)
        })
        .collect();

    Ok(serde_json::json!({ "result": filtered }))
}

fn compare_values(field: &Value, operator: &str, target: &Value) -> bool {
    match operator {
        "eq" => field == target,
        "ne" => field != target,
        "contains" => match (field, target) {
            (Value::String(s), Value::String(t)) => s.contains(t.as_str()),
            (Value::Array(arr), _) => arr.contains(target),
            _ => false,
        },
        "gt" | "lt" | "gte" | "lte" => {
            let ord = numeric_cmp(field, target);
            matches!(
                (operator, ord),
                ("gt", Some(Ordering::Greater))
                    | ("lt", Some(Ordering::Less))
                    | ("gte", Some(Ordering::Greater | Ordering::Equal))
                    | ("lte", Some(Ordering::Less | Ordering::Equal))
            )
        }
        _ => false,
    }
}

fn numeric_cmp(a: &Value, b: &Value) -> Option<Ordering> {
    let a_f = value_as_f64(a)?;
    let b_f = value_as_f64(b)?;
    a_f.partial_cmp(&b_f)
}

fn value_as_f64(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.parse::<f64>().ok(),
        _ => None,
    }
}

fn op_sort(args: &Value) -> Result<Value, String> {
    let data = require_data(args)?;
    let key = args.get("key").and_then(Value::as_str);
    let order = args.get("order").and_then(Value::as_str).unwrap_or("asc");

    let arr = data
        .as_array()
        .ok_or_else(|| "'sort' operation requires an array".to_string())?;

    let mut sorted = arr.clone();

    sorted.sort_by(|a, b| {
        let va = key.map_or(a, |k| a.get(k).unwrap_or(&Value::Null));
        let vb = key.map_or(b, |k| b.get(k).unwrap_or(&Value::Null));
        let cmp = sort_cmp(va, vb);
        if order == "desc" {
            cmp.reverse()
        } else {
            cmp
        }
    });

    Ok(serde_json::json!({ "result": sorted }))
}

fn sort_cmp(a: &Value, b: &Value) -> Ordering {
    // Try numeric comparison first
    if let (Some(af), Some(bf)) = (value_as_f64(a), value_as_f64(b)) {
        return af.partial_cmp(&bf).unwrap_or(Ordering::Equal);
    }
    // Fall back to string comparison
    let sa = value_as_sort_string(a);
    let sb = value_as_sort_string(b);
    sa.cmp(&sb)
}

fn value_as_sort_string(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

fn op_pick(args: &Value) -> Result<Value, String> {
    let data = require_data(args)?;
    let keys = args
        .get("keys")
        .and_then(Value::as_array)
        .ok_or_else(|| "Missing required parameter: 'keys' (array of strings)".to_string())?;

    let map = data
        .as_object()
        .ok_or_else(|| "'pick' operation requires an object".to_string())?;

    let mut result = Map::new();
    for k in keys {
        if let Some(key_str) = k.as_str() {
            if let Some(v) = map.get(key_str) {
                result.insert(key_str.to_string(), v.clone());
            }
        }
    }

    Ok(serde_json::json!({ "result": Value::Object(result) }))
}

fn op_omit(args: &Value) -> Result<Value, String> {
    let data = require_data(args)?;
    let keys = args
        .get("keys")
        .and_then(Value::as_array)
        .ok_or_else(|| "Missing required parameter: 'keys' (array of strings)".to_string())?;

    let map = data
        .as_object()
        .ok_or_else(|| "'omit' operation requires an object".to_string())?;

    let omit_set: Vec<&str> = keys.iter().filter_map(Value::as_str).collect();
    let mut result = Map::new();
    for (k, v) in map {
        if !omit_set.contains(&k.as_str()) {
            result.insert(k.clone(), v.clone());
        }
    }

    Ok(serde_json::json!({ "result": Value::Object(result) }))
}

fn op_type_of(args: &Value) -> Result<Value, String> {
    let data = require_data(args)?;
    let type_name = match data {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    };
    Ok(serde_json::json!({ "type": type_name }))
}

fn op_validate(args: &Value) -> Result<Value, String> {
    let data = require_data(args)?;
    let schema = args
        .get("schema")
        .ok_or_else(|| "Missing required parameter: 'schema'".to_string())?;

    let mut errors: Vec<String> = Vec::new();

    // Type check
    if let Some(expected_type) = schema.get("type").and_then(Value::as_str) {
        let actual = match data {
            Value::Null => "null",
            Value::Bool(_) => "boolean",
            Value::Number(_) => "number",
            Value::String(_) => "string",
            Value::Array(_) => "array",
            Value::Object(_) => "object",
        };
        // JSON Schema allows "integer" as a type
        let type_ok = if expected_type == "integer" {
            data.as_i64().is_some() || data.as_u64().is_some()
        } else {
            actual == expected_type
        };
        if !type_ok {
            errors.push(format!("Expected type '{expected_type}', got '{actual}'"));
        }
    }

    // Required fields check (only for objects)
    if let (Some(required), Some(obj)) = (
        schema.get("required").and_then(Value::as_array),
        data.as_object(),
    ) {
        for req in required {
            if let Some(field) = req.as_str() {
                if !obj.contains_key(field) {
                    errors.push(format!("Missing required field: '{field}'"));
                }
            }
        }
    }

    // Properties type check (only for objects)
    if let (Some(props), Some(obj)) = (
        schema.get("properties").and_then(Value::as_object),
        data.as_object(),
    ) {
        for (prop_name, prop_schema) in props {
            if let Some(field_value) = obj.get(prop_name) {
                if let Some(expected_type) = prop_schema.get("type").and_then(Value::as_str) {
                    let actual = match field_value {
                        Value::Null => "null",
                        Value::Bool(_) => "boolean",
                        Value::Number(_) => "number",
                        Value::String(_) => "string",
                        Value::Array(_) => "array",
                        Value::Object(_) => "object",
                    };
                    let type_ok = if expected_type == "integer" {
                        field_value.as_i64().is_some() || field_value.as_u64().is_some()
                    } else {
                        actual == expected_type
                    };
                    if !type_ok {
                        errors.push(format!(
                            "Field '{prop_name}': expected type '{expected_type}', got '{actual}'"
                        ));
                    }
                }
            }
        }
    }

    // minItems / maxItems check (only for arrays)
    if let Some(arr) = data.as_array() {
        if let Some(min) = schema.get("minItems").and_then(Value::as_u64) {
            if (arr.len() as u64) < min {
                errors.push(format!(
                    "Array length {} is less than minItems {min}",
                    arr.len()
                ));
            }
        }
        if let Some(max) = schema.get("maxItems").and_then(Value::as_u64) {
            if (arr.len() as u64) > max {
                errors.push(format!("Array length {} exceeds maxItems {max}", arr.len()));
            }
        }
    }

    let valid = errors.is_empty();
    Ok(serde_json::json!({
        "valid": valid,
        "errors": errors
    }))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use serde_json::json;

    fn make_call(args: Value) -> ToolCall {
        ToolCall {
            id: "test".to_string(),
            name: "json_query".to_string(),
            arguments: args,
        }
    }

    async fn exec(args: Value) -> ToolResult {
        let skill = JsonQuerySkill::new();
        skill.execute(make_call(args)).await.unwrap()
    }

    fn parse_result(result: &ToolResult) -> Value {
        serde_json::from_str(&result.content).unwrap()
    }

    // -----------------------------------------------------------------------
    // Descriptor
    // -----------------------------------------------------------------------

    #[test]
    fn test_descriptor() {
        let skill = JsonQuerySkill::new();
        let desc = skill.descriptor();
        assert_eq!(desc.name, "json_query");
        assert!(desc.required_capabilities.is_empty());
        assert!(desc.parameters_schema.is_object());
    }

    #[test]
    fn test_default() {
        let skill = JsonQuerySkill::default();
        assert_eq!(skill.descriptor().name, "json_query");
    }

    // -----------------------------------------------------------------------
    // Missing operation
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_missing_operation() {
        let r = exec(json!({})).await;
        assert!(r.is_error);
        assert!(r.content.contains("operation"));
    }

    #[tokio::test]
    async fn test_unknown_operation() {
        let r = exec(json!({"operation": "foo"})).await;
        assert!(r.is_error);
        assert!(r.content.contains("Unknown operation"));
    }

    // -----------------------------------------------------------------------
    // get
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_get_simple() {
        let r = exec(json!({
            "operation": "get",
            "data": {"name": "Alice", "age": 30},
            "path": "name"
        }))
        .await;
        assert!(!r.is_error);
        let v = parse_result(&r);
        assert_eq!(v["value"], "Alice");
    }

    #[tokio::test]
    async fn test_get_nested() {
        let r = exec(json!({
            "operation": "get",
            "data": {"users": [{"name": "Bob"}, {"name": "Carol"}]},
            "path": "users.1.name"
        }))
        .await;
        assert!(!r.is_error);
        let v = parse_result(&r);
        assert_eq!(v["value"], "Carol");
    }

    #[tokio::test]
    async fn test_get_array_index() {
        let r = exec(json!({
            "operation": "get",
            "data": {"items": [10, 20, 30]},
            "path": "items.2"
        }))
        .await;
        assert!(!r.is_error);
        let v = parse_result(&r);
        assert_eq!(v["value"], 30);
    }

    #[tokio::test]
    async fn test_get_not_found() {
        let r = exec(json!({
            "operation": "get",
            "data": {"a": 1},
            "path": "b"
        }))
        .await;
        assert!(r.is_error);
        assert!(r.content.contains("not found"));
    }

    #[tokio::test]
    async fn test_get_missing_data() {
        let r = exec(json!({
            "operation": "get",
            "path": "a"
        }))
        .await;
        assert!(r.is_error);
        assert!(r.content.contains("data"));
    }

    #[tokio::test]
    async fn test_get_missing_path() {
        let r = exec(json!({
            "operation": "get",
            "data": {"a": 1}
        }))
        .await;
        assert!(r.is_error);
        assert!(r.content.contains("path"));
    }

    // -----------------------------------------------------------------------
    // set
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_set_existing_key() {
        let r = exec(json!({
            "operation": "set",
            "data": {"name": "Alice"},
            "path": "name",
            "value": "Bob"
        }))
        .await;
        assert!(!r.is_error);
        let v = parse_result(&r);
        assert_eq!(v["result"]["name"], "Bob");
    }

    #[tokio::test]
    async fn test_set_new_key() {
        let r = exec(json!({
            "operation": "set",
            "data": {"a": 1},
            "path": "b",
            "value": 2
        }))
        .await;
        assert!(!r.is_error);
        let v = parse_result(&r);
        assert_eq!(v["result"]["a"], 1);
        assert_eq!(v["result"]["b"], 2);
    }

    #[tokio::test]
    async fn test_set_nested() {
        let r = exec(json!({
            "operation": "set",
            "data": {"user": {"name": "Alice"}},
            "path": "user.age",
            "value": 30
        }))
        .await;
        assert!(!r.is_error);
        let v = parse_result(&r);
        assert_eq!(v["result"]["user"]["name"], "Alice");
        assert_eq!(v["result"]["user"]["age"], 30);
    }

    #[tokio::test]
    async fn test_set_array_element() {
        let r = exec(json!({
            "operation": "set",
            "data": {"items": [1, 2, 3]},
            "path": "items.1",
            "value": 99
        }))
        .await;
        assert!(!r.is_error);
        let v = parse_result(&r);
        assert_eq!(v["result"]["items"][1], 99);
    }

    #[tokio::test]
    async fn test_set_missing_value() {
        let r = exec(json!({
            "operation": "set",
            "data": {"a": 1},
            "path": "a"
        }))
        .await;
        assert!(r.is_error);
        assert!(r.content.contains("value"));
    }

    // -----------------------------------------------------------------------
    // delete
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_delete_key() {
        let r = exec(json!({
            "operation": "delete",
            "data": {"a": 1, "b": 2},
            "path": "a"
        }))
        .await;
        assert!(!r.is_error);
        let v = parse_result(&r);
        assert!(v["result"].get("a").is_none());
        assert_eq!(v["result"]["b"], 2);
    }

    #[tokio::test]
    async fn test_delete_nested() {
        let r = exec(json!({
            "operation": "delete",
            "data": {"user": {"name": "Alice", "age": 30}},
            "path": "user.age"
        }))
        .await;
        assert!(!r.is_error);
        let v = parse_result(&r);
        assert_eq!(v["result"]["user"]["name"], "Alice");
        assert!(v["result"]["user"].get("age").is_none());
    }

    #[tokio::test]
    async fn test_delete_array_element() {
        let r = exec(json!({
            "operation": "delete",
            "data": {"items": [10, 20, 30]},
            "path": "items.1"
        }))
        .await;
        assert!(!r.is_error);
        let v = parse_result(&r);
        let items = v["result"]["items"].as_array().unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0], 10);
        assert_eq!(items[1], 30);
    }

    #[tokio::test]
    async fn test_delete_nonexistent() {
        let r = exec(json!({
            "operation": "delete",
            "data": {"a": 1},
            "path": "b"
        }))
        .await;
        assert!(!r.is_error);
        let v = parse_result(&r);
        assert_eq!(v["result"]["a"], 1);
    }

    // -----------------------------------------------------------------------
    // keys
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_keys() {
        let r = exec(json!({
            "operation": "keys",
            "data": {"c": 3, "a": 1, "b": 2}
        }))
        .await;
        assert!(!r.is_error);
        let v = parse_result(&r);
        let keys = v["keys"].as_array().unwrap();
        assert_eq!(keys.len(), 3);
        // serde_json preserves insertion order
        assert!(keys.contains(&json!("a")));
        assert!(keys.contains(&json!("b")));
        assert!(keys.contains(&json!("c")));
    }

    #[tokio::test]
    async fn test_keys_not_object() {
        let r = exec(json!({
            "operation": "keys",
            "data": [1, 2, 3]
        }))
        .await;
        assert!(r.is_error);
    }

    // -----------------------------------------------------------------------
    // values
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_values() {
        let r = exec(json!({
            "operation": "values",
            "data": {"a": 1, "b": 2}
        }))
        .await;
        assert!(!r.is_error);
        let v = parse_result(&r);
        let vals = v["values"].as_array().unwrap();
        assert_eq!(vals.len(), 2);
        assert!(vals.contains(&json!(1)));
        assert!(vals.contains(&json!(2)));
    }

    #[tokio::test]
    async fn test_values_not_object() {
        let r = exec(json!({
            "operation": "values",
            "data": "string"
        }))
        .await;
        assert!(r.is_error);
    }

    // -----------------------------------------------------------------------
    // length
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_length_array() {
        let r = exec(json!({
            "operation": "length",
            "data": [1, 2, 3, 4]
        }))
        .await;
        assert!(!r.is_error);
        let v = parse_result(&r);
        assert_eq!(v["length"], 4);
    }

    #[tokio::test]
    async fn test_length_object() {
        let r = exec(json!({
            "operation": "length",
            "data": {"a": 1, "b": 2}
        }))
        .await;
        assert!(!r.is_error);
        let v = parse_result(&r);
        assert_eq!(v["length"], 2);
    }

    #[tokio::test]
    async fn test_length_string() {
        let r = exec(json!({
            "operation": "length",
            "data": "hello"
        }))
        .await;
        assert!(!r.is_error);
        let v = parse_result(&r);
        assert_eq!(v["length"], 5);
    }

    #[tokio::test]
    async fn test_length_invalid() {
        let r = exec(json!({
            "operation": "length",
            "data": 42
        }))
        .await;
        assert!(r.is_error);
    }

    // -----------------------------------------------------------------------
    // flatten
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_flatten_simple() {
        let r = exec(json!({
            "operation": "flatten",
            "data": {"a": {"b": {"c": 1}}, "d": 2}
        }))
        .await;
        assert!(!r.is_error);
        let v = parse_result(&r);
        assert_eq!(v["result"]["a.b.c"], 1);
        assert_eq!(v["result"]["d"], 2);
    }

    #[tokio::test]
    async fn test_flatten_with_prefix() {
        let r = exec(json!({
            "operation": "flatten",
            "data": {"x": 1},
            "prefix": "root"
        }))
        .await;
        assert!(!r.is_error);
        let v = parse_result(&r);
        assert_eq!(v["result"]["root.x"], 1);
    }

    #[tokio::test]
    async fn test_flatten_with_array() {
        let r = exec(json!({
            "operation": "flatten",
            "data": {"items": [10, 20]}
        }))
        .await;
        assert!(!r.is_error);
        let v = parse_result(&r);
        assert_eq!(v["result"]["items.0"], 10);
        assert_eq!(v["result"]["items.1"], 20);
    }

    #[tokio::test]
    async fn test_flatten_not_object() {
        let r = exec(json!({
            "operation": "flatten",
            "data": [1, 2]
        }))
        .await;
        assert!(r.is_error);
    }

    // -----------------------------------------------------------------------
    // merge
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_merge_simple() {
        let r = exec(json!({
            "operation": "merge",
            "data": {"a": 1, "b": 2},
            "other": {"b": 3, "c": 4}
        }))
        .await;
        assert!(!r.is_error);
        let v = parse_result(&r);
        assert_eq!(v["result"]["a"], 1);
        assert_eq!(v["result"]["b"], 3); // overwritten
        assert_eq!(v["result"]["c"], 4);
    }

    #[tokio::test]
    async fn test_merge_deep() {
        let r = exec(json!({
            "operation": "merge",
            "data": {"user": {"name": "Alice", "age": 30}},
            "other": {"user": {"age": 31, "email": "a@b.c"}}
        }))
        .await;
        assert!(!r.is_error);
        let v = parse_result(&r);
        assert_eq!(v["result"]["user"]["name"], "Alice");
        assert_eq!(v["result"]["user"]["age"], 31);
        assert_eq!(v["result"]["user"]["email"], "a@b.c");
    }

    #[tokio::test]
    async fn test_merge_missing_other() {
        let r = exec(json!({
            "operation": "merge",
            "data": {"a": 1}
        }))
        .await;
        assert!(r.is_error);
        assert!(r.content.contains("other"));
    }

    #[tokio::test]
    async fn test_merge_not_objects() {
        let r = exec(json!({
            "operation": "merge",
            "data": [1],
            "other": [2]
        }))
        .await;
        assert!(r.is_error);
    }

    // -----------------------------------------------------------------------
    // diff
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_diff() {
        let r = exec(json!({
            "operation": "diff",
            "data": {"a": 1, "b": 2, "c": 3},
            "other": {"b": 2, "c": 99, "d": 4}
        }))
        .await;
        assert!(!r.is_error);
        let v = parse_result(&r);
        // a was removed
        assert_eq!(v["removed"]["a"], 1);
        // d was added
        assert_eq!(v["added"]["d"], 4);
        // c changed
        assert_eq!(v["changed"]["c"]["from"], 3);
        assert_eq!(v["changed"]["c"]["to"], 99);
        // b unchanged — should not appear anywhere
        assert!(v["added"].get("b").is_none());
        assert!(v["removed"].get("b").is_none());
        assert!(v["changed"].get("b").is_none());
    }

    #[tokio::test]
    async fn test_diff_identical() {
        let r = exec(json!({
            "operation": "diff",
            "data": {"a": 1},
            "other": {"a": 1}
        }))
        .await;
        assert!(!r.is_error);
        let v = parse_result(&r);
        assert!(v["added"].as_object().unwrap().is_empty());
        assert!(v["removed"].as_object().unwrap().is_empty());
        assert!(v["changed"].as_object().unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_diff_not_objects() {
        let r = exec(json!({
            "operation": "diff",
            "data": 1,
            "other": 2
        }))
        .await;
        assert!(r.is_error);
    }

    // -----------------------------------------------------------------------
    // filter
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_filter_eq() {
        let r = exec(json!({
            "operation": "filter",
            "data": [
                {"name": "Alice", "age": 30},
                {"name": "Bob", "age": 25},
                {"name": "Carol", "age": 30}
            ],
            "key": "age",
            "operator": "eq",
            "value": 30
        }))
        .await;
        assert!(!r.is_error);
        let v = parse_result(&r);
        let results = v["result"].as_array().unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0]["name"], "Alice");
        assert_eq!(results[1]["name"], "Carol");
    }

    #[tokio::test]
    async fn test_filter_gt() {
        let r = exec(json!({
            "operation": "filter",
            "data": [
                {"val": 10},
                {"val": 20},
                {"val": 30}
            ],
            "key": "val",
            "operator": "gt",
            "value": 15
        }))
        .await;
        assert!(!r.is_error);
        let v = parse_result(&r);
        let results = v["result"].as_array().unwrap();
        assert_eq!(results.len(), 2);
    }

    #[tokio::test]
    async fn test_filter_lt() {
        let r = exec(json!({
            "operation": "filter",
            "data": [{"v": 1}, {"v": 5}, {"v": 10}],
            "key": "v",
            "operator": "lt",
            "value": 5
        }))
        .await;
        assert!(!r.is_error);
        let v = parse_result(&r);
        let results = v["result"].as_array().unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0]["v"], 1);
    }

    #[tokio::test]
    async fn test_filter_gte() {
        let r = exec(json!({
            "operation": "filter",
            "data": [{"v": 1}, {"v": 5}, {"v": 10}],
            "key": "v",
            "operator": "gte",
            "value": 5
        }))
        .await;
        assert!(!r.is_error);
        let v = parse_result(&r);
        assert_eq!(v["result"].as_array().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn test_filter_lte() {
        let r = exec(json!({
            "operation": "filter",
            "data": [{"v": 1}, {"v": 5}, {"v": 10}],
            "key": "v",
            "operator": "lte",
            "value": 5
        }))
        .await;
        assert!(!r.is_error);
        let v = parse_result(&r);
        assert_eq!(v["result"].as_array().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn test_filter_ne() {
        let r = exec(json!({
            "operation": "filter",
            "data": [{"v": 1}, {"v": 2}, {"v": 3}],
            "key": "v",
            "operator": "ne",
            "value": 2
        }))
        .await;
        assert!(!r.is_error);
        let v = parse_result(&r);
        assert_eq!(v["result"].as_array().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn test_filter_contains() {
        let r = exec(json!({
            "operation": "filter",
            "data": [
                {"name": "Alice"},
                {"name": "Bob"},
                {"name": "Alicia"}
            ],
            "key": "name",
            "operator": "contains",
            "value": "Ali"
        }))
        .await;
        assert!(!r.is_error);
        let v = parse_result(&r);
        assert_eq!(v["result"].as_array().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn test_filter_missing_key() {
        let r = exec(json!({
            "operation": "filter",
            "data": [{"a": 1}],
            "operator": "eq",
            "value": 1
        }))
        .await;
        assert!(r.is_error);
        assert!(r.content.contains("key"));
    }

    #[tokio::test]
    async fn test_filter_not_array() {
        let r = exec(json!({
            "operation": "filter",
            "data": {"a": 1},
            "key": "a",
            "operator": "eq",
            "value": 1
        }))
        .await;
        assert!(r.is_error);
    }

    // -----------------------------------------------------------------------
    // sort
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_sort_numbers_asc() {
        let r = exec(json!({
            "operation": "sort",
            "data": [3, 1, 2]
        }))
        .await;
        assert!(!r.is_error);
        let v = parse_result(&r);
        let arr = v["result"].as_array().unwrap();
        assert_eq!(arr, &[json!(1), json!(2), json!(3)]);
    }

    #[tokio::test]
    async fn test_sort_numbers_desc() {
        let r = exec(json!({
            "operation": "sort",
            "data": [3, 1, 2],
            "order": "desc"
        }))
        .await;
        assert!(!r.is_error);
        let v = parse_result(&r);
        let arr = v["result"].as_array().unwrap();
        assert_eq!(arr, &[json!(3), json!(2), json!(1)]);
    }

    #[tokio::test]
    async fn test_sort_by_key() {
        let r = exec(json!({
            "operation": "sort",
            "data": [
                {"name": "Charlie", "age": 25},
                {"name": "Alice", "age": 30},
                {"name": "Bob", "age": 20}
            ],
            "key": "age"
        }))
        .await;
        assert!(!r.is_error);
        let v = parse_result(&r);
        let arr = v["result"].as_array().unwrap();
        assert_eq!(arr[0]["name"], "Bob");
        assert_eq!(arr[1]["name"], "Charlie");
        assert_eq!(arr[2]["name"], "Alice");
    }

    #[tokio::test]
    async fn test_sort_by_key_desc() {
        let r = exec(json!({
            "operation": "sort",
            "data": [
                {"name": "A"},
                {"name": "C"},
                {"name": "B"}
            ],
            "key": "name",
            "order": "desc"
        }))
        .await;
        assert!(!r.is_error);
        let v = parse_result(&r);
        let arr = v["result"].as_array().unwrap();
        assert_eq!(arr[0]["name"], "C");
        assert_eq!(arr[1]["name"], "B");
        assert_eq!(arr[2]["name"], "A");
    }

    #[tokio::test]
    async fn test_sort_not_array() {
        let r = exec(json!({
            "operation": "sort",
            "data": {"a": 1}
        }))
        .await;
        assert!(r.is_error);
    }

    // -----------------------------------------------------------------------
    // pick
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_pick() {
        let r = exec(json!({
            "operation": "pick",
            "data": {"a": 1, "b": 2, "c": 3},
            "keys": ["a", "c"]
        }))
        .await;
        assert!(!r.is_error);
        let v = parse_result(&r);
        assert_eq!(v["result"]["a"], 1);
        assert_eq!(v["result"]["c"], 3);
        assert!(v["result"].get("b").is_none());
    }

    #[tokio::test]
    async fn test_pick_missing_key() {
        let r = exec(json!({
            "operation": "pick",
            "data": {"a": 1},
            "keys": ["a", "z"]
        }))
        .await;
        assert!(!r.is_error);
        let v = parse_result(&r);
        assert_eq!(v["result"]["a"], 1);
        assert!(v["result"].get("z").is_none());
    }

    #[tokio::test]
    async fn test_pick_not_object() {
        let r = exec(json!({
            "operation": "pick",
            "data": [1, 2],
            "keys": ["a"]
        }))
        .await;
        assert!(r.is_error);
    }

    #[tokio::test]
    async fn test_pick_missing_keys_param() {
        let r = exec(json!({
            "operation": "pick",
            "data": {"a": 1}
        }))
        .await;
        assert!(r.is_error);
        assert!(r.content.contains("keys"));
    }

    // -----------------------------------------------------------------------
    // omit
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_omit() {
        let r = exec(json!({
            "operation": "omit",
            "data": {"a": 1, "b": 2, "c": 3},
            "keys": ["b"]
        }))
        .await;
        assert!(!r.is_error);
        let v = parse_result(&r);
        assert_eq!(v["result"]["a"], 1);
        assert_eq!(v["result"]["c"], 3);
        assert!(v["result"].get("b").is_none());
    }

    #[tokio::test]
    async fn test_omit_not_object() {
        let r = exec(json!({
            "operation": "omit",
            "data": 42,
            "keys": ["a"]
        }))
        .await;
        assert!(r.is_error);
    }

    // -----------------------------------------------------------------------
    // type_of
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_type_of_string() {
        let r = exec(json!({"operation": "type_of", "data": "hello"})).await;
        assert!(!r.is_error);
        let v = parse_result(&r);
        assert_eq!(v["type"], "string");
    }

    #[tokio::test]
    async fn test_type_of_number() {
        let r = exec(json!({"operation": "type_of", "data": 42})).await;
        assert!(!r.is_error);
        let v = parse_result(&r);
        assert_eq!(v["type"], "number");
    }

    #[tokio::test]
    async fn test_type_of_boolean() {
        let r = exec(json!({"operation": "type_of", "data": true})).await;
        assert!(!r.is_error);
        let v = parse_result(&r);
        assert_eq!(v["type"], "boolean");
    }

    #[tokio::test]
    async fn test_type_of_null() {
        let r = exec(json!({"operation": "type_of", "data": null})).await;
        assert!(!r.is_error);
        let v = parse_result(&r);
        assert_eq!(v["type"], "null");
    }

    #[tokio::test]
    async fn test_type_of_array() {
        let r = exec(json!({"operation": "type_of", "data": [1, 2]})).await;
        assert!(!r.is_error);
        let v = parse_result(&r);
        assert_eq!(v["type"], "array");
    }

    #[tokio::test]
    async fn test_type_of_object() {
        let r = exec(json!({"operation": "type_of", "data": {"a": 1}})).await;
        assert!(!r.is_error);
        let v = parse_result(&r);
        assert_eq!(v["type"], "object");
    }

    // -----------------------------------------------------------------------
    // validate
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_validate_valid() {
        let r = exec(json!({
            "operation": "validate",
            "data": {"name": "Alice", "age": 30},
            "schema": {
                "type": "object",
                "required": ["name", "age"],
                "properties": {
                    "name": {"type": "string"},
                    "age": {"type": "number"}
                }
            }
        }))
        .await;
        assert!(!r.is_error);
        let v = parse_result(&r);
        assert_eq!(v["valid"], true);
        assert!(v["errors"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_validate_wrong_type() {
        let r = exec(json!({
            "operation": "validate",
            "data": "not an object",
            "schema": {"type": "object"}
        }))
        .await;
        assert!(!r.is_error);
        let v = parse_result(&r);
        assert_eq!(v["valid"], false);
        let errors = v["errors"].as_array().unwrap();
        assert!(!errors.is_empty());
    }

    #[tokio::test]
    async fn test_validate_missing_required() {
        let r = exec(json!({
            "operation": "validate",
            "data": {"name": "Alice"},
            "schema": {
                "type": "object",
                "required": ["name", "email"]
            }
        }))
        .await;
        assert!(!r.is_error);
        let v = parse_result(&r);
        assert_eq!(v["valid"], false);
        let errors: Vec<String> = v["errors"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e.as_str().unwrap().to_string())
            .collect();
        assert!(errors.iter().any(|e| e.contains("email")));
    }

    #[tokio::test]
    async fn test_validate_property_type_mismatch() {
        let r = exec(json!({
            "operation": "validate",
            "data": {"age": "thirty"},
            "schema": {
                "type": "object",
                "properties": {
                    "age": {"type": "number"}
                }
            }
        }))
        .await;
        assert!(!r.is_error);
        let v = parse_result(&r);
        assert_eq!(v["valid"], false);
    }

    #[tokio::test]
    async fn test_validate_integer_type() {
        let r = exec(json!({
            "operation": "validate",
            "data": 42,
            "schema": {"type": "integer"}
        }))
        .await;
        assert!(!r.is_error);
        let v = parse_result(&r);
        assert_eq!(v["valid"], true);
    }

    #[tokio::test]
    async fn test_validate_array_min_items() {
        let r = exec(json!({
            "operation": "validate",
            "data": [1],
            "schema": {"type": "array", "minItems": 3}
        }))
        .await;
        assert!(!r.is_error);
        let v = parse_result(&r);
        assert_eq!(v["valid"], false);
    }

    #[tokio::test]
    async fn test_validate_array_max_items() {
        let r = exec(json!({
            "operation": "validate",
            "data": [1, 2, 3, 4, 5],
            "schema": {"type": "array", "maxItems": 3}
        }))
        .await;
        assert!(!r.is_error);
        let v = parse_result(&r);
        assert_eq!(v["valid"], false);
    }

    #[tokio::test]
    async fn test_validate_missing_schema() {
        let r = exec(json!({
            "operation": "validate",
            "data": {}
        }))
        .await;
        assert!(r.is_error);
        assert!(r.content.contains("schema"));
    }

    // -----------------------------------------------------------------------
    // Edge cases & integration
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_get_deeply_nested() {
        let r = exec(json!({
            "operation": "get",
            "data": {"a": {"b": {"c": {"d": {"e": 42}}}}},
            "path": "a.b.c.d.e"
        }))
        .await;
        assert!(!r.is_error);
        let v = parse_result(&r);
        assert_eq!(v["value"], 42);
    }

    #[tokio::test]
    async fn test_set_creates_intermediate_objects() {
        let r = exec(json!({
            "operation": "set",
            "data": {},
            "path": "a.b.c",
            "value": "deep"
        }))
        .await;
        assert!(!r.is_error);
        let v = parse_result(&r);
        assert_eq!(v["result"]["a"]["b"]["c"], "deep");
    }

    #[tokio::test]
    async fn test_flatten_empty_object() {
        let r = exec(json!({
            "operation": "flatten",
            "data": {}
        }))
        .await;
        assert!(!r.is_error);
        let v = parse_result(&r);
        assert!(v["result"].as_object().unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_filter_empty_array() {
        let r = exec(json!({
            "operation": "filter",
            "data": [],
            "key": "x",
            "operator": "eq",
            "value": 1
        }))
        .await;
        assert!(!r.is_error);
        let v = parse_result(&r);
        assert!(v["result"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_sort_empty_array() {
        let r = exec(json!({
            "operation": "sort",
            "data": []
        }))
        .await;
        assert!(!r.is_error);
        let v = parse_result(&r);
        assert!(v["result"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_sort_strings() {
        let r = exec(json!({
            "operation": "sort",
            "data": ["banana", "apple", "cherry"]
        }))
        .await;
        assert!(!r.is_error);
        let v = parse_result(&r);
        let arr = v["result"].as_array().unwrap();
        assert_eq!(arr[0], "apple");
        assert_eq!(arr[1], "banana");
        assert_eq!(arr[2], "cherry");
    }

    #[tokio::test]
    async fn test_merge_empty_objects() {
        let r = exec(json!({
            "operation": "merge",
            "data": {},
            "other": {}
        }))
        .await;
        assert!(!r.is_error);
        let v = parse_result(&r);
        assert!(v["result"].as_object().unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_diff_empty_objects() {
        let r = exec(json!({
            "operation": "diff",
            "data": {},
            "other": {}
        }))
        .await;
        assert!(!r.is_error);
        let v = parse_result(&r);
        assert!(v["added"].as_object().unwrap().is_empty());
        assert!(v["removed"].as_object().unwrap().is_empty());
        assert!(v["changed"].as_object().unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_pick_empty_keys() {
        let r = exec(json!({
            "operation": "pick",
            "data": {"a": 1, "b": 2},
            "keys": []
        }))
        .await;
        assert!(!r.is_error);
        let v = parse_result(&r);
        assert!(v["result"].as_object().unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_omit_empty_keys() {
        let r = exec(json!({
            "operation": "omit",
            "data": {"a": 1, "b": 2},
            "keys": []
        }))
        .await;
        assert!(!r.is_error);
        let v = parse_result(&r);
        assert_eq!(v["result"]["a"], 1);
        assert_eq!(v["result"]["b"], 2);
    }

    #[tokio::test]
    async fn test_filter_contains_array() {
        let r = exec(json!({
            "operation": "filter",
            "data": [
                {"tags": ["rust", "wasm"]},
                {"tags": ["python"]},
                {"tags": ["rust", "security"]}
            ],
            "key": "tags",
            "operator": "contains",
            "value": "rust"
        }))
        .await;
        assert!(!r.is_error);
        let v = parse_result(&r);
        assert_eq!(v["result"].as_array().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn test_validate_all_pass() {
        let r = exec(json!({
            "operation": "validate",
            "data": [1, 2, 3],
            "schema": {
                "type": "array",
                "minItems": 1,
                "maxItems": 5
            }
        }))
        .await;
        assert!(!r.is_error);
        let v = parse_result(&r);
        assert_eq!(v["valid"], true);
    }

    #[tokio::test]
    async fn test_keys_empty_object() {
        let r = exec(json!({
            "operation": "keys",
            "data": {}
        }))
        .await;
        assert!(!r.is_error);
        let v = parse_result(&r);
        assert!(v["keys"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_values_empty_object() {
        let r = exec(json!({
            "operation": "values",
            "data": {}
        }))
        .await;
        assert!(!r.is_error);
        let v = parse_result(&r);
        assert!(v["values"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_length_empty_array() {
        let r = exec(json!({
            "operation": "length",
            "data": []
        }))
        .await;
        assert!(!r.is_error);
        let v = parse_result(&r);
        assert_eq!(v["length"], 0);
    }

    #[tokio::test]
    async fn test_delete_from_array_out_of_bounds() {
        let r = exec(json!({
            "operation": "delete",
            "data": {"items": [1, 2]},
            "path": "items.5"
        }))
        .await;
        assert!(!r.is_error);
        // Array unchanged since index is out of bounds
        let v = parse_result(&r);
        let items = v["result"]["items"].as_array().unwrap();
        assert_eq!(items.len(), 2);
    }

    #[tokio::test]
    async fn test_set_extend_array() {
        let r = exec(json!({
            "operation": "set",
            "data": {"items": [1]},
            "path": "items.3",
            "value": 99
        }))
        .await;
        assert!(!r.is_error);
        let v = parse_result(&r);
        let items = v["result"]["items"].as_array().unwrap();
        assert_eq!(items.len(), 4);
        assert_eq!(items[3], 99);
    }

    #[tokio::test]
    async fn test_filter_with_missing_field_in_some_items() {
        let r = exec(json!({
            "operation": "filter",
            "data": [
                {"name": "Alice", "score": 90},
                {"name": "Bob"},
                {"name": "Carol", "score": 85}
            ],
            "key": "score",
            "operator": "gte",
            "value": 85
        }))
        .await;
        assert!(!r.is_error);
        let v = parse_result(&r);
        // Bob is excluded because he has no "score" field
        assert_eq!(v["result"].as_array().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn test_sort_with_missing_key() {
        let r = exec(json!({
            "operation": "sort",
            "data": [
                {"name": "C", "val": 3},
                {"name": "A"},
                {"name": "B", "val": 1}
            ],
            "key": "val"
        }))
        .await;
        assert!(!r.is_error);
        // Items with missing key get Value::Null, sorted to beginning
        let v = parse_result(&r);
        let arr = v["result"].as_array().unwrap();
        assert_eq!(arr.len(), 3);
    }
}
