//! Calculator skill for the Argentor AI agent framework.
//!
//! Provides precise math operations that AI agents can invoke as a tool.
//! Inspired by Vercel AI SDK Math tool, LangChain LLMMathChain,
//! Semantic Kernel MathPlugin, and AutoGPT maths block.

use argentor_core::{ArgentorResult, ToolCall, ToolResult};
use argentor_skills::skill::{Skill, SkillDescriptor};
use async_trait::async_trait;
use serde_json::json;

/// A pure-math calculator skill requiring no special capabilities.
///
/// Supports arithmetic, rounding, powers/roots, logarithms, trigonometry,
/// statistical aggregations, primality testing, and simple expression evaluation.
///
/// # Examples
///
/// ```no_run
/// use argentor_builtins::calculator::CalculatorSkill;
/// use argentor_skills::skill::Skill;
/// use argentor_core::ToolCall;
/// use serde_json::json;
///
/// let calc = CalculatorSkill::new();
/// let call = ToolCall {
///     id: "1".into(),
///     name: "calculator".into(),
///     arguments: json!({"operation": "add", "a": 2.0, "b": 3.0}),
/// };
/// // let result = calc.execute(call).await.unwrap();
/// // assert_eq!(result.content, "5");
/// ```
pub struct CalculatorSkill {
    descriptor: SkillDescriptor,
}

impl CalculatorSkill {
    /// Create a new `CalculatorSkill`.
    pub fn new() -> Self {
        Self {
            descriptor: SkillDescriptor {
                name: "calculator".to_string(),
                description: "Perform precise mathematical calculations. Supports arithmetic, \
                    powers, roots, logarithms, trigonometry, rounding, statistics, primality \
                    testing, and simple expression evaluation."
                    .to_string(),
                parameters_schema: json!({
                    "type": "object",
                    "properties": {
                        "operation": {
                            "type": "string",
                            "description": "The math operation to perform",
                            "enum": [
                                "add", "subtract", "multiply", "divide",
                                "power", "sqrt", "cbrt",
                                "abs", "ceil", "floor", "round",
                                "modulo", "factorial",
                                "log", "log10", "ln",
                                "sin", "cos", "tan",
                                "min", "max", "mean", "median",
                                "is_prime",
                                "evaluate"
                            ]
                        },
                        "a": {
                            "type": "number",
                            "description": "First operand for binary operations (add, subtract, multiply, divide, modulo)"
                        },
                        "b": {
                            "type": "number",
                            "description": "Second operand for binary operations"
                        },
                        "value": {
                            "type": "number",
                            "description": "Input value for unary operations (sqrt, cbrt, abs, ceil, floor, round, log, log10, ln, sin, cos, tan)"
                        },
                        "values": {
                            "type": "array",
                            "items": { "type": "number" },
                            "description": "Array of numbers for aggregate operations (min, max, mean, median)"
                        },
                        "base": {
                            "type": "number",
                            "description": "Base for power or logarithm operations"
                        },
                        "exponent": {
                            "type": "number",
                            "description": "Exponent for the power operation"
                        },
                        "n": {
                            "type": "integer",
                            "description": "Integer input for factorial or is_prime"
                        },
                        "expression": {
                            "type": "string",
                            "description": "A simple math expression to evaluate (supports +, -, *, /, ^, parentheses)"
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

impl Default for CalculatorSkill {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Skill for CalculatorSkill {
    fn descriptor(&self) -> &SkillDescriptor {
        &self.descriptor
    }

    async fn execute(&self, call: ToolCall) -> ArgentorResult<ToolResult> {
        let args = &call.arguments;

        let operation = match args.get("operation").and_then(|v| v.as_str()) {
            Some(op) => op,
            None => {
                return Ok(ToolResult::error(
                    &call.id,
                    "Missing required parameter: operation",
                ))
            }
        };

        let result = match operation {
            "add" => binary_op(args, &call.id, |a, b| Ok(a + b)),
            "subtract" => binary_op(args, &call.id, |a, b| Ok(a - b)),
            "multiply" => binary_op(args, &call.id, |a, b| Ok(a * b)),
            "divide" => binary_op(args, &call.id, |a, b| {
                if b == 0.0 {
                    Err("Division by zero".to_string())
                } else {
                    Ok(a / b)
                }
            }),
            "modulo" => binary_op(args, &call.id, |a, b| {
                if b == 0.0 {
                    Err("Modulo by zero".to_string())
                } else {
                    Ok(a % b)
                }
            }),
            "power" => {
                let base = match get_f64(args, "base") {
                    Some(v) => v,
                    None => {
                        return Ok(ToolResult::error(
                            &call.id,
                            "Missing required parameter: base",
                        ))
                    }
                };
                let exponent = match get_f64(args, "exponent") {
                    Some(v) => v,
                    None => {
                        return Ok(ToolResult::error(
                            &call.id,
                            "Missing required parameter: exponent",
                        ))
                    }
                };
                Ok(success_json(&call.id, base.powf(exponent)))
            }
            "sqrt" => unary_op(args, &call.id, |v| {
                if v < 0.0 {
                    Err("Cannot compute square root of a negative number".to_string())
                } else {
                    Ok(v.sqrt())
                }
            }),
            "cbrt" => unary_op(args, &call.id, |v| Ok(v.cbrt())),
            "abs" => unary_op(args, &call.id, |v| Ok(v.abs())),
            "ceil" => unary_op(args, &call.id, |v| Ok(v.ceil())),
            "floor" => unary_op(args, &call.id, |v| Ok(v.floor())),
            "round" => unary_op(args, &call.id, |v| Ok(v.round())),
            "ln" => unary_op(args, &call.id, |v| {
                if v <= 0.0 {
                    Err("Logarithm undefined for non-positive values".to_string())
                } else {
                    Ok(v.ln())
                }
            }),
            "log10" => unary_op(args, &call.id, |v| {
                if v <= 0.0 {
                    Err("Logarithm undefined for non-positive values".to_string())
                } else {
                    Ok(v.log10())
                }
            }),
            "log" => {
                let value = match get_f64(args, "value") {
                    Some(v) => v,
                    None => {
                        return Ok(ToolResult::error(
                            &call.id,
                            "Missing required parameter: value",
                        ))
                    }
                };
                if value <= 0.0 {
                    return Ok(ToolResult::error(
                        &call.id,
                        "Logarithm undefined for non-positive values",
                    ));
                }
                let base = get_f64(args, "base").unwrap_or(std::f64::consts::E);
                if base <= 0.0 || base == 1.0 {
                    return Ok(ToolResult::error(
                        &call.id,
                        "Logarithm base must be positive and not equal to 1",
                    ));
                }
                Ok(success_json(&call.id, value.log(base)))
            }
            "sin" => unary_op(args, &call.id, |v| Ok(v.sin())),
            "cos" => unary_op(args, &call.id, |v| Ok(v.cos())),
            "tan" => unary_op(args, &call.id, |v| Ok(v.tan())),
            "factorial" => {
                let n = match args.get("n").and_then(serde_json::Value::as_i64) {
                    Some(v) => v,
                    None => {
                        return Ok(ToolResult::error(
                            &call.id,
                            "Missing required integer parameter: n",
                        ))
                    }
                };
                if n < 0 {
                    return Ok(ToolResult::error(
                        &call.id,
                        "Factorial undefined for negative numbers",
                    ));
                }
                if n > 20 {
                    return Ok(ToolResult::error(
                        &call.id,
                        "Factorial overflow: maximum supported value is 20",
                    ));
                }
                let result = factorial(n as u64);
                Ok(ToolResult::success(
                    &call.id,
                    json!({ "result": result }).to_string(),
                ))
            }
            "is_prime" => {
                let n = match args.get("n").and_then(serde_json::Value::as_i64) {
                    Some(v) => v,
                    None => {
                        return Ok(ToolResult::error(
                            &call.id,
                            "Missing required integer parameter: n",
                        ))
                    }
                };
                if n < 0 {
                    return Ok(ToolResult::error(
                        &call.id,
                        "Primality test undefined for negative numbers",
                    ));
                }
                let prime = is_prime(n as u64);
                Ok(ToolResult::success(
                    &call.id,
                    json!({ "result": prime }).to_string(),
                ))
            }
            "min" => aggregate_op(args, &call.id, |vals| {
                vals.iter().copied().fold(f64::INFINITY, f64::min)
            }),
            "max" => aggregate_op(args, &call.id, |vals| {
                vals.iter().copied().fold(f64::NEG_INFINITY, f64::max)
            }),
            "mean" => aggregate_op(args, &call.id, |vals| {
                vals.iter().sum::<f64>() / vals.len() as f64
            }),
            "median" => {
                let values = match get_values(args) {
                    Some(v) => v,
                    None => {
                        return Ok(ToolResult::error(
                            &call.id,
                            "Missing required parameter: values (array of numbers)",
                        ))
                    }
                };
                if values.is_empty() {
                    return Ok(ToolResult::error(
                        &call.id,
                        "values array must not be empty",
                    ));
                }
                let mut sorted = values;
                sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
                let mid = sorted.len() / 2;
                let median = if sorted.len() % 2 == 0 {
                    (sorted[mid - 1] + sorted[mid]) / 2.0
                } else {
                    sorted[mid]
                };
                Ok(success_json(&call.id, median))
            }
            "evaluate" => {
                let expr = match args.get("expression").and_then(|v| v.as_str()) {
                    Some(e) => e,
                    None => {
                        return Ok(ToolResult::error(
                            &call.id,
                            "Missing required parameter: expression",
                        ))
                    }
                };
                match evaluate_expression(expr) {
                    Ok(val) => Ok(success_json(&call.id, val)),
                    Err(e) => Ok(ToolResult::error(&call.id, e)),
                }
            }
            other => Ok(ToolResult::error(
                &call.id,
                format!("Unknown operation: {other}"),
            )),
        };

        result
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Extract an `f64` from a JSON object by key.
fn get_f64(args: &serde_json::Value, key: &str) -> Option<f64> {
    args.get(key).and_then(serde_json::Value::as_f64)
}

/// Extract a `Vec<f64>` from the `"values"` key.
fn get_values(args: &serde_json::Value) -> Option<Vec<f64>> {
    args.get("values")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(serde_json::Value::as_f64).collect())
}

/// Build a success `ToolResult` with `{"result": value}`.
fn success_json(call_id: &str, value: f64) -> ToolResult {
    ToolResult::success(call_id, json!({ "result": value }).to_string())
}

/// Helper for binary operations that read `a` and `b`.
fn binary_op(
    args: &serde_json::Value,
    call_id: &str,
    f: impl FnOnce(f64, f64) -> Result<f64, String>,
) -> Result<ToolResult, argentor_core::ArgentorError> {
    let a = match get_f64(args, "a") {
        Some(v) => v,
        None => return Ok(ToolResult::error(call_id, "Missing required parameter: a")),
    };
    let b = match get_f64(args, "b") {
        Some(v) => v,
        None => return Ok(ToolResult::error(call_id, "Missing required parameter: b")),
    };
    match f(a, b) {
        Ok(result) => Ok(success_json(call_id, result)),
        Err(e) => Ok(ToolResult::error(call_id, e)),
    }
}

/// Helper for unary operations that read `value`.
fn unary_op(
    args: &serde_json::Value,
    call_id: &str,
    f: impl FnOnce(f64) -> Result<f64, String>,
) -> Result<ToolResult, argentor_core::ArgentorError> {
    let value = match get_f64(args, "value") {
        Some(v) => v,
        None => {
            return Ok(ToolResult::error(
                call_id,
                "Missing required parameter: value",
            ))
        }
    };
    match f(value) {
        Ok(result) => Ok(success_json(call_id, result)),
        Err(e) => Ok(ToolResult::error(call_id, e)),
    }
}

/// Helper for aggregate operations that read `values`.
fn aggregate_op(
    args: &serde_json::Value,
    call_id: &str,
    f: impl FnOnce(&[f64]) -> f64,
) -> Result<ToolResult, argentor_core::ArgentorError> {
    let values = match get_values(args) {
        Some(v) => v,
        None => {
            return Ok(ToolResult::error(
                call_id,
                "Missing required parameter: values (array of numbers)",
            ))
        }
    };
    if values.is_empty() {
        return Ok(ToolResult::error(call_id, "values array must not be empty"));
    }
    Ok(success_json(call_id, f(&values)))
}

/// Compute n! iteratively.
fn factorial(n: u64) -> u64 {
    (1..=n).product()
}

/// Deterministic primality check.
fn is_prime(n: u64) -> bool {
    if n < 2 {
        return false;
    }
    if n < 4 {
        return true;
    }
    if n % 2 == 0 || n % 3 == 0 {
        return false;
    }
    let mut i = 5u64;
    while i * i <= n {
        if n % i == 0 || n % (i + 2) == 0 {
            return false;
        }
        i += 6;
    }
    true
}

// ---------------------------------------------------------------------------
// Simple expression evaluator (recursive descent)
// ---------------------------------------------------------------------------

/// Evaluate a simple math expression string supporting `+`, `-`, `*`, `/`, `^`,
/// unary minus, and parentheses, with standard operator precedence.
fn evaluate_expression(input: &str) -> Result<f64, String> {
    let tokens = tokenize(input)?;
    let mut pos = 0;
    let result = parse_expr(&tokens, &mut pos)?;
    if pos != tokens.len() {
        return Err(format!(
            "Unexpected token at position {pos}: {:?}",
            tokens[pos]
        ));
    }
    Ok(result)
}

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Number(f64),
    Plus,
    Minus,
    Star,
    Slash,
    Caret,
    LParen,
    RParen,
}

/// Tokenize an expression string into a vector of [`Token`]s.
fn tokenize(input: &str) -> Result<Vec<Token>, String> {
    let mut tokens = Vec::new();
    let chars: Vec<char> = input.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        match chars[i] {
            ' ' | '\t' | '\n' | '\r' => {
                i += 1;
            }
            '+' => {
                tokens.push(Token::Plus);
                i += 1;
            }
            '-' => {
                tokens.push(Token::Minus);
                i += 1;
            }
            '*' => {
                tokens.push(Token::Star);
                i += 1;
            }
            '/' => {
                tokens.push(Token::Slash);
                i += 1;
            }
            '^' => {
                tokens.push(Token::Caret);
                i += 1;
            }
            '(' => {
                tokens.push(Token::LParen);
                i += 1;
            }
            ')' => {
                tokens.push(Token::RParen);
                i += 1;
            }
            c if c.is_ascii_digit() || c == '.' => {
                let start = i;
                while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.') {
                    i += 1;
                }
                let num_str: String = chars[start..i].iter().collect();
                let num = num_str
                    .parse::<f64>()
                    .map_err(|_| format!("Invalid number: {num_str}"))?;
                tokens.push(Token::Number(num));
            }
            other => {
                return Err(format!("Unexpected character: '{other}'"));
            }
        }
    }

    Ok(tokens)
}

/// Grammar (highest to lowest precedence):
///   expr     -> term (('+' | '-') term)*
///   term     -> power (('*' | '/') power)*
///   power    -> unary ('^' power)?          // right-associative
///   unary    -> '-' unary | primary
///   primary  -> NUMBER | '(' expr ')'
fn parse_expr(tokens: &[Token], pos: &mut usize) -> Result<f64, String> {
    let mut left = parse_term(tokens, pos)?;
    while *pos < tokens.len() {
        match tokens[*pos] {
            Token::Plus => {
                *pos += 1;
                left += parse_term(tokens, pos)?;
            }
            Token::Minus => {
                *pos += 1;
                left -= parse_term(tokens, pos)?;
            }
            _ => break,
        }
    }
    Ok(left)
}

fn parse_term(tokens: &[Token], pos: &mut usize) -> Result<f64, String> {
    let mut left = parse_power(tokens, pos)?;
    while *pos < tokens.len() {
        match tokens[*pos] {
            Token::Star => {
                *pos += 1;
                left *= parse_power(tokens, pos)?;
            }
            Token::Slash => {
                *pos += 1;
                let right = parse_power(tokens, pos)?;
                if right == 0.0 {
                    return Err("Division by zero in expression".to_string());
                }
                left /= right;
            }
            _ => break,
        }
    }
    Ok(left)
}

fn parse_power(tokens: &[Token], pos: &mut usize) -> Result<f64, String> {
    let base = parse_unary(tokens, pos)?;
    if *pos < tokens.len() && tokens[*pos] == Token::Caret {
        *pos += 1;
        // Right-associative: recurse into parse_power
        let exp = parse_power(tokens, pos)?;
        Ok(base.powf(exp))
    } else {
        Ok(base)
    }
}

fn parse_unary(tokens: &[Token], pos: &mut usize) -> Result<f64, String> {
    if *pos < tokens.len() && tokens[*pos] == Token::Minus {
        *pos += 1;
        let val = parse_unary(tokens, pos)?;
        Ok(-val)
    } else {
        parse_primary(tokens, pos)
    }
}

fn parse_primary(tokens: &[Token], pos: &mut usize) -> Result<f64, String> {
    if *pos >= tokens.len() {
        return Err("Unexpected end of expression".to_string());
    }
    match &tokens[*pos] {
        Token::Number(n) => {
            let v = *n;
            *pos += 1;
            Ok(v)
        }
        Token::LParen => {
            *pos += 1;
            let val = parse_expr(tokens, pos)?;
            if *pos >= tokens.len() || tokens[*pos] != Token::RParen {
                return Err("Missing closing parenthesis".to_string());
            }
            *pos += 1;
            Ok(val)
        }
        other => Err(format!("Unexpected token: {other:?}")),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use serde_json::json;

    fn call(op: &str, args: serde_json::Value) -> ToolCall {
        let mut map = args;
        map.as_object_mut()
            .unwrap()
            .insert("operation".to_string(), json!(op));
        ToolCall {
            id: "test-call".to_string(),
            name: "calculator".to_string(),
            arguments: map,
        }
    }

    async fn exec(op: &str, args: serde_json::Value) -> ToolResult {
        let skill = CalculatorSkill::new();
        skill.execute(call(op, args)).await.unwrap()
    }

    fn result_f64(tr: &ToolResult) -> f64 {
        let v: serde_json::Value = serde_json::from_str(&tr.content).unwrap();
        v["result"].as_f64().unwrap()
    }

    fn result_bool(tr: &ToolResult) -> bool {
        let v: serde_json::Value = serde_json::from_str(&tr.content).unwrap();
        v["result"].as_bool().unwrap()
    }

    fn result_u64(tr: &ToolResult) -> u64 {
        let v: serde_json::Value = serde_json::from_str(&tr.content).unwrap();
        v["result"].as_u64().unwrap()
    }

    // --- Arithmetic ---

    #[tokio::test]
    async fn test_add() {
        let r = exec("add", json!({"a": 2.5, "b": 3.5})).await;
        assert!(!r.is_error);
        assert!((result_f64(&r) - 6.0).abs() < f64::EPSILON);
    }

    #[tokio::test]
    async fn test_subtract() {
        let r = exec("subtract", json!({"a": 10.0, "b": 4.0})).await;
        assert!(!r.is_error);
        assert!((result_f64(&r) - 6.0).abs() < f64::EPSILON);
    }

    #[tokio::test]
    async fn test_multiply() {
        let r = exec("multiply", json!({"a": 3.0, "b": 7.0})).await;
        assert!(!r.is_error);
        assert!((result_f64(&r) - 21.0).abs() < f64::EPSILON);
    }

    #[tokio::test]
    async fn test_divide() {
        let r = exec("divide", json!({"a": 15.0, "b": 3.0})).await;
        assert!(!r.is_error);
        assert!((result_f64(&r) - 5.0).abs() < f64::EPSILON);
    }

    #[tokio::test]
    async fn test_divide_by_zero() {
        let r = exec("divide", json!({"a": 1.0, "b": 0.0})).await;
        assert!(r.is_error);
        assert!(r.content.contains("Division by zero"));
    }

    #[tokio::test]
    async fn test_modulo() {
        let r = exec("modulo", json!({"a": 17.0, "b": 5.0})).await;
        assert!(!r.is_error);
        assert!((result_f64(&r) - 2.0).abs() < f64::EPSILON);
    }

    #[tokio::test]
    async fn test_modulo_by_zero() {
        let r = exec("modulo", json!({"a": 10.0, "b": 0.0})).await;
        assert!(r.is_error);
        assert!(r.content.contains("Modulo by zero"));
    }

    // --- Power / roots ---

    #[tokio::test]
    async fn test_power() {
        let r = exec("power", json!({"base": 2.0, "exponent": 10.0})).await;
        assert!(!r.is_error);
        assert!((result_f64(&r) - 1024.0).abs() < f64::EPSILON);
    }

    #[tokio::test]
    async fn test_sqrt() {
        let r = exec("sqrt", json!({"value": 144.0})).await;
        assert!(!r.is_error);
        assert!((result_f64(&r) - 12.0).abs() < f64::EPSILON);
    }

    #[tokio::test]
    async fn test_sqrt_negative() {
        let r = exec("sqrt", json!({"value": -4.0})).await;
        assert!(r.is_error);
        assert!(r.content.contains("negative"));
    }

    #[tokio::test]
    async fn test_cbrt() {
        let r = exec("cbrt", json!({"value": 27.0})).await;
        assert!(!r.is_error);
        assert!((result_f64(&r) - 3.0).abs() < f64::EPSILON);
    }

    #[tokio::test]
    async fn test_cbrt_negative() {
        let r = exec("cbrt", json!({"value": -8.0})).await;
        assert!(!r.is_error);
        assert!((result_f64(&r) - (-2.0)).abs() < f64::EPSILON);
    }

    // --- Rounding / abs ---

    #[tokio::test]
    async fn test_abs() {
        let r = exec("abs", json!({"value": -42.5})).await;
        assert!(!r.is_error);
        assert!((result_f64(&r) - 42.5).abs() < f64::EPSILON);
    }

    #[tokio::test]
    async fn test_ceil() {
        let r = exec("ceil", json!({"value": 2.3})).await;
        assert!(!r.is_error);
        assert!((result_f64(&r) - 3.0).abs() < f64::EPSILON);
    }

    #[tokio::test]
    async fn test_floor() {
        let r = exec("floor", json!({"value": 2.9})).await;
        assert!(!r.is_error);
        assert!((result_f64(&r) - 2.0).abs() < f64::EPSILON);
    }

    #[tokio::test]
    async fn test_round() {
        let r = exec("round", json!({"value": 2.5})).await;
        assert!(!r.is_error);
        assert!((result_f64(&r) - 3.0).abs() < f64::EPSILON);
    }

    // --- Logarithms ---

    #[tokio::test]
    async fn test_ln() {
        let r = exec("ln", json!({"value": std::f64::consts::E})).await;
        assert!(!r.is_error);
        assert!((result_f64(&r) - 1.0).abs() < 1e-10);
    }

    #[tokio::test]
    async fn test_log10() {
        let r = exec("log10", json!({"value": 1000.0})).await;
        assert!(!r.is_error);
        assert!((result_f64(&r) - 3.0).abs() < 1e-10);
    }

    #[tokio::test]
    async fn test_log_custom_base() {
        let r = exec("log", json!({"value": 8.0, "base": 2.0})).await;
        assert!(!r.is_error);
        assert!((result_f64(&r) - 3.0).abs() < 1e-10);
    }

    #[tokio::test]
    async fn test_log_default_base_e() {
        let r = exec("log", json!({"value": std::f64::consts::E})).await;
        assert!(!r.is_error);
        assert!((result_f64(&r) - 1.0).abs() < 1e-10);
    }

    #[tokio::test]
    async fn test_log_nonpositive() {
        let r = exec("ln", json!({"value": 0.0})).await;
        assert!(r.is_error);
        assert!(r.content.contains("non-positive"));
    }

    #[tokio::test]
    async fn test_log_base_one() {
        let r = exec("log", json!({"value": 10.0, "base": 1.0})).await;
        assert!(r.is_error);
        assert!(r.content.contains("not equal to 1"));
    }

    // --- Trigonometry ---

    #[tokio::test]
    async fn test_sin() {
        let r = exec("sin", json!({"value": 0.0})).await;
        assert!(!r.is_error);
        assert!(result_f64(&r).abs() < 1e-10);
    }

    #[tokio::test]
    async fn test_cos() {
        let r = exec("cos", json!({"value": 0.0})).await;
        assert!(!r.is_error);
        assert!((result_f64(&r) - 1.0).abs() < 1e-10);
    }

    #[tokio::test]
    async fn test_tan() {
        let r = exec("tan", json!({"value": 0.0})).await;
        assert!(!r.is_error);
        assert!(result_f64(&r).abs() < 1e-10);
    }

    #[tokio::test]
    async fn test_sin_pi_half() {
        let r = exec("sin", json!({"value": std::f64::consts::FRAC_PI_2})).await;
        assert!(!r.is_error);
        assert!((result_f64(&r) - 1.0).abs() < 1e-10);
    }

    // --- Factorial ---

    #[tokio::test]
    async fn test_factorial_zero() {
        let r = exec("factorial", json!({"n": 0})).await;
        assert!(!r.is_error);
        assert_eq!(result_u64(&r), 1);
    }

    #[tokio::test]
    async fn test_factorial_five() {
        let r = exec("factorial", json!({"n": 5})).await;
        assert!(!r.is_error);
        assert_eq!(result_u64(&r), 120);
    }

    #[tokio::test]
    async fn test_factorial_twenty() {
        let r = exec("factorial", json!({"n": 20})).await;
        assert!(!r.is_error);
        assert_eq!(result_u64(&r), 2_432_902_008_176_640_000);
    }

    #[tokio::test]
    async fn test_factorial_overflow() {
        let r = exec("factorial", json!({"n": 21})).await;
        assert!(r.is_error);
        assert!(r.content.contains("overflow"));
    }

    #[tokio::test]
    async fn test_factorial_negative() {
        let r = exec("factorial", json!({"n": -1})).await;
        assert!(r.is_error);
        assert!(r.content.contains("negative"));
    }

    // --- Primality ---

    #[tokio::test]
    async fn test_is_prime_small_primes() {
        for p in [2, 3, 5, 7, 11, 13, 17, 19, 23, 29, 31] {
            let r = exec("is_prime", json!({"n": p})).await;
            assert!(!r.is_error);
            assert!(result_bool(&r), "{p} should be prime");
        }
    }

    #[tokio::test]
    async fn test_is_prime_composites() {
        for n in [0, 1, 4, 6, 8, 9, 10, 12, 15, 100] {
            let r = exec("is_prime", json!({"n": n})).await;
            assert!(!r.is_error);
            assert!(!result_bool(&r), "{n} should not be prime");
        }
    }

    #[tokio::test]
    async fn test_is_prime_negative() {
        let r = exec("is_prime", json!({"n": -7})).await;
        assert!(r.is_error);
        assert!(r.content.contains("negative"));
    }

    // --- Aggregates ---

    #[tokio::test]
    async fn test_min() {
        let r = exec("min", json!({"values": [5.0, 2.0, 8.0, 1.0, 9.0]})).await;
        assert!(!r.is_error);
        assert!((result_f64(&r) - 1.0).abs() < f64::EPSILON);
    }

    #[tokio::test]
    async fn test_max() {
        let r = exec("max", json!({"values": [5.0, 2.0, 8.0, 1.0, 9.0]})).await;
        assert!(!r.is_error);
        assert!((result_f64(&r) - 9.0).abs() < f64::EPSILON);
    }

    #[tokio::test]
    async fn test_mean() {
        let r = exec("mean", json!({"values": [2.0, 4.0, 6.0, 8.0, 10.0]})).await;
        assert!(!r.is_error);
        assert!((result_f64(&r) - 6.0).abs() < f64::EPSILON);
    }

    #[tokio::test]
    async fn test_median_odd() {
        let r = exec("median", json!({"values": [3.0, 1.0, 2.0]})).await;
        assert!(!r.is_error);
        assert!((result_f64(&r) - 2.0).abs() < f64::EPSILON);
    }

    #[tokio::test]
    async fn test_median_even() {
        let r = exec("median", json!({"values": [4.0, 1.0, 3.0, 2.0]})).await;
        assert!(!r.is_error);
        assert!((result_f64(&r) - 2.5).abs() < f64::EPSILON);
    }

    #[tokio::test]
    async fn test_aggregate_empty() {
        let r = exec("min", json!({"values": []})).await;
        assert!(r.is_error);
        assert!(r.content.contains("empty"));
    }

    #[tokio::test]
    async fn test_aggregate_missing_values() {
        let r = exec("min", json!({})).await;
        assert!(r.is_error);
        assert!(r.content.contains("values"));
    }

    // --- Expression evaluator ---

    #[tokio::test]
    async fn test_eval_simple_add() {
        let r = exec("evaluate", json!({"expression": "2 + 3"})).await;
        assert!(!r.is_error);
        assert!((result_f64(&r) - 5.0).abs() < f64::EPSILON);
    }

    #[tokio::test]
    async fn test_eval_precedence() {
        let r = exec("evaluate", json!({"expression": "2 + 3 * 4"})).await;
        assert!(!r.is_error);
        assert!((result_f64(&r) - 14.0).abs() < f64::EPSILON);
    }

    #[tokio::test]
    async fn test_eval_parentheses() {
        let r = exec("evaluate", json!({"expression": "(2 + 3) * 4"})).await;
        assert!(!r.is_error);
        assert!((result_f64(&r) - 20.0).abs() < f64::EPSILON);
    }

    #[tokio::test]
    async fn test_eval_power() {
        let r = exec("evaluate", json!({"expression": "2 ^ 10"})).await;
        assert!(!r.is_error);
        assert!((result_f64(&r) - 1024.0).abs() < f64::EPSILON);
    }

    #[tokio::test]
    async fn test_eval_power_right_assoc() {
        // 2^3^2 should be 2^(3^2) = 2^9 = 512
        let r = exec("evaluate", json!({"expression": "2^3^2"})).await;
        assert!(!r.is_error);
        assert!((result_f64(&r) - 512.0).abs() < f64::EPSILON);
    }

    #[tokio::test]
    async fn test_eval_unary_minus() {
        let r = exec("evaluate", json!({"expression": "-3 + 5"})).await;
        assert!(!r.is_error);
        assert!((result_f64(&r) - 2.0).abs() < f64::EPSILON);
    }

    #[tokio::test]
    async fn test_eval_nested_parens() {
        let r = exec("evaluate", json!({"expression": "((1 + 2) * (3 + 4))"})).await;
        assert!(!r.is_error);
        assert!((result_f64(&r) - 21.0).abs() < f64::EPSILON);
    }

    #[tokio::test]
    async fn test_eval_division_by_zero() {
        let r = exec("evaluate", json!({"expression": "1 / 0"})).await;
        assert!(r.is_error);
        assert!(r.content.contains("Division by zero"));
    }

    #[tokio::test]
    async fn test_eval_decimal() {
        let r = exec("evaluate", json!({"expression": "0.1 + 0.2"})).await;
        assert!(!r.is_error);
        assert!((result_f64(&r) - 0.3).abs() < 1e-10);
    }

    #[tokio::test]
    async fn test_eval_complex() {
        // (10 - 2) * 3 + 4 / 2 = 24 + 2 = 26
        let r = exec("evaluate", json!({"expression": "(10 - 2) * 3 + 4 / 2"})).await;
        assert!(!r.is_error);
        assert!((result_f64(&r) - 26.0).abs() < f64::EPSILON);
    }

    #[tokio::test]
    async fn test_eval_missing_paren() {
        let r = exec("evaluate", json!({"expression": "(1 + 2"})).await;
        assert!(r.is_error);
        assert!(r.content.contains("parenthesis"));
    }

    #[tokio::test]
    async fn test_eval_invalid_char() {
        let r = exec("evaluate", json!({"expression": "2 & 3"})).await;
        assert!(r.is_error);
    }

    #[tokio::test]
    async fn test_eval_empty() {
        let r = exec("evaluate", json!({"expression": ""})).await;
        assert!(r.is_error);
    }

    // --- Missing parameters ---

    #[tokio::test]
    async fn test_missing_operation() {
        let skill = CalculatorSkill::new();
        let tc = ToolCall {
            id: "t".to_string(),
            name: "calculator".to_string(),
            arguments: json!({}),
        };
        let r = skill.execute(tc).await.unwrap();
        assert!(r.is_error);
        assert!(r.content.contains("operation"));
    }

    #[tokio::test]
    async fn test_unknown_operation() {
        let r = exec("foobar", json!({})).await;
        assert!(r.is_error);
        assert!(r.content.contains("Unknown operation"));
    }

    #[tokio::test]
    async fn test_missing_a() {
        let r = exec("add", json!({"b": 1.0})).await;
        assert!(r.is_error);
        assert!(r.content.contains("a"));
    }

    #[tokio::test]
    async fn test_missing_b() {
        let r = exec("add", json!({"a": 1.0})).await;
        assert!(r.is_error);
        assert!(r.content.contains("b"));
    }

    #[tokio::test]
    async fn test_missing_value() {
        let r = exec("sqrt", json!({})).await;
        assert!(r.is_error);
        assert!(r.content.contains("value"));
    }

    #[tokio::test]
    async fn test_missing_n_factorial() {
        let r = exec("factorial", json!({})).await;
        assert!(r.is_error);
        assert!(r.content.contains("n"));
    }

    #[tokio::test]
    async fn test_missing_expression() {
        let r = exec("evaluate", json!({})).await;
        assert!(r.is_error);
        assert!(r.content.contains("expression"));
    }

    // --- Descriptor ---

    #[tokio::test]
    async fn test_descriptor() {
        let skill = CalculatorSkill::new();
        let desc = skill.descriptor();
        assert_eq!(desc.name, "calculator");
        assert!(desc.required_capabilities.is_empty());
        assert!(desc.parameters_schema["properties"]["operation"].is_object());
    }

    // --- Default trait ---

    #[test]
    fn test_default() {
        let skill = CalculatorSkill::default();
        assert_eq!(skill.descriptor().name, "calculator");
    }

    // --- Pure function unit tests ---

    #[test]
    fn test_factorial_fn() {
        assert_eq!(factorial(0), 1);
        assert_eq!(factorial(1), 1);
        assert_eq!(factorial(10), 3_628_800);
        assert_eq!(factorial(20), 2_432_902_008_176_640_000);
    }

    #[test]
    fn test_is_prime_fn() {
        assert!(!is_prime(0));
        assert!(!is_prime(1));
        assert!(is_prime(2));
        assert!(is_prime(3));
        assert!(!is_prime(4));
        assert!(is_prime(97));
        assert!(!is_prime(99));
        assert!(is_prime(7919));
    }

    #[test]
    fn test_tokenize() {
        let tokens = tokenize("2 + 3.5 * (4 - 1)").unwrap();
        assert_eq!(tokens.len(), 9);
        assert_eq!(tokens[0], Token::Number(2.0));
        assert_eq!(tokens[1], Token::Plus);
        assert_eq!(tokens[2], Token::Number(3.5));
    }

    #[test]
    fn test_evaluate_expression_fn() {
        assert!((evaluate_expression("1+1").unwrap() - 2.0).abs() < f64::EPSILON);
        assert!((evaluate_expression("2*3+4").unwrap() - 10.0).abs() < f64::EPSILON);
        assert!((evaluate_expression("2*(3+4)").unwrap() - 14.0).abs() < f64::EPSILON);
        assert!(evaluate_expression("").is_err());
        assert!(evaluate_expression("1/0").is_err());
    }
}
