use super::{semantic_predicate, LambdaFact};
use serde_json::{Map, Value};
use std::collections::BTreeSet;

/// Returns whether one JSON formal argument satisfies a Lambda Skill type.
pub(super) fn lambda_arg_matches_type(
    value: &Value,
    param_name: &str,
    all_args: &Map<String, Value>,
    ty: &str,
) -> bool {
    lambda_arg_matches_type_with_facts(value, param_name, all_args, ty, &BTreeSet::new())
}

/// Returns whether one JSON formal argument satisfies a type with gate facts.
pub(super) fn lambda_arg_matches_type_with_facts(
    value: &Value,
    param_name: &str,
    all_args: &Map<String, Value>,
    ty: &str,
    facts: &BTreeSet<LambdaFact>,
) -> bool {
    let (base, refinement) = split_refinement(ty);
    if !base_matches(value, base, param_name, all_args, facts) {
        return false;
    }
    refinement
        .map(|expr| refinement_matches(value, param_name, all_args, expr, facts))
        .unwrap_or(true)
}

/// Returns refinements in this type that the runtime cannot evaluate.
pub(super) fn unsupported_refinements_in_type(ty: &str) -> Vec<String> {
    refinement_segments(ty)
        .into_iter()
        .flat_map(unsupported_refinements_in_expr)
        .collect()
}

/// Returns whether the type carries at least one explicit refinement segment.
pub(super) fn has_refinement_in_type(ty: &str) -> bool {
    !refinement_segments(ty).is_empty()
}

/// Returns predicate names mentioned by all refinements in this type.
pub(super) fn predicate_names_in_type(ty: &str) -> Vec<String> {
    let mut names = refinement_segments(ty)
        .into_iter()
        .flat_map(predicate_names_in_expr)
        .collect::<Vec<_>>();
    names.sort();
    names.dedup();
    names
}

/// Returns custom fact predicate shapes required by refinements in this type.
pub(super) fn fact_refinement_shapes_in_type(ty: &str) -> Vec<(String, usize)> {
    let mut shapes = refinement_segments(ty)
        .into_iter()
        .flat_map(fact_refinement_shapes_in_expr)
        .collect::<Vec<_>>();
    shapes.sort();
    shapes.dedup();
    shapes
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
    facts: &BTreeSet<LambdaFact>,
) -> bool {
    let lowered = base.trim().to_ascii_lowercase();
    if let Some(inner) = lowered.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
        return value.as_array().is_some_and(|items| {
            items.iter().all(|item| {
                lambda_arg_matches_type_with_facts(item, param_name, all_args, inner, facts)
            })
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
    facts: &BTreeSet<LambdaFact>,
) -> bool {
    let expr = strip_outer_parens(expr.trim());
    if expr.is_empty() {
        return true;
    }
    let implication_parts = split_top_level(expr, "==>");
    if implication_parts.len() > 1 {
        let consequent = implication_parts[1..].join("==>");
        return !refinement_matches(value, param_name, all_args, implication_parts[0], facts)
            || refinement_matches(value, param_name, all_args, &consequent, facts);
    }
    let and_parts = split_top_level(expr, "&&");
    if and_parts.len() > 1 {
        return and_parts
            .iter()
            .all(|part| refinement_matches(value, param_name, all_args, part, facts));
    }
    if let Some(inner) = negated_expr(expr) {
        return semantic_refinement_matches(value, param_name, all_args, inner)
            .map(|result| !result)
            .unwrap_or(false);
    }
    let or_parts = split_top_level(expr, "||");
    if or_parts.len() > 1 {
        return or_parts
            .iter()
            .any(|part| refinement_matches(value, param_name, all_args, part, facts));
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
    if let Some(result) = fact_predicate_matches(value, param_name, all_args, expr, facts) {
        return result;
    }
    false
}

fn fact_predicate_matches(
    value: &Value,
    param_name: &str,
    all_args: &Map<String, Value>,
    expr: &str,
    facts: &BTreeSet<LambdaFact>,
) -> Option<bool> {
    if let Some((name, args)) = predicate_call(expr) {
        let mut resolved = Vec::with_capacity(args.len());
        for arg in args {
            resolved.push(resolve_fact_arg(arg, value, param_name, all_args)?);
        }
        return Some(facts.contains(&LambdaFact::new(name, resolved)));
    }
    predicate_atom_name(expr)
        .map(|name| facts.contains(&LambdaFact::new(name, Vec::<String>::new())))
}

fn resolve_fact_arg(
    raw: &str,
    current: &Value,
    param_name: &str,
    all_args: &Map<String, Value>,
) -> Option<String> {
    let token = raw.trim();
    if token.is_empty() {
        return None;
    }
    if token == param_name {
        return Some(canonical_fact_arg(current));
    }
    if let Some(value) = all_args.get(token) {
        return Some(canonical_fact_arg(value));
    }
    if let Some(unquoted) = token
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .or_else(|| {
            token
                .strip_prefix('\'')
                .and_then(|value| value.strip_suffix('\''))
        })
    {
        return Some(unquoted.to_string());
    }
    if is_identifier(token) {
        return Some(canonical_fact_arg(current));
    }
    Some(token.to_string())
}

fn canonical_fact_arg(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| value.to_string())
}

fn semantic_refinement_matches(
    value: &Value,
    param_name: &str,
    all_args: &Map<String, Value>,
    expr: &str,
) -> Option<bool> {
    let expr = strip_outer_parens(expr.trim());
    if expr.is_empty() {
        return Some(true);
    }
    let and_parts = split_top_level(expr, "&&");
    if and_parts.len() > 1 {
        return and_parts.iter().try_fold(true, |acc, part| {
            semantic_refinement_matches(value, param_name, all_args, part)
                .map(|result| acc && result)
        });
    }
    let or_parts = split_top_level(expr, "||");
    if or_parts.len() > 1 {
        return or_parts.iter().try_fold(false, |acc, part| {
            semantic_refinement_matches(value, param_name, all_args, part)
                .map(|result| acc || result)
        });
    }
    if let Some(inner) = negated_expr(expr) {
        return semantic_refinement_matches(value, param_name, all_args, inner)
            .map(|result| !result);
    }
    compare_expr(value, param_name, all_args, expr)
        .or_else(|| runtime_predicate(value, expr))
        .or_else(|| string_predicate(value, expr))
}

fn unsupported_refinements_in_expr(expr: &str) -> Vec<String> {
    let expr = strip_outer_parens(expr.trim());
    if expr.is_empty() {
        return Vec::new();
    }
    if record_field_type_list(expr) {
        return Vec::new();
    }
    let implication_parts = split_top_level(expr, "==>");
    if implication_parts.len() > 1 {
        return implication_parts
            .iter()
            .flat_map(|part| unsupported_refinements_in_expr(part))
            .collect();
    }
    let and_parts = split_top_level(expr, "&&");
    if and_parts.len() > 1 {
        return and_parts
            .iter()
            .flat_map(|part| unsupported_refinements_in_expr(part))
            .collect();
    }
    if negated_expr(expr).is_some() {
        return if semantic_refinement_shape(expr) {
            Vec::new()
        } else {
            vec![expr.to_string()]
        };
    }
    let or_parts = split_top_level(expr, "||");
    if or_parts.len() > 1 {
        return or_parts
            .iter()
            .flat_map(|part| unsupported_refinements_in_expr(part))
            .collect();
    }
    if compare_expr_shape(expr)
        || runtime_predicate_shape(expr)
        || string_predicate_shape(expr)
        || predicate_call_name(expr).is_some()
        || predicate_atom_name(expr).is_some()
    {
        return Vec::new();
    }
    vec![expr.to_string()]
}

fn predicate_names_in_expr(expr: &str) -> Vec<String> {
    let expr = strip_outer_parens(expr.trim());
    if expr.is_empty() {
        return Vec::new();
    }
    if record_field_type_list(expr) {
        return Vec::new();
    }
    let implication_parts = split_top_level(expr, "==>");
    if implication_parts.len() > 1 {
        return implication_parts
            .iter()
            .flat_map(|part| predicate_names_in_expr(part))
            .collect();
    }
    let and_parts = split_top_level(expr, "&&");
    if and_parts.len() > 1 {
        return and_parts
            .iter()
            .flat_map(|part| predicate_names_in_expr(part))
            .collect();
    }
    if let Some(inner) = negated_expr(expr) {
        return predicate_names_in_expr(inner);
    }
    let or_parts = split_top_level(expr, "||");
    if or_parts.len() > 1 {
        return or_parts
            .iter()
            .flat_map(|part| predicate_names_in_expr(part))
            .collect();
    }
    predicate_call_name(expr)
        .or_else(|| predicate_atom_name(expr))
        .map(|name| vec![name.to_string()])
        .unwrap_or_default()
}

fn fact_refinement_shapes_in_expr(expr: &str) -> Vec<(String, usize)> {
    let expr = strip_outer_parens(expr.trim());
    if expr.is_empty() || record_field_type_list(expr) {
        return Vec::new();
    }
    let implication_parts = split_top_level(expr, "==>");
    if implication_parts.len() > 1 {
        return implication_parts
            .into_iter()
            .flat_map(fact_refinement_shapes_in_expr)
            .collect();
    }
    let and_parts = split_top_level(expr, "&&");
    if and_parts.len() > 1 {
        return and_parts
            .into_iter()
            .flat_map(fact_refinement_shapes_in_expr)
            .collect();
    }
    if let Some(inner) = negated_expr(expr) {
        return fact_refinement_shapes_in_expr(inner);
    }
    let or_parts = split_top_level(expr, "||");
    if or_parts.len() > 1 {
        return or_parts
            .into_iter()
            .flat_map(fact_refinement_shapes_in_expr)
            .collect();
    }
    if compare_expr_shape(expr) || runtime_predicate_shape(expr) || string_predicate_shape(expr) {
        return Vec::new();
    }
    if let Some((name, args)) = predicate_call(expr) {
        return vec![(name.to_string(), args.len())];
    }
    predicate_atom_name(expr)
        .map(|name| vec![(name.to_string(), 0)])
        .unwrap_or_default()
}

fn refinement_segments(ty: &str) -> Vec<&str> {
    let mut segments = Vec::new();
    let mut depth = 0usize;
    let mut start = None;
    for (index, ch) in ty.char_indices() {
        match ch {
            '{' => {
                if depth == 0 {
                    start = Some(index + ch.len_utf8());
                }
                depth += 1;
            }
            '}' => {
                if depth == 0 {
                    continue;
                }
                depth -= 1;
                if depth == 0 {
                    if let Some(start_index) = start.take() {
                        segments.push(ty[start_index..index].trim());
                    }
                }
            }
            _ => {}
        }
    }
    segments
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

fn negated_expr(expr: &str) -> Option<&str> {
    let inner = expr.trim().strip_prefix('!')?.trim();
    (!inner.is_empty()).then_some(strip_outer_parens(inner))
}

fn record_field_type_list(expr: &str) -> bool {
    split_top_level(expr, ",").into_iter().all(|field| {
        field
            .split_once(':')
            .is_some_and(|(name, ty)| is_identifier(name.trim()) && !ty.trim().is_empty())
    })
}

fn semantic_refinement_shape(expr: &str) -> bool {
    let expr = strip_outer_parens(expr.trim());
    if expr.is_empty() {
        return true;
    }
    let and_parts = split_top_level(expr, "&&");
    if and_parts.len() > 1 {
        return and_parts
            .into_iter()
            .all(|part| semantic_refinement_shape(part));
    }
    if let Some(inner) = negated_expr(expr) {
        return semantic_refinement_shape(inner);
    }
    let or_parts = split_top_level(expr, "||");
    if or_parts.len() > 1 {
        return or_parts
            .into_iter()
            .all(|part| semantic_refinement_shape(part));
    }
    compare_expr_shape(expr) || runtime_predicate_shape(expr) || string_predicate_shape(expr)
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
    if let Some(value) = current.as_object().and_then(|object| object.get(raw)) {
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
    match raw {
        "true" => return Some(CmpValue::Bool(true)),
        "false" => return Some(CmpValue::Bool(false)),
        _ => {}
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
        (_, _, "=" | "==") => Some(cmp_values_equal(left, right)),
        (_, _, "!=") => Some(!cmp_values_equal(left, right)),
        _ => None,
    }
}

fn cmp_values_equal(left: &CmpValue, right: &CmpValue) -> bool {
    match (left, right) {
        (CmpValue::String(left), CmpValue::Symbol(right))
        | (CmpValue::Symbol(left), CmpValue::String(right)) => left == right,
        _ => left == right,
    }
}

fn string_predicate(value: &Value, expr: &str) -> Option<bool> {
    let text = value.as_str()?;
    let (name, _) = expr.split_once('(')?;
    let pred = name.trim();
    if let Some(suffixes) = pred.strip_prefix("ends_with_") {
        return Some(suffixes.split("_or_").any(|suffix| {
            let suffix = suffix.trim_start_matches('.');
            text.ends_with(&format!(".{suffix}")) || text.ends_with(suffix)
        }));
    }
    if let Some(prefixes) = pred.strip_prefix("starts_with_") {
        return Some(
            prefixes
                .split("_or_")
                .any(|prefix| text.starts_with(prefix)),
        );
    }
    if let Some(needles) = pred.strip_prefix("contains_") {
        return Some(needles.split("_or_").any(|needle| text.contains(needle)));
    }
    None
}

fn string_predicate_shape(expr: &str) -> bool {
    expr.split_once('(').is_some_and(|(name, _)| {
        let name = name.trim();
        name.starts_with("ends_with_")
            || name.starts_with("starts_with_")
            || name.starts_with("contains_")
    })
}

fn runtime_predicate(value: &Value, expr: &str) -> Option<bool> {
    semantic_predicate::matches(value, expr)
}

fn runtime_predicate_shape(expr: &str) -> bool {
    semantic_predicate::is_supported_expr(expr)
}

fn predicate_call(expr: &str) -> Option<(&str, Vec<&str>)> {
    let (name, rest) = expr.split_once('(')?;
    let name = name.trim();
    if !is_identifier(name) {
        return None;
    }
    let args = rest.strip_suffix(')')?;
    if !balanced_parens(args) {
        return None;
    }
    let args = if args.trim().is_empty() {
        Vec::new()
    } else {
        split_top_level(args, ",")
    };
    Some((name, args))
}

fn predicate_call_name(expr: &str) -> Option<&str> {
    predicate_call(expr).map(|(name, _)| name)
}

fn predicate_atom_name(expr: &str) -> Option<&str> {
    is_identifier(expr).then_some(expr.trim())
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
    fn generic_string_refinements_must_hold() {
        let args = object(json!({"path": "reports/final.md"}));
        assert!(lambda_arg_matches_type(
            args.get("path").unwrap(),
            "path",
            &args,
            "str{starts_with_reports(p)}",
        ));
        assert!(lambda_arg_matches_type(
            args.get("path").unwrap(),
            "path",
            &args,
            "str{contains_final(p)}",
        ));
        assert!(lambda_arg_matches_type(
            args.get("path").unwrap(),
            "path",
            &args,
            "str{ends_with_pdf_or_md(p)}",
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
    fn enum_symbol_refinements_match_json_strings() {
        let args = object(
            json!({"cred": "secret", "cred_obj": {"sec": "secret"}, "mode": "private", "wait": true}),
        );
        assert!(lambda_arg_matches_type(
            args.get("cred").unwrap(),
            "cred",
            &args,
            "TrelloCred{sec = secret}",
        ));
        assert!(lambda_arg_matches_type(
            args.get("cred_obj").unwrap(),
            "cred_obj",
            &args,
            "SecretValue{sec = secret}",
        ));
        assert!(lambda_arg_matches_type(
            args.get("wait").unwrap(),
            "wait",
            &args,
            "bool{w = true}",
        ));
        assert!(lambda_arg_matches_type(
            args.get("mode").unwrap(),
            "mode",
            &args,
            "Visibility{mode == private}",
        ));
        assert!(!lambda_arg_matches_type(
            args.get("mode").unwrap(),
            "mode",
            &args,
            "Visibility{mode != private}",
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
    fn host_predicate_facts_guard_custom_refinements() {
        let args = object(json!({"plan": "move files"}));
        let mut facts = BTreeSet::new();
        assert!(!lambda_arg_matches_type_with_facts(
            args.get("plan").unwrap(),
            "plan",
            &args,
            "Plan{plan_approved(p)}",
            &facts,
        ));
        facts.insert(LambdaFact::new(
            "plan_approved",
            vec![serde_json::to_string(args.get("plan").unwrap()).unwrap()],
        ));
        assert!(lambda_arg_matches_type_with_facts(
            args.get("plan").unwrap(),
            "plan",
            &args,
            "Plan{plan_approved(p)}",
            &facts,
        ));
        let mut wrong_facts = BTreeSet::new();
        wrong_facts.insert(LambdaFact::new(
            "plan_approved",
            vec![serde_json::to_string(&json!("delete files")).unwrap()],
        ));
        assert!(!lambda_arg_matches_type_with_facts(
            args.get("plan").unwrap(),
            "plan",
            &args,
            "Plan{plan_approved(p)}",
            &wrong_facts,
        ));
        let mut zero_arg_facts = BTreeSet::new();
        zero_arg_facts.insert(LambdaFact::new("plan_approved", Vec::new()));
        assert!(lambda_arg_matches_type_with_facts(
            args.get("plan").unwrap(),
            "plan",
            &args,
            "Plan{plan_approved}",
            &zero_arg_facts,
        ));
    }

    #[test]
    fn negated_refinements_only_use_semantic_predicates() {
        let args = object(json!({"to": "+14155552671"}));
        assert!(lambda_arg_matches_type(
            args.get("to").unwrap(),
            "to",
            &args,
            "PhoneRecipient{!(emergency_number(r))}",
        ));
        let args = object(json!({"to": "911"}));
        assert!(!lambda_arg_matches_type(
            args.get("to").unwrap(),
            "to",
            &args,
            "PhoneRecipient{!(emergency_number(r))}",
        ));
        let args = object(json!({"value": "anything"}));
        assert!(!lambda_arg_matches_type(
            args.get("value").unwrap(),
            "value",
            &args,
            "str{!(unknown_fact(v))}",
        ));
    }

    #[test]
    fn unsupported_refinements_are_reported_for_readiness() {
        assert_eq!(
            unsupported_refinements_in_type("str{(valid_arxiv_id(id) && ends_with_pdf(path))}"),
            Vec::<String>::new()
        );
        assert!(unsupported_refinements_in_type("int{n > 0 && n <= 10}").is_empty());
        assert!(unsupported_refinements_in_type(
            "Request{uses_budget_tokens(r) ==> budget_tokens < max_tokens}"
        )
        .is_empty());
        assert!(unsupported_refinements_in_type("Result<unit{authed(s)}, Err>").is_empty());
        assert!(unsupported_refinements_in_type(
            "{layout: LayoutName, style: StyleName, aspect: AspectName}"
        )
        .is_empty());
        assert!(unsupported_refinements_in_type(
            "PhoneRecipient{(!(emergency_number(r)) && lawful_phone_use(r))}"
        )
        .is_empty());
        assert_eq!(
            unsupported_refinements_in_type("str{!(unknown_fact(v))}"),
            vec!["!(unknown_fact(v))".to_string()]
        );
        assert!(unsupported_refinements_in_type("Plan{plan_approved}").is_empty());
        assert_eq!(
            predicate_names_in_type("Result<unit{authed(s)}, Err{safe(e) && phase1_done}>"),
            vec![
                "authed".to_string(),
                "phase1_done".to_string(),
                "safe".to_string()
            ]
        );
        assert_eq!(
            fact_refinement_shapes_in_type(
                "Plan{plan_approved(p) && ends_with_md(path) && phase1_done}"
            ),
            vec![
                ("phase1_done".to_string(), 0),
                ("plan_approved".to_string(), 1)
            ]
        );
        assert_eq!(
            unsupported_refinements_in_type("str{host_custom_rule x}"),
            vec!["host_custom_rule x".to_string()]
        );
    }
}
