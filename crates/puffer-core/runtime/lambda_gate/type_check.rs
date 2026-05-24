use serde_json::{Map, Value};

/// Returns whether one JSON formal argument satisfies a Lambda Skill type.
pub(super) fn lambda_arg_matches_type(
    value: &Value,
    param_name: &str,
    all_args: &Map<String, Value>,
    ty: &str,
) -> bool {
    let (base, refinement) = split_refinement(ty);
    if !base_matches(value, base, param_name, all_args) {
        return false;
    }
    refinement
        .map(|expr| refinement_matches(value, param_name, all_args, expr))
        .unwrap_or(true)
}

fn split_refinement(ty: &str) -> (&str, Option<&str>) {
    let trimmed = ty.trim();
    let Some((base, tail)) = trimmed.split_once('{') else {
        return (trimmed, None);
    };
    let refinement = tail.strip_suffix('}').unwrap_or(tail).trim();
    (base.trim(), Some(refinement))
}

fn base_matches(
    value: &Value,
    base: &str,
    param_name: &str,
    all_args: &Map<String, Value>,
) -> bool {
    let lowered = base.trim().to_ascii_lowercase();
    if let Some(inner) = lowered.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
        return value.as_array().is_some_and(|items| {
            items
                .iter()
                .all(|item| lambda_arg_matches_type(item, param_name, all_args, inner))
        });
    }
    match lowered.as_str() {
        "str" | "string" => value.is_string(),
        "int" => json_integer(value).is_some(),
        "nat" => json_integer(value).is_some_and(|number| number >= 0),
        "real" | "float" | "number" => value.as_f64().is_some(),
        "bool" => value.is_boolean(),
        "unit" => value.is_null() || value.as_object().is_some_and(Map::is_empty),
        _ => true,
    }
}

fn refinement_matches(
    value: &Value,
    param_name: &str,
    all_args: &Map<String, Value>,
    expr: &str,
) -> bool {
    let expr = strip_outer_parens(expr.trim());
    if expr.is_empty() {
        return true;
    }
    let and_parts = split_top_level(expr, "&&");
    if and_parts.len() > 1 {
        return and_parts
            .iter()
            .all(|part| refinement_matches(value, param_name, all_args, part));
    }
    let or_parts = split_top_level(expr, "||");
    if or_parts.len() > 1 {
        return or_parts
            .iter()
            .any(|part| refinement_matches(value, param_name, all_args, part));
    }
    if let Some(result) = compare_expr(value, param_name, all_args, expr) {
        return result;
    }
    if let Some(result) = string_predicate(value, expr) {
        return result;
    }
    false
}

fn strip_outer_parens(mut expr: &str) -> &str {
    loop {
        let trimmed = expr.trim();
        if !(trimmed.starts_with('(') && trimmed.ends_with(')')) {
            return trimmed;
        }
        let inner = &trimmed[1..trimmed.len() - 1];
        if !balanced_parens(inner) {
            return trimmed;
        }
        expr = inner;
    }
}

fn split_top_level<'a>(expr: &'a str, op: &str) -> Vec<&'a str> {
    let mut parts = Vec::new();
    let mut depth = 0usize;
    let mut start = 0usize;
    let mut index = 0usize;
    while index < expr.len() {
        let rest = &expr[index..];
        if rest.starts_with('(') {
            depth += 1;
            index += 1;
            continue;
        }
        if rest.starts_with(')') {
            depth = depth.saturating_sub(1);
            index += 1;
            continue;
        }
        if depth == 0 && rest.starts_with(op) {
            parts.push(expr[start..index].trim());
            index += op.len();
            start = index;
            continue;
        }
        index += rest.chars().next().map(char::len_utf8).unwrap_or(1);
    }
    if parts.is_empty() {
        return vec![expr.trim()];
    }
    parts.push(expr[start..].trim());
    parts
}

fn balanced_parens(expr: &str) -> bool {
    let mut depth = 0usize;
    for ch in expr.chars() {
        match ch {
            '(' => depth += 1,
            ')' => {
                let Some(next) = depth.checked_sub(1) else {
                    return false;
                };
                depth = next;
            }
            _ => {}
        }
    }
    depth == 0
}

fn compare_expr(
    value: &Value,
    param_name: &str,
    all_args: &Map<String, Value>,
    expr: &str,
) -> Option<bool> {
    for op in [">=", "<=", "!=", "==", "=", ">", "<"] {
        let Some((left, right)) = expr.split_once(op) else {
            continue;
        };
        let left = operand_value(left.trim(), value, param_name, all_args, true)?;
        let right = operand_value(right.trim(), value, param_name, all_args, false)?;
        return compare_values(&left, &right, op);
    }
    None
}

#[derive(Debug, Clone, PartialEq)]
enum CmpValue {
    Number(f64),
    String(String),
    Bool(bool),
    Symbol(String),
}

fn operand_value(
    raw: &str,
    current: &Value,
    param_name: &str,
    all_args: &Map<String, Value>,
    left_side: bool,
) -> Option<CmpValue> {
    if raw == param_name {
        return cmp_value_from_json(current);
    }
    if let Some(value) = all_args.get(raw) {
        return cmp_value_from_json(value);
    }
    if left_side && is_identifier(raw) {
        return cmp_value_from_json(current);
    }
    if let Ok(number) = raw.parse::<f64>() {
        return Some(CmpValue::Number(number));
    }
    if let Some(unquoted) = raw
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
    {
        return Some(CmpValue::String(unquoted.to_string()));
    }
    is_identifier(raw).then(|| CmpValue::Symbol(raw.to_string()))
}

fn cmp_value_from_json(value: &Value) -> Option<CmpValue> {
    if let Some(number) = value.as_f64() {
        return Some(CmpValue::Number(number));
    }
    if let Some(text) = value.as_str() {
        return Some(CmpValue::String(text.to_string()));
    }
    value.as_bool().map(CmpValue::Bool)
}

fn compare_values(left: &CmpValue, right: &CmpValue, op: &str) -> Option<bool> {
    match (left, right, op) {
        (CmpValue::Number(left), CmpValue::Number(right), ">=") => Some(left >= right),
        (CmpValue::Number(left), CmpValue::Number(right), "<=") => Some(left <= right),
        (CmpValue::Number(left), CmpValue::Number(right), ">") => Some(left > right),
        (CmpValue::Number(left), CmpValue::Number(right), "<") => Some(left < right),
        (_, _, "=" | "==") => Some(left == right),
        (_, _, "!=") => Some(left != right),
        _ => None,
    }
}

fn string_predicate(value: &Value, expr: &str) -> Option<bool> {
    let text = value.as_str()?;
    let (name, _) = expr.split_once('(')?;
    let pred = name.trim();
    let suffixes = pred.strip_prefix("ends_with_")?;
    Some(suffixes.split("_or_").any(|suffix| {
        let suffix = suffix.trim_start_matches('.');
        text.ends_with(&format!(".{suffix}")) || text.ends_with(suffix)
    }))
}

fn json_integer(value: &Value) -> Option<i128> {
    value
        .as_i64()
        .map(i128::from)
        .or_else(|| value.as_u64().map(i128::from))
}

fn is_identifier(raw: &str) -> bool {
    let mut chars = raw.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first == '_' || first.is_ascii_alphabetic())
        && chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn object(value: Value) -> Map<String, Value> {
        value.as_object().unwrap().clone()
    }

    #[test]
    fn numeric_refinements_must_hold() {
        let args = object(json!({"amount": 10.5, "max_slippage": 100}));
        assert!(lambda_arg_matches_type(
            args.get("amount").unwrap(),
            "amount",
            &args,
            "real{a > 0}",
        ));
        assert!(!lambda_arg_matches_type(
            &json!(-1),
            "amount",
            &args,
            "real{a > 0}",
        ));
        assert!(lambda_arg_matches_type(
            args.get("max_slippage").unwrap(),
            "max_slippage",
            &args,
            "int{(s >= 0 && s <= 1000)}",
        ));
        assert!(!lambda_arg_matches_type(
            &json!(1001),
            "max_slippage",
            &args,
            "int{(s >= 0 && s <= 1000)}",
        ));
    }

    #[test]
    fn nat_rejects_negative_integers() {
        let args = Map::new();
        assert!(lambda_arg_matches_type(&json!(0), "n", &args, "nat"));
        assert!(!lambda_arg_matches_type(&json!(-1), "n", &args, "nat"));
    }

    #[test]
    fn string_suffix_refinements_must_hold() {
        let args = object(json!({"path": "report.pdf"}));
        assert!(lambda_arg_matches_type(
            args.get("path").unwrap(),
            "path",
            &args,
            "str{ends_with_pdf(p)}",
        ));
        assert!(!lambda_arg_matches_type(
            args.get("path").unwrap(),
            "path",
            &args,
            "str{ends_with_xlsx(p)}",
        ));
    }

    #[test]
    fn cross_argument_comparisons_must_hold() {
        let args = object(json!({"from": "USDC", "to": "ETH"}));
        assert!(lambda_arg_matches_type(
            args.get("to").unwrap(),
            "to",
            &args,
            "TokenAddr{to != from}",
        ));
        assert!(lambda_arg_matches_type(
            args.get("to").unwrap(),
            "to",
            &args,
            "TokenAddr{from != to}",
        ));
        let args = object(json!({"from": "USDC", "to": "USDC"}));
        assert!(!lambda_arg_matches_type(
            args.get("to").unwrap(),
            "to",
            &args,
            "TokenAddr{to != from}",
        ));
    }

    #[test]
    fn unsupported_refinements_fail_closed() {
        let args = object(json!({"value": "abc"}));
        assert!(!lambda_arg_matches_type(
            args.get("value").unwrap(),
            "value",
            &args,
            "str{valid_address(a)}",
        ));
    }
}
