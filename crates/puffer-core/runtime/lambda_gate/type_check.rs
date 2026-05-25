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

/// Returns refinements in this type that the runtime cannot evaluate.
pub(super) fn unsupported_refinements_in_type(ty: &str) -> Vec<String> {
    let (_, refinement) = split_refinement(ty);
    refinement
        .map(unsupported_refinements_in_expr)
        .unwrap_or_default()
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
    if let Some(result) = runtime_predicate(value, expr) {
        return result;
    }
    if let Some(result) = string_predicate(value, expr) {
        return result;
    }
    false
}

fn unsupported_refinements_in_expr(expr: &str) -> Vec<String> {
    let expr = strip_outer_parens(expr.trim());
    if expr.is_empty() {
        return Vec::new();
    }
    let and_parts = split_top_level(expr, "&&");
    if and_parts.len() > 1 {
        return and_parts
            .iter()
            .flat_map(|part| unsupported_refinements_in_expr(part))
            .collect();
    }
    let or_parts = split_top_level(expr, "||");
    if or_parts.len() > 1 {
        return or_parts
            .iter()
            .flat_map(|part| unsupported_refinements_in_expr(part))
            .collect();
    }
    if compare_expr_shape(expr) || runtime_predicate_shape(expr) || string_predicate_shape(expr) {
        return Vec::new();
    }
    vec![expr.to_string()]
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

fn compare_expr_shape(expr: &str) -> bool {
    [">=", "<=", "!=", "==", "=", ">", "<"]
        .into_iter()
        .any(|op| {
            expr.split_once(op)
                .is_some_and(|(left, right)| !left.trim().is_empty() && !right.trim().is_empty())
        })
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

fn string_predicate_shape(expr: &str) -> bool {
    expr.split_once('(')
        .is_some_and(|(name, _)| name.trim().starts_with("ends_with_"))
}

fn runtime_predicate(value: &Value, expr: &str) -> Option<bool> {
    let (name, _) = expr.split_once('(')?;
    match name.trim() {
        "valid_arxiv_id" => value.as_str().map(valid_arxiv_id_list),
        "parsed_ok" => Some(parsed_paper_value(value)),
        _ => None,
    }
}

fn runtime_predicate_shape(expr: &str) -> bool {
    expr.split_once('(')
        .is_some_and(|(name, _)| matches!(name.trim(), "valid_arxiv_id" | "parsed_ok"))
}

fn valid_arxiv_id_list(text: &str) -> bool {
    let mut seen = false;
    for item in text.split(',') {
        let id = item.trim();
        if id.is_empty() || !valid_arxiv_id(id) {
            return false;
        }
        seen = true;
    }
    seen
}

fn valid_arxiv_id(id: &str) -> bool {
    let base = id
        .rsplit_once('v')
        .and_then(|(prefix, suffix)| {
            (!suffix.is_empty() && suffix.chars().all(|ch| ch.is_ascii_digit())).then_some(prefix)
        })
        .unwrap_or(id);
    valid_new_arxiv_id(base) || valid_old_arxiv_id(base)
}

fn valid_new_arxiv_id(id: &str) -> bool {
    let Some((ym, number)) = id.split_once('.') else {
        return false;
    };
    ym.len() == 4
        && ym.chars().all(|ch| ch.is_ascii_digit())
        && matches!(number.len(), 4 | 5)
        && number.chars().all(|ch| ch.is_ascii_digit())
}

fn valid_old_arxiv_id(id: &str) -> bool {
    let Some((archive, number)) = id.split_once('/') else {
        return false;
    };
    !archive.is_empty()
        && archive
            .chars()
            .all(|ch| ch.is_ascii_alphabetic() || matches!(ch, '-' | '.'))
        && number.len() == 7
        && number.chars().all(|ch| ch.is_ascii_digit())
}

fn parsed_paper_value(value: &Value) -> bool {
    let Some(object) = value.as_object() else {
        return false;
    };
    let has_title = object
        .get("title")
        .and_then(Value::as_str)
        .is_some_and(|title| !title.trim().is_empty());
    let has_valid_id = ["arxiv_id", "id", "eprint"]
        .into_iter()
        .filter_map(|key| object.get(key).and_then(Value::as_str))
        .any(valid_arxiv_id);
    has_title && has_valid_id
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
    fn arxiv_id_refinements_must_hold() {
        let args = object(json!({"id": "2402.03300"}));
        assert!(lambda_arg_matches_type(
            args.get("id").unwrap(),
            "id",
            &args,
            "str{valid_arxiv_id(id)}",
        ));
        let args = object(json!({"id": "hep-th/0601001v2"}));
        assert!(lambda_arg_matches_type(
            args.get("id").unwrap(),
            "id",
            &args,
            "str{valid_arxiv_id(id)}",
        ));
        let args = object(json!({"id_list": "2402.03300, 1706.03762v7"}));
        assert!(lambda_arg_matches_type(
            args.get("id_list").unwrap(),
            "id_list",
            &args,
            "str{valid_arxiv_id(id)}",
        ));
        let args = object(json!({"id": "https://arxiv.org/abs/2402.03300"}));
        assert!(!lambda_arg_matches_type(
            args.get("id").unwrap(),
            "id",
            &args,
            "str{valid_arxiv_id(id)}",
        ));
    }

    #[test]
    fn parsed_ok_refinements_must_hold() {
        let args = object(json!({
            "paper": {"title": "Attention Is All You Need", "arxiv_id": "1706.03762v7"}
        }));
        assert!(lambda_arg_matches_type(
            args.get("paper").unwrap(),
            "paper",
            &args,
            "Paper{parsed_ok(p)}",
        ));
        let args = object(json!({"paper": {"title": "Missing identifier"}}));
        assert!(!lambda_arg_matches_type(
            args.get("paper").unwrap(),
            "paper",
            &args,
            "Paper{parsed_ok(p)}",
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

    #[test]
    fn unsupported_refinements_are_reported_for_readiness() {
        assert_eq!(
            unsupported_refinements_in_type("str{(valid_arxiv_id(id) && ends_with_pdf(path))}"),
            Vec::<String>::new()
        );
        assert!(unsupported_refinements_in_type("int{n > 0 && n <= 10}").is_empty());
        assert_eq!(
            unsupported_refinements_in_type("str{host_custom_rule(x)}"),
            vec!["host_custom_rule(x)".to_string()]
        );
    }
}
