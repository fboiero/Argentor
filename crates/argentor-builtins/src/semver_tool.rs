//! Semantic versioning skill for the Argentor AI agent framework.
//!
//! Provides semver parsing, comparison, bumping (major/minor/patch),
//! range matching, and validation.

use argentor_core::{ArgentorResult, ToolCall, ToolResult};
use argentor_skills::skill::{Skill, SkillDescriptor};
use async_trait::async_trait;
use serde_json::json;
use std::cmp::Ordering;

/// Semantic versioning skill for parsing, comparing, and bumping versions.
pub struct SemverToolSkill {
    descriptor: SkillDescriptor,
}

impl SemverToolSkill {
    /// Create a new semver tool skill.
    pub fn new() -> Self {
        Self {
            descriptor: SkillDescriptor {
                name: "semver_tool".to_string(),
                description: "Semantic version parse, compare, bump (major/minor/patch), range matching, and validation.".to_string(),
                parameters_schema: json!({
                    "type": "object",
                    "properties": {
                        "operation": {
                            "type": "string",
                            "enum": ["parse", "validate", "compare", "bump", "sort", "satisfies", "max", "min"],
                            "description": "The semver operation to perform"
                        },
                        "version": {
                            "type": "string",
                            "description": "Version string (e.g., 1.2.3)"
                        },
                        "version_a": {
                            "type": "string",
                            "description": "First version for comparison"
                        },
                        "version_b": {
                            "type": "string",
                            "description": "Second version for comparison"
                        },
                        "bump_type": {
                            "type": "string",
                            "enum": ["major", "minor", "patch"],
                            "description": "Type of version bump"
                        },
                        "versions": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "List of versions to sort/max/min"
                        },
                        "range": {
                            "type": "string",
                            "description": "Version range (e.g., >=1.0.0, ^1.2.0, ~1.2.0)"
                        }
                    },
                    "required": ["operation"]
                }),
                required_capabilities: vec![],
            },
        }
    }
}

impl Default for SemverToolSkill {
    fn default() -> Self {
        Self::new()
    }
}

/// Parsed semantic version.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Semver {
    major: u64,
    minor: u64,
    patch: u64,
    prerelease: Option<String>,
    build: Option<String>,
}

impl Semver {
    fn parse(version: &str) -> Result<Self, String> {
        let version = version.trim().strip_prefix('v').unwrap_or(version);

        // Split off build metadata
        let (version, build) = if let Some((v, b)) = version.split_once('+') {
            (v, Some(b.to_string()))
        } else {
            (version, None)
        };

        // Split off prerelease
        let (version, prerelease) = if let Some((v, p)) = version.split_once('-') {
            (v, Some(p.to_string()))
        } else {
            (version, None)
        };

        let parts: Vec<&str> = version.split('.').collect();
        if parts.len() != 3 {
            return Err(format!(
                "Invalid semver: expected 3 numeric parts (X.Y.Z), got '{version}'"
            ));
        }

        let major: u64 = parts[0]
            .parse()
            .map_err(|_| format!("Invalid major version: '{}'", parts[0]))?;
        let minor: u64 = parts[1]
            .parse()
            .map_err(|_| format!("Invalid minor version: '{}'", parts[1]))?;
        let patch: u64 = parts[2]
            .parse()
            .map_err(|_| format!("Invalid patch version: '{}'", parts[2]))?;

        Ok(Self {
            major,
            minor,
            patch,
            prerelease,
            build,
        })
    }

    fn to_string_clean(&self) -> String {
        let mut s = format!("{}.{}.{}", self.major, self.minor, self.patch);
        if let Some(ref pre) = self.prerelease {
            s.push('-');
            s.push_str(pre);
        }
        if let Some(ref build) = self.build {
            s.push('+');
            s.push_str(build);
        }
        s
    }

    fn cmp_version(&self, other: &Self) -> Ordering {
        self.major
            .cmp(&other.major)
            .then(self.minor.cmp(&other.minor))
            .then(self.patch.cmp(&other.patch))
            .then_with(|| {
                // Pre-release has lower precedence than release
                match (&self.prerelease, &other.prerelease) {
                    (None, None) => Ordering::Equal,
                    (Some(_), None) => Ordering::Less,
                    (None, Some(_)) => Ordering::Greater,
                    (Some(a), Some(b)) => a.cmp(b),
                }
            })
    }

    fn bump(&self, bump_type: &str) -> Result<Self, String> {
        match bump_type {
            "major" => Ok(Self {
                major: self.major + 1,
                minor: 0,
                patch: 0,
                prerelease: None,
                build: None,
            }),
            "minor" => Ok(Self {
                major: self.major,
                minor: self.minor + 1,
                patch: 0,
                prerelease: None,
                build: None,
            }),
            "patch" => Ok(Self {
                major: self.major,
                minor: self.minor,
                patch: self.patch + 1,
                prerelease: None,
                build: None,
            }),
            _ => Err(format!(
                "Unknown bump type: '{bump_type}'. Use major, minor, or patch."
            )),
        }
    }
}

/// Check if a version satisfies a simple range constraint.
fn satisfies_range(version: &Semver, range: &str) -> Result<bool, String> {
    let range = range.trim();

    // Handle caret ranges (^): compatible with version
    if let Some(base) = range.strip_prefix('^') {
        let base = Semver::parse(base)?;
        if version.major != base.major {
            return Ok(false);
        }
        if base.major == 0 {
            if version.minor != base.minor {
                return Ok(false);
            }
            return Ok(version.cmp_version(&base) != Ordering::Less);
        }
        return Ok(version.cmp_version(&base) != Ordering::Less);
    }

    // Handle tilde ranges (~): approximately equivalent
    if let Some(base) = range.strip_prefix('~') {
        let base = Semver::parse(base)?;
        if version.major != base.major || version.minor != base.minor {
            return Ok(false);
        }
        return Ok(version.patch >= base.patch);
    }

    // Handle comparison operators
    if let Some(ver) = range.strip_prefix(">=") {
        let target = Semver::parse(ver.trim())?;
        return Ok(version.cmp_version(&target) != Ordering::Less);
    }
    if let Some(ver) = range.strip_prefix("<=") {
        let target = Semver::parse(ver.trim())?;
        return Ok(version.cmp_version(&target) != Ordering::Greater);
    }
    if let Some(ver) = range.strip_prefix('>') {
        let target = Semver::parse(ver.trim())?;
        return Ok(version.cmp_version(&target) == Ordering::Greater);
    }
    if let Some(ver) = range.strip_prefix('<') {
        let target = Semver::parse(ver.trim())?;
        return Ok(version.cmp_version(&target) == Ordering::Less);
    }
    if let Some(ver) = range.strip_prefix('=') {
        let target = Semver::parse(ver.trim())?;
        return Ok(version.cmp_version(&target) == Ordering::Equal);
    }

    // Exact match
    let target = Semver::parse(range)?;
    Ok(version.cmp_version(&target) == Ordering::Equal)
}

#[async_trait]
impl Skill for SemverToolSkill {
    fn descriptor(&self) -> &SkillDescriptor {
        &self.descriptor
    }

    async fn execute(&self, call: ToolCall) -> ArgentorResult<ToolResult> {
        let operation = match call.arguments["operation"].as_str() {
            Some(op) => op,
            None => {
                return Ok(ToolResult::error(
                    &call.id,
                    "Missing required parameter: 'operation'",
                ))
            }
        };

        match operation {
            "parse" => {
                let version = match call.arguments["version"].as_str() {
                    Some(v) => v,
                    None => return Ok(ToolResult::error(&call.id, "Missing required parameter: 'version'")),
                };
                match Semver::parse(version) {
                    Ok(sv) => {
                        let response = json!({
                            "major": sv.major,
                            "minor": sv.minor,
                            "patch": sv.patch,
                            "prerelease": sv.prerelease,
                            "build": sv.build,
                            "normalized": sv.to_string_clean()
                        });
                        Ok(ToolResult::success(&call.id, response.to_string()))
                    }
                    Err(e) => Ok(ToolResult::error(&call.id, e)),
                }
            }
            "validate" => {
                let version = match call.arguments["version"].as_str() {
                    Some(v) => v,
                    None => return Ok(ToolResult::error(&call.id, "Missing required parameter: 'version'")),
                };
                let valid = Semver::parse(version).is_ok();
                let response = json!({ "valid": valid, "input": version });
                Ok(ToolResult::success(&call.id, response.to_string()))
            }
            "compare" => {
                let va = match call.arguments["version_a"].as_str() {
                    Some(v) => v,
                    None => return Ok(ToolResult::error(&call.id, "Missing required parameter: 'version_a'")),
                };
                let vb = match call.arguments["version_b"].as_str() {
                    Some(v) => v,
                    None => return Ok(ToolResult::error(&call.id, "Missing required parameter: 'version_b'")),
                };
                let a = match Semver::parse(va) {
                    Ok(v) => v,
                    Err(e) => return Ok(ToolResult::error(&call.id, format!("version_a: {e}"))),
                };
                let b = match Semver::parse(vb) {
                    Ok(v) => v,
                    Err(e) => return Ok(ToolResult::error(&call.id, format!("version_b: {e}"))),
                };
                let cmp_result = match a.cmp_version(&b) {
                    Ordering::Less => "less",
                    Ordering::Equal => "equal",
                    Ordering::Greater => "greater",
                };
                let response = json!({
                    "version_a": va,
                    "version_b": vb,
                    "result": cmp_result,
                    "a_is_newer": a.cmp_version(&b) == Ordering::Greater
                });
                Ok(ToolResult::success(&call.id, response.to_string()))
            }
            "bump" => {
                let version = match call.arguments["version"].as_str() {
                    Some(v) => v,
                    None => return Ok(ToolResult::error(&call.id, "Missing required parameter: 'version'")),
                };
                let bump_type = match call.arguments["bump_type"].as_str() {
                    Some(v) => v,
                    None => return Ok(ToolResult::error(&call.id, "Missing required parameter: 'bump_type'")),
                };
                let sv = match Semver::parse(version) {
                    Ok(v) => v,
                    Err(e) => return Ok(ToolResult::error(&call.id, e)),
                };
                match sv.bump(bump_type) {
                    Ok(bumped) => {
                        let response = json!({
                            "original": version,
                            "bumped": bumped.to_string_clean(),
                            "bump_type": bump_type
                        });
                        Ok(ToolResult::success(&call.id, response.to_string()))
                    }
                    Err(e) => Ok(ToolResult::error(&call.id, e)),
                }
            }
            "sort" => {
                let versions: Vec<String> = match call.arguments["versions"].as_array() {
                    Some(arr) => arr.iter().filter_map(|v| v.as_str().map(String::from)).collect(),
                    None => return Ok(ToolResult::error(&call.id, "Missing required parameter: 'versions'")),
                };
                let mut parsed: Vec<(String, Semver)> = Vec::new();
                for v in &versions {
                    match Semver::parse(v) {
                        Ok(sv) => parsed.push((v.clone(), sv)),
                        Err(e) => return Ok(ToolResult::error(&call.id, format!("Invalid version '{v}': {e}"))),
                    }
                }
                parsed.sort_by(|a, b| a.1.cmp_version(&b.1));
                let sorted: Vec<String> = parsed.into_iter().map(|(s, _)| s).collect();
                let response = json!({ "sorted": sorted });
                Ok(ToolResult::success(&call.id, response.to_string()))
            }
            "satisfies" => {
                let version = match call.arguments["version"].as_str() {
                    Some(v) => v,
                    None => return Ok(ToolResult::error(&call.id, "Missing required parameter: 'version'")),
                };
                let range = match call.arguments["range"].as_str() {
                    Some(v) => v,
                    None => return Ok(ToolResult::error(&call.id, "Missing required parameter: 'range'")),
                };
                let sv = match Semver::parse(version) {
                    Ok(v) => v,
                    Err(e) => return Ok(ToolResult::error(&call.id, e)),
                };
                match satisfies_range(&sv, range) {
                    Ok(satisfies) => {
                        let response = json!({
                            "version": version,
                            "range": range,
                            "satisfies": satisfies
                        });
                        Ok(ToolResult::success(&call.id, response.to_string()))
                    }
                    Err(e) => Ok(ToolResult::error(&call.id, e)),
                }
            }
            "max" => {
                let versions: Vec<String> = match call.arguments["versions"].as_array() {
                    Some(arr) => arr.iter().filter_map(|v| v.as_str().map(String::from)).collect(),
                    None => return Ok(ToolResult::error(&call.id, "Missing required parameter: 'versions'")),
                };
                if versions.is_empty() {
                    return Ok(ToolResult::error(&call.id, "Versions array is empty"));
                }
                let mut max_ver: Option<(String, Semver)> = None;
                for v in &versions {
                    match Semver::parse(v) {
                        Ok(sv) => {
                            if let Some((_, ref current_max)) = max_ver {
                                if sv.cmp_version(current_max) == Ordering::Greater {
                                    max_ver = Some((v.clone(), sv));
                                }
                            } else {
                                max_ver = Some((v.clone(), sv));
                            }
                        }
                        Err(e) => return Ok(ToolResult::error(&call.id, format!("Invalid version '{v}': {e}"))),
                    }
                }
                let Some((max_str, _)) = max_ver else {
                    return Ok(ToolResult::error(&call.id, "No valid versions found"));
                };
                let response = json!({ "max": max_str });
                Ok(ToolResult::success(&call.id, response.to_string()))
            }
            "min" => {
                let versions: Vec<String> = match call.arguments["versions"].as_array() {
                    Some(arr) => arr.iter().filter_map(|v| v.as_str().map(String::from)).collect(),
                    None => return Ok(ToolResult::error(&call.id, "Missing required parameter: 'versions'")),
                };
                if versions.is_empty() {
                    return Ok(ToolResult::error(&call.id, "Versions array is empty"));
                }
                let mut min_ver: Option<(String, Semver)> = None;
                for v in &versions {
                    match Semver::parse(v) {
                        Ok(sv) => {
                            if let Some((_, ref current_min)) = min_ver {
                                if sv.cmp_version(current_min) == Ordering::Less {
                                    min_ver = Some((v.clone(), sv));
                                }
                            } else {
                                min_ver = Some((v.clone(), sv));
                            }
                        }
                        Err(e) => return Ok(ToolResult::error(&call.id, format!("Invalid version '{v}': {e}"))),
                    }
                }
                let Some((min_str, _)) = min_ver else {
                    return Ok(ToolResult::error(&call.id, "No valid versions found"));
                };
                let response = json!({ "min": min_str });
                Ok(ToolResult::success(&call.id, response.to_string()))
            }
            _ => Ok(ToolResult::error(
                &call.id,
                format!("Unknown operation: '{operation}'. Supported: parse, validate, compare, bump, sort, satisfies, max, min"),
            )),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn make_call(args: Value) -> ToolCall {
        ToolCall {
            id: "test".to_string(),
            name: "semver_tool".to_string(),
            arguments: args,
        }
    }

    #[tokio::test]
    async fn test_parse() {
        let skill = SemverToolSkill::new();
        let call = make_call(json!({"operation": "parse", "version": "1.2.3"}));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error, "Result: {}", result.content);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["major"], 1);
        assert_eq!(parsed["minor"], 2);
        assert_eq!(parsed["patch"], 3);
    }

    #[tokio::test]
    async fn test_parse_with_prerelease() {
        let skill = SemverToolSkill::new();
        let call = make_call(json!({"operation": "parse", "version": "1.0.0-beta.1"}));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["prerelease"], "beta.1");
    }

    #[tokio::test]
    async fn test_parse_with_build() {
        let skill = SemverToolSkill::new();
        let call = make_call(json!({"operation": "parse", "version": "1.0.0+build.123"}));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["build"], "build.123");
    }

    #[tokio::test]
    async fn test_parse_v_prefix() {
        let skill = SemverToolSkill::new();
        let call = make_call(json!({"operation": "parse", "version": "v2.0.0"}));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["major"], 2);
    }

    #[tokio::test]
    async fn test_validate_valid() {
        let skill = SemverToolSkill::new();
        let call = make_call(json!({"operation": "validate", "version": "1.2.3"}));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["valid"], true);
    }

    #[tokio::test]
    async fn test_validate_invalid() {
        let skill = SemverToolSkill::new();
        let call = make_call(json!({"operation": "validate", "version": "not.a.version"}));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["valid"], false);
    }

    #[tokio::test]
    async fn test_compare_greater() {
        let skill = SemverToolSkill::new();
        let call =
            make_call(json!({"operation": "compare", "version_a": "2.0.0", "version_b": "1.0.0"}));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["result"], "greater");
        assert_eq!(parsed["a_is_newer"], true);
    }

    #[tokio::test]
    async fn test_compare_less() {
        let skill = SemverToolSkill::new();
        let call =
            make_call(json!({"operation": "compare", "version_a": "1.0.0", "version_b": "1.1.0"}));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["result"], "less");
    }

    #[tokio::test]
    async fn test_compare_equal() {
        let skill = SemverToolSkill::new();
        let call =
            make_call(json!({"operation": "compare", "version_a": "1.0.0", "version_b": "1.0.0"}));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["result"], "equal");
    }

    #[tokio::test]
    async fn test_bump_major() {
        let skill = SemverToolSkill::new();
        let call =
            make_call(json!({"operation": "bump", "version": "1.2.3", "bump_type": "major"}));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["bumped"], "2.0.0");
    }

    #[tokio::test]
    async fn test_bump_minor() {
        let skill = SemverToolSkill::new();
        let call =
            make_call(json!({"operation": "bump", "version": "1.2.3", "bump_type": "minor"}));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["bumped"], "1.3.0");
    }

    #[tokio::test]
    async fn test_bump_patch() {
        let skill = SemverToolSkill::new();
        let call =
            make_call(json!({"operation": "bump", "version": "1.2.3", "bump_type": "patch"}));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["bumped"], "1.2.4");
    }

    #[tokio::test]
    async fn test_sort() {
        let skill = SemverToolSkill::new();
        let call = make_call(json!({
            "operation": "sort",
            "versions": ["3.0.0", "1.0.0", "2.1.0", "2.0.0"]
        }));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(
            parsed["sorted"],
            json!(["1.0.0", "2.0.0", "2.1.0", "3.0.0"])
        );
    }

    #[tokio::test]
    async fn test_satisfies_caret() {
        let skill = SemverToolSkill::new();
        let call =
            make_call(json!({"operation": "satisfies", "version": "1.5.0", "range": "^1.2.0"}));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["satisfies"], true);
    }

    #[tokio::test]
    async fn test_satisfies_caret_fail() {
        let skill = SemverToolSkill::new();
        let call =
            make_call(json!({"operation": "satisfies", "version": "2.0.0", "range": "^1.2.0"}));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["satisfies"], false);
    }

    #[tokio::test]
    async fn test_satisfies_tilde() {
        let skill = SemverToolSkill::new();
        let call =
            make_call(json!({"operation": "satisfies", "version": "1.2.5", "range": "~1.2.0"}));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["satisfies"], true);
    }

    #[tokio::test]
    async fn test_satisfies_gte() {
        let skill = SemverToolSkill::new();
        let call =
            make_call(json!({"operation": "satisfies", "version": "2.0.0", "range": ">=1.0.0"}));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["satisfies"], true);
    }

    #[tokio::test]
    async fn test_max() {
        let skill = SemverToolSkill::new();
        let call = make_call(json!({"operation": "max", "versions": ["1.0.0", "3.0.0", "2.5.0"]}));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["max"], "3.0.0");
    }

    #[tokio::test]
    async fn test_min() {
        let skill = SemverToolSkill::new();
        let call = make_call(json!({"operation": "min", "versions": ["1.0.0", "3.0.0", "0.5.0"]}));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["min"], "0.5.0");
    }

    #[tokio::test]
    async fn test_missing_operation() {
        let skill = SemverToolSkill::new();
        let call = make_call(json!({"version": "1.0.0"}));
        let result = skill.execute(call).await.unwrap();
        assert!(result.is_error);
    }

    #[tokio::test]
    async fn test_unknown_operation() {
        let skill = SemverToolSkill::new();
        let call = make_call(json!({"operation": "diff"}));
        let result = skill.execute(call).await.unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("Unknown operation"));
    }

    #[test]
    fn test_descriptor_name() {
        let skill = SemverToolSkill::new();
        assert_eq!(skill.descriptor().name, "semver_tool");
    }
}
