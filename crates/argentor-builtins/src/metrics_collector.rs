//! Metrics collection skill for the Argentor AI agent framework.
//!
//! Provides in-memory counter/gauge/histogram collection with
//! Prometheus and JSON export formats.

use argentor_core::{ArgentorResult, ToolCall, ToolResult};
use argentor_skills::skill::{Skill, SkillDescriptor};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Mutex;

/// In-memory metrics collection skill.
pub struct MetricsCollectorSkill {
    descriptor: SkillDescriptor,
    counters: Mutex<HashMap<String, f64>>,
    gauges: Mutex<HashMap<String, f64>>,
    histograms: Mutex<HashMap<String, Vec<f64>>>,
}

impl MetricsCollectorSkill {
    /// Create a new metrics collector skill.
    pub fn new() -> Self {
        Self {
            descriptor: SkillDescriptor {
                name: "metrics_collector".to_string(),
                description:
                    "In-memory counter/gauge/histogram collection, format as Prometheus/JSON."
                        .to_string(),
                parameters_schema: json!({
                    "type": "object",
                    "properties": {
                        "operation": {
                            "type": "string",
                            "enum": ["counter_inc", "counter_get", "gauge_set", "gauge_inc", "gauge_dec", "gauge_get", "histogram_observe", "histogram_get", "export_json", "export_prometheus", "reset", "list"],
                            "description": "The metrics operation to perform"
                        },
                        "name": {
                            "type": "string",
                            "description": "Metric name"
                        },
                        "value": {
                            "type": "number",
                            "description": "Value to set/increment/observe"
                        }
                    },
                    "required": ["operation"]
                }),
                required_capabilities: vec![],
                requires_approval: false,
            },
            counters: Mutex::new(HashMap::new()),
            gauges: Mutex::new(HashMap::new()),
            histograms: Mutex::new(HashMap::new()),
        }
    }
}

impl Default for MetricsCollectorSkill {
    fn default() -> Self {
        Self::new()
    }
}

/// Compute histogram statistics.
fn histogram_stats(values: &[f64]) -> Value {
    if values.is_empty() {
        return json!({
            "count": 0,
            "sum": 0.0,
            "min": null,
            "max": null,
            "mean": null,
            "p50": null,
            "p90": null,
            "p99": null
        });
    }

    let count = values.len();
    let sum: f64 = values.iter().sum();
    let mean = sum / count as f64;
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let min = sorted[0];
    let max = sorted[count - 1];

    let percentile = |p: f64| -> f64 {
        let idx = (p / 100.0 * (count - 1) as f64).round() as usize;
        sorted[idx.min(count - 1)]
    };

    json!({
        "count": count,
        "sum": sum,
        "min": min,
        "max": max,
        "mean": mean,
        "p50": percentile(50.0),
        "p90": percentile(90.0),
        "p99": percentile(99.0)
    })
}

#[async_trait]
impl Skill for MetricsCollectorSkill {
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
            "counter_inc" => {
                let name = match call.arguments["name"].as_str() {
                    Some(v) => v.to_string(),
                    None => return Ok(ToolResult::error(&call.id, "Missing required parameter: 'name'")),
                };
                let value = call.arguments["value"].as_f64().unwrap_or(1.0);
                let mut counters = self.counters.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                let entry = counters.entry(name.clone()).or_insert(0.0);
                *entry += value;
                let current = *entry;
                let response = json!({ "name": name, "value": current, "type": "counter" });
                Ok(ToolResult::success(&call.id, response.to_string()))
            }
            "counter_get" => {
                let name = match call.arguments["name"].as_str() {
                    Some(v) => v,
                    None => return Ok(ToolResult::error(&call.id, "Missing required parameter: 'name'")),
                };
                let counters = self.counters.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                let value = counters.get(name).copied().unwrap_or(0.0);
                let response = json!({ "name": name, "value": value, "type": "counter" });
                Ok(ToolResult::success(&call.id, response.to_string()))
            }
            "gauge_set" => {
                let name = match call.arguments["name"].as_str() {
                    Some(v) => v.to_string(),
                    None => return Ok(ToolResult::error(&call.id, "Missing required parameter: 'name'")),
                };
                let value = call.arguments["value"].as_f64().unwrap_or(0.0);
                let mut gauges = self.gauges.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                gauges.insert(name.clone(), value);
                let response = json!({ "name": name, "value": value, "type": "gauge" });
                Ok(ToolResult::success(&call.id, response.to_string()))
            }
            "gauge_inc" => {
                let name = match call.arguments["name"].as_str() {
                    Some(v) => v.to_string(),
                    None => return Ok(ToolResult::error(&call.id, "Missing required parameter: 'name'")),
                };
                let value = call.arguments["value"].as_f64().unwrap_or(1.0);
                let mut gauges = self.gauges.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                let entry = gauges.entry(name.clone()).or_insert(0.0);
                *entry += value;
                let current = *entry;
                let response = json!({ "name": name, "value": current, "type": "gauge" });
                Ok(ToolResult::success(&call.id, response.to_string()))
            }
            "gauge_dec" => {
                let name = match call.arguments["name"].as_str() {
                    Some(v) => v.to_string(),
                    None => return Ok(ToolResult::error(&call.id, "Missing required parameter: 'name'")),
                };
                let value = call.arguments["value"].as_f64().unwrap_or(1.0);
                let mut gauges = self.gauges.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                let entry = gauges.entry(name.clone()).or_insert(0.0);
                *entry -= value;
                let current = *entry;
                let response = json!({ "name": name, "value": current, "type": "gauge" });
                Ok(ToolResult::success(&call.id, response.to_string()))
            }
            "gauge_get" => {
                let name = match call.arguments["name"].as_str() {
                    Some(v) => v,
                    None => return Ok(ToolResult::error(&call.id, "Missing required parameter: 'name'")),
                };
                let gauges = self.gauges.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                let value = gauges.get(name).copied().unwrap_or(0.0);
                let response = json!({ "name": name, "value": value, "type": "gauge" });
                Ok(ToolResult::success(&call.id, response.to_string()))
            }
            "histogram_observe" => {
                let name = match call.arguments["name"].as_str() {
                    Some(v) => v.to_string(),
                    None => return Ok(ToolResult::error(&call.id, "Missing required parameter: 'name'")),
                };
                let value = match call.arguments["value"].as_f64() {
                    Some(v) => v,
                    None => return Ok(ToolResult::error(&call.id, "Missing required parameter: 'value'")),
                };
                let mut histograms = self.histograms.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                histograms.entry(name.clone()).or_default().push(value);
                let count = histograms[&name].len();
                let response = json!({ "name": name, "observed": value, "total_observations": count });
                Ok(ToolResult::success(&call.id, response.to_string()))
            }
            "histogram_get" => {
                let name = match call.arguments["name"].as_str() {
                    Some(v) => v,
                    None => return Ok(ToolResult::error(&call.id, "Missing required parameter: 'name'")),
                };
                let histograms = self.histograms.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                let values = histograms.get(name).cloned().unwrap_or_default();
                let stats = histogram_stats(&values);
                let response = json!({ "name": name, "stats": stats, "type": "histogram" });
                Ok(ToolResult::success(&call.id, response.to_string()))
            }
            "export_json" => {
                let counters = self.counters.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                let gauges = self.gauges.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                let histograms = self.histograms.lock().unwrap_or_else(std::sync::PoisonError::into_inner);

                let mut hist_export = serde_json::Map::new();
                for (name, values) in histograms.iter() {
                    hist_export.insert(name.clone(), histogram_stats(values));
                }

                let response = json!({
                    "counters": *counters,
                    "gauges": *gauges,
                    "histograms": hist_export,
                    "total_metrics": counters.len() + gauges.len() + histograms.len()
                });
                Ok(ToolResult::success(&call.id, response.to_string()))
            }
            "export_prometheus" => {
                let counters = self.counters.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                let gauges = self.gauges.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                let histograms = self.histograms.lock().unwrap_or_else(std::sync::PoisonError::into_inner);

                let mut output = String::new();
                for (name, value) in counters.iter() {
                    output.push_str(&format!("# TYPE {name} counter\n"));
                    output.push_str(&format!("{name} {value}\n"));
                }
                for (name, value) in gauges.iter() {
                    output.push_str(&format!("# TYPE {name} gauge\n"));
                    output.push_str(&format!("{name} {value}\n"));
                }
                for (name, values) in histograms.iter() {
                    let stats = histogram_stats(values);
                    output.push_str(&format!("# TYPE {name} histogram\n"));
                    output.push_str(&format!("{name}_count {}\n", stats["count"]));
                    output.push_str(&format!("{name}_sum {}\n", stats["sum"]));
                }

                let response = json!({ "prometheus": output.trim_end() });
                Ok(ToolResult::success(&call.id, response.to_string()))
            }
            "reset" => {
                let mut counters = self.counters.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                let mut gauges = self.gauges.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                let mut histograms = self.histograms.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                counters.clear();
                gauges.clear();
                histograms.clear();
                let response = json!({ "reset": true });
                Ok(ToolResult::success(&call.id, response.to_string()))
            }
            "list" => {
                let counters = self.counters.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                let gauges = self.gauges.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                let histograms = self.histograms.lock().unwrap_or_else(std::sync::PoisonError::into_inner);

                let counter_names: Vec<&String> = counters.keys().collect();
                let gauge_names: Vec<&String> = gauges.keys().collect();
                let histogram_names: Vec<&String> = histograms.keys().collect();

                let response = json!({
                    "counters": counter_names,
                    "gauges": gauge_names,
                    "histograms": histogram_names,
                    "total": counters.len() + gauges.len() + histograms.len()
                });
                Ok(ToolResult::success(&call.id, response.to_string()))
            }
            _ => Ok(ToolResult::error(
                &call.id,
                format!("Unknown operation: '{operation}'. Supported: counter_inc, counter_get, gauge_set, gauge_inc, gauge_dec, gauge_get, histogram_observe, histogram_get, export_json, export_prometheus, reset, list"),
            )),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn make_call(args: Value) -> ToolCall {
        ToolCall {
            id: "test".to_string(),
            name: "metrics_collector".to_string(),
            arguments: args,
        }
    }

    #[tokio::test]
    async fn test_counter_inc() {
        let skill = MetricsCollectorSkill::new();
        let call = make_call(json!({"operation": "counter_inc", "name": "requests"}));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error, "Result: {}", result.content);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["value"], 1.0);
    }

    #[tokio::test]
    async fn test_counter_inc_multiple() {
        let skill = MetricsCollectorSkill::new();
        skill
            .execute(make_call(
                json!({"operation": "counter_inc", "name": "hits"}),
            ))
            .await
            .unwrap();
        skill
            .execute(make_call(
                json!({"operation": "counter_inc", "name": "hits"}),
            ))
            .await
            .unwrap();
        let result = skill
            .execute(make_call(
                json!({"operation": "counter_inc", "name": "hits"}),
            ))
            .await
            .unwrap();
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["value"], 3.0);
    }

    #[tokio::test]
    async fn test_counter_inc_custom_value() {
        let skill = MetricsCollectorSkill::new();
        let result = skill
            .execute(make_call(
                json!({"operation": "counter_inc", "name": "bytes", "value": 1024}),
            ))
            .await
            .unwrap();
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["value"], 1024.0);
    }

    #[tokio::test]
    async fn test_counter_get() {
        let skill = MetricsCollectorSkill::new();
        skill
            .execute(make_call(
                json!({"operation": "counter_inc", "name": "x", "value": 5}),
            ))
            .await
            .unwrap();
        let result = skill
            .execute(make_call(json!({"operation": "counter_get", "name": "x"})))
            .await
            .unwrap();
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["value"], 5.0);
    }

    #[tokio::test]
    async fn test_counter_get_nonexistent() {
        let skill = MetricsCollectorSkill::new();
        let result = skill
            .execute(make_call(
                json!({"operation": "counter_get", "name": "nope"}),
            ))
            .await
            .unwrap();
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["value"], 0.0);
    }

    #[tokio::test]
    async fn test_gauge_set_and_get() {
        let skill = MetricsCollectorSkill::new();
        skill
            .execute(make_call(
                json!({"operation": "gauge_set", "name": "temp", "value": 42.5}),
            ))
            .await
            .unwrap();
        let result = skill
            .execute(make_call(json!({"operation": "gauge_get", "name": "temp"})))
            .await
            .unwrap();
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["value"], 42.5);
    }

    #[tokio::test]
    async fn test_gauge_inc_dec() {
        let skill = MetricsCollectorSkill::new();
        skill
            .execute(make_call(
                json!({"operation": "gauge_set", "name": "conn", "value": 10}),
            ))
            .await
            .unwrap();
        skill
            .execute(make_call(
                json!({"operation": "gauge_inc", "name": "conn", "value": 5}),
            ))
            .await
            .unwrap();
        let result = skill
            .execute(make_call(
                json!({"operation": "gauge_dec", "name": "conn", "value": 3}),
            ))
            .await
            .unwrap();
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["value"], 12.0);
    }

    #[tokio::test]
    async fn test_histogram_observe_and_get() {
        let skill = MetricsCollectorSkill::new();
        for v in &[0.1, 0.2, 0.3, 0.15, 0.5] {
            skill
                .execute(make_call(
                    json!({"operation": "histogram_observe", "name": "latency", "value": v}),
                ))
                .await
                .unwrap();
        }
        let result = skill
            .execute(make_call(
                json!({"operation": "histogram_get", "name": "latency"}),
            ))
            .await
            .unwrap();
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["stats"]["count"], 5);
        assert_eq!(parsed["stats"]["min"], 0.1);
        assert_eq!(parsed["stats"]["max"], 0.5);
    }

    #[tokio::test]
    async fn test_export_json() {
        let skill = MetricsCollectorSkill::new();
        skill
            .execute(make_call(
                json!({"operation": "counter_inc", "name": "reqs"}),
            ))
            .await
            .unwrap();
        skill
            .execute(make_call(
                json!({"operation": "gauge_set", "name": "mem", "value": 256}),
            ))
            .await
            .unwrap();
        let result = skill
            .execute(make_call(json!({"operation": "export_json"})))
            .await
            .unwrap();
        assert!(!result.is_error);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["total_metrics"], 2);
    }

    #[tokio::test]
    async fn test_export_prometheus() {
        let skill = MetricsCollectorSkill::new();
        skill
            .execute(make_call(
                json!({"operation": "counter_inc", "name": "http_requests_total", "value": 100}),
            ))
            .await
            .unwrap();
        let result = skill
            .execute(make_call(json!({"operation": "export_prometheus"})))
            .await
            .unwrap();
        assert!(!result.is_error);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        let prom = parsed["prometheus"].as_str().unwrap();
        assert!(prom.contains("# TYPE http_requests_total counter"));
        assert!(prom.contains("http_requests_total 100"));
    }

    #[tokio::test]
    async fn test_reset() {
        let skill = MetricsCollectorSkill::new();
        skill
            .execute(make_call(json!({"operation": "counter_inc", "name": "x"})))
            .await
            .unwrap();
        skill
            .execute(make_call(json!({"operation": "reset"})))
            .await
            .unwrap();
        let result = skill
            .execute(make_call(json!({"operation": "counter_get", "name": "x"})))
            .await
            .unwrap();
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["value"], 0.0);
    }

    #[tokio::test]
    async fn test_list() {
        let skill = MetricsCollectorSkill::new();
        skill
            .execute(make_call(json!({"operation": "counter_inc", "name": "c1"})))
            .await
            .unwrap();
        skill
            .execute(make_call(
                json!({"operation": "gauge_set", "name": "g1", "value": 1}),
            ))
            .await
            .unwrap();
        let result = skill
            .execute(make_call(json!({"operation": "list"})))
            .await
            .unwrap();
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["total"], 2);
    }

    #[tokio::test]
    async fn test_missing_operation() {
        let skill = MetricsCollectorSkill::new();
        let call = make_call(json!({"name": "x"}));
        let result = skill.execute(call).await.unwrap();
        assert!(result.is_error);
    }

    #[tokio::test]
    async fn test_unknown_operation() {
        let skill = MetricsCollectorSkill::new();
        let call = make_call(json!({"operation": "summary"}));
        let result = skill.execute(call).await.unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("Unknown operation"));
    }

    #[test]
    fn test_descriptor_name() {
        let skill = MetricsCollectorSkill::new();
        assert_eq!(skill.descriptor().name, "metrics_collector");
    }
}
