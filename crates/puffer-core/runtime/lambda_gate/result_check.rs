use super::{semantic_predicate, LambdaFact};
use serde_json::{Map, Value};
use std::collections::BTreeSet;

/// Returns whether a concrete host result satisfies executable result refinements.
pub(super) fn lambda_result_matches_type_with_facts(
    value: &Value,
    formal_args: &Map<String, Value>,
    ty: &str,
    _facts: &BTreeSet<LambdaFact>,
) -> bool {
    value_matches_type(value, "result", formal_args, ty)
}

fn value_matches_type(value: &Value, name: &str, scope: &Map<String, Value>, ty: &str) -> bool {
    let ty = ty.trim();
    if ty.is_empty() {
        return true;
    }
    if let Some(ok_type) = result_ok_type(ty) {
        return value_matches_type(value, name, scope, ok_type);
    }
    if let Some(record) = record_type(ty) {
        return record_matches(value, scope, record);
    }
    let (base, refinement) = split_top_level_refinement(ty);
    if !base_matches(value, name, scope, base) {
        return false;
    }
    refinement
        .map(|expr| refinement_matches(value, name, scope, expr))
        .unwrap_or(true)
}

fn base_matches(value: &Value, name: &str, scope: &Map<String, Value>, base: &str) -> bool {
    let base = base.trim();
    if base.is_empty() {
        return true;
    }
    if let Some(inner) = array_type(base) {
        return value.as_array().is_some_and(|items| {
            items
                .iter()
                .all(|item| value_matches_type(item, name, scope, inner))
        });
    }
    if let Some(ok_type) = result_ok_type(base) {
        return value_matches_type(value, name, scope, ok_type);
    }
    if let Some(record) = record_type(base) {
        return record_matches(value, scope, record);
    }
    match base.to_ascii_lowercase().as_str() {
        "str" | "string" => value.is_string(),
        "int" => json_integer(value).is_some(),
        "nat" => json_integer(value).is_some_and(|number| number >= 0),
        "real" | "float" | "number" => value.as_f64().is_some(),
        "bool" => value.is_boolean(),
        "unit" => value.is_null() || value.as_object().is_some_and(Map::is_empty),
        _ => true,
    }
}

fn refinement_matches(value: &Value, name: &str, scope: &Map<String, Value>, expr: &str) -> bool {
    let expr = strip_outer_parens(expr.trim());
    if expr.is_empty() {
        return true;
    }
    if let Some(record) = record_fields(expr) {
        return record_matches(value, scope, record);
    }
    let implication_parts = split_expr_top_level(expr, "==>");
    if implication_parts.len() > 1 {
        let consequent = implication_parts[1..].join("==>");
        return !refinement_matches(value, name, scope, implication_parts[0])
            || refinement_matches(value, name, scope, &consequent);
    }
    let and_parts = split_expr_top_level(expr, "&&");
    if and_parts.len() > 1 {
        return and_parts
            .iter()
            .all(|part| refinement_matches(value, name, scope, part));
    }
    let or_parts = split_expr_top_level(expr, "||");
    if or_parts.len() > 1 {
        return or_parts
            .iter()
            .any(|part| refinement_matches(value, name, scope, part));
    }
    if let Some(inner) = negated_expr(expr) {
        return semantic_refinement_matches(value, name, scope, inner)
            .map(|result| !result)
            .unwrap_or(false);
    }
    semantic_refinement_matches(value, name, scope, expr)
        .or_else(|| fact_producer_shape(expr).then_some(true))
        .unwrap_or(false)
}

fn semantic_refinement_matches(
    value: &Value,
    name: &str,
    scope: &Map<String, Value>,
    expr: &str,
) -> Option<bool> {
    compare_expr(value, name, scope, expr)
        .or_else(|| semantic_predicate::matches(value, expr))
        .or_else(|| string_predicate(value, expr))
}

fn record_matches(
    value: &Value,
    outer_scope: &Map<String, Value>,
    fields: Vec<(&str, &str)>,
) -> bool {
    let Some(object) = value.as_object() else {
        return false;
    };
    let mut scope = outer_scope.clone();
    scope.extend(
        object
            .iter()
            .map(|(key, value)| (key.clone(), value.clone())),
    );
    fields.into_iter().all(|(field, ty)| {
        object
            .get(field)
            .is_some_and(|value| value_matches_type(value, field, &scope, ty))
    })
}

fn split_top_level_refinement(ty: &str) -> (&str, Option<&str>) {
    let mut paren = 0usize;
    let mut bracket = 0usize;
    let mut angle = 0usize;
    for (index, ch) in ty.char_indices() {
        match ch {
            '(' => paren += 1,
            ')' => paren = paren.saturating_sub(1),
            '[' => bracket += 1,
            ']' => bracket = bracket.saturating_sub(1),
            '<' => angle += 1,
            '>' => angle = angle.saturating_sub(1),
            '{' if paren == 0 && bracket == 0 && angle == 0 => {
                let Some(end) = matching_brace_end(&ty[index..]) else {
                    return (ty.trim(), None);
                };
                if index + end + 1 != ty.len() {
                    return (ty.trim(), None);
                }
                return (ty[..index].trim(), Some(ty[index + 1..index + end].trim()));
            }
            _ => {}
        }
    }
    (ty.trim(), None)
}

fn matching_brace_end(input: &str) -> Option<usize> {
    let mut depth = 0usize;
    for (index, ch) in input.char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(index);
                }
            }
            _ => {}
        }
    }
    None
}

fn array_type(ty: &str) -> Option<&str> {
    ty.strip_prefix('[')
        .and_then(|inner| inner.strip_suffix(']'))
        .map(str::trim)
}

fn result_ok_type(ty: &str) -> Option<&str> {
    let inner = ty.strip_prefix("Result<")?.strip_suffix('>')?;
    split_type_top_level(inner, ",").into_iter().next()
}

fn record_type(ty: &str) -> Option<Vec<(&str, &str)>> {
    let inner = ty.strip_prefix('{')?.strip_suffix('}')?;
    record_fields(inner)
}

fn record_fields(expr: &str) -> Option<Vec<(&str, &str)>> {
    let fields = split_type_top_level(expr, ",");
    if fields.is_empty() {
        return None;
    }
    fields
        .into_iter()
        .map(|field| {
            let (name, ty) = field.split_once(':')?;
            let name = name.trim();
            let ty = ty.trim();
            (is_identifier(name) && !ty.is_empty()).then_some((name, ty))
        })
        .collect()
}

fn split_expr_top_level<'a>(expr: &'a str, op: &str) -> Vec<&'a str> {
    split_top_level(expr, op, false)
}

fn split_type_top_level<'a>(expr: &'a str, op: &str) -> Vec<&'a str> {
    split_top_level(expr, op, true)
}

fn split_top_level<'a>(expr: &'a str, op: &str, track_angle: bool) -> Vec<&'a str> {
    let mut parts = Vec::new();
    let mut depth = Nesting::default();
    let mut start = 0usize;
    let mut index = 0usize;
    while index < expr.len() {
        let rest = &expr[index..];
        if depth.is_top_level() && rest.starts_with(op) {
            parts.push(expr[start..index].trim());
            index += op.len();
            start = index;
            continue;
        }
        let Some(ch) = rest.chars().next() else {
            break;
        };
        depth.observe(ch, track_angle);
        index += ch.len_utf8();
    }
    if parts.is_empty() {
        return vec![expr.trim()];
    }
    parts.push(expr[start..].trim());
    parts
}

#[derive(Default)]
struct Nesting {
    paren: usize,
    bracket: usize,
    brace: usize,
    angle: usize,
}

impl Nesting {
    fn is_top_level(&self) -> bool {
        self.paren == 0 && self.bracket == 0 && self.brace == 0 && self.angle == 0
    }

    fn observe(&mut self, ch: char, track_angle: bool) {
        match ch {
            '(' => self.paren += 1,
            ')' => self.paren = self.paren.saturating_sub(1),
            '[' => self.bracket += 1,
            ']' => self.bracket = self.bracket.saturating_sub(1),
            '{' => self.brace += 1,
            '}' => self.brace = self.brace.saturating_sub(1),
            '<' if track_angle => self.angle += 1,
            '>' if track_angle => self.angle = self.angle.saturating_sub(1),
            _ => {}
        }
    }
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

fn negated_expr(expr: &str) -> Option<&str> {
    let inner = expr.trim().strip_prefix('!')?.trim();
    (!inner.is_empty()).then_some(strip_outer_parens(inner))
}

fn compare_expr(value: &Value, name: &str, scope: &Map<String, Value>, expr: &str) -> Option<bool> {
    for op in [">=", "<=", "!=", "==", "=", ">", "<"] {
        let Some((left, right)) = expr.split_once(op) else {
            continue;
        };
        let left = operand_value(left.trim(), value, name, scope, true)?;
        let right = operand_value(right.trim(), value, name, scope, false)?;
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
    name: &str,
    scope: &Map<String, Value>,
    left_side: bool,
) -> Option<CmpValue> {
    if raw == name {
        return cmp_value_from_json(current);
    }
    if let Some(value) = scope.get(raw) {
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

fn fact_producer_shape(expr: &str) -> bool {
    predicate_call_name(expr).is_some() || predicate_atom_name(expr).is_some()
}

fn predicate_call_name(expr: &str) -> Option<&str> {
    let (name, rest) = expr.split_once('(')?;
    rest.strip_suffix(')')?;
    let name = name.trim();
    is_identifier(name).then_some(name)
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

    #[test]
    fn enforces_result_semantic_and_string_refinements() {
        let args = Map::new();
        assert!(lambda_result_matches_type_with_facts(
            &json!("paper.pdf"),
            &args,
            "str{ends_with_pdf(p)}",
            &BTreeSet::new()
        ));
        assert!(!lambda_result_matches_type_with_facts(
            &json!("paper.txt"),
            &args,
            "str{ends_with_pdf(p)}",
            &BTreeSet::new()
        ));
    }

    #[test]
    fn enforces_record_result_refinements() {
        let args = Map::new();
        let ty = "{host: BindHost{loopback_only(h)}, port: BindPort{ephemeral_port(p)}}";
        assert!(lambda_result_matches_type_with_facts(
            &json!({"host": "127.0.0.1", "port": 3000}),
            &args,
            ty,
            &BTreeSet::new()
        ));
        assert!(!lambda_result_matches_type_with_facts(
            &json!({"host": "example.com", "port": 3000}),
            &args,
            ty,
            &BTreeSet::new()
        ));
        assert!(!lambda_result_matches_type_with_facts(
            &json!({"host": "127.0.0.1", "port": 80}),
            &args,
            ty,
            &BTreeSet::new()
        ));
    }

    #[test]
    fn enforces_object_field_comparisons_and_implications() {
        let args = Map::new();
        assert!(lambda_result_matches_type_with_facts(
            &json!({"sec": "secret"}),
            &args,
            "SecretValue{sec = secret}",
            &BTreeSet::new()
        ));
        assert!(lambda_result_matches_type_with_facts(
            &json!({"uses_budget_tokens": true, "budget_tokens": 1024, "max_tokens": 2048}),
            &args,
            "Request{uses_budget_tokens(r) ==> budget_tokens < max_tokens}",
            &BTreeSet::new()
        ));
        assert!(!lambda_result_matches_type_with_facts(
            &json!({"uses_budget_tokens": true, "budget_tokens": 4096, "max_tokens": 2048}),
            &args,
            "Request{uses_budget_tokens(r) ==> budget_tokens < max_tokens}",
            &BTreeSet::new()
        ));
    }

    #[test]
    fn checks_success_branch_of_result_types() {
        let args = Map::new();
        let ty = "Result<{source_lang: LangCode, user_lang: LangCode{user_lang != source_lang}}, LangCode>";
        assert!(lambda_result_matches_type_with_facts(
            &json!({"source_lang": "en", "user_lang": "zh"}),
            &args,
            ty,
            &BTreeSet::new()
        ));
        assert!(!lambda_result_matches_type_with_facts(
            &json!({"source_lang": "en", "user_lang": "en"}),
            &args,
            ty,
            &BTreeSet::new()
        ));
    }

    #[test]
    fn enforces_graphql_success_result_refinements() {
        let args = Map::new();
        assert!(lambda_result_matches_type_with_facts(
            &json!({"data": {"mutation": {"userErrors": []}}}),
            &args,
            "Result<GqlResponse{gql_success(r)}, ShopifyErr>",
            &BTreeSet::new()
        ));
        assert!(!lambda_result_matches_type_with_facts(
            &json!({"data": {"mutation": {"userErrors": [{"message": "bad"}]}}}),
            &args,
            "Result<GqlResponse{gql_success(r)}, ShopifyErr>",
            &BTreeSet::new()
        ));
        assert!(!lambda_result_matches_type_with_facts(
            &json!({"errors": [{"message": "bad"}]}),
            &args,
            "Result<GqlResponse{gql_success(r)}, ShopifyErr>",
            &BTreeSet::new()
        ));
    }

    #[test]
    fn treats_positive_custom_result_facts_as_producers() {
        let args = Map::new();
        assert!(lambda_result_matches_type_with_facts(
            &json!("PR-7"),
            &args,
            "PRRef{pr_resolved(p)}",
            &BTreeSet::new()
        ));
    }

    #[test]
    fn treats_array_result_fact_refinements_as_producers() {
        let args = Map::new();
        assert!(lambda_result_matches_type_with_facts(
            &json!([{"title": "x", "arxiv_id": "2605.13044"}]),
            &args,
            "[Paper]{parsed_ok(p)}",
            &BTreeSet::new()
        ));
    }
}
