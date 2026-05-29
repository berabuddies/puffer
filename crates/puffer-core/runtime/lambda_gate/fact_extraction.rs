use super::{canonical_fact_arg, semantic_predicate, LambdaFact};
use serde_json::{Map, Value};
use std::collections::BTreeSet;

/// Instantiates positive custom fact refinements from a host result type.
pub(super) fn facts_from_result_refinements(
    ty: &str,
    result: &Value,
    formal_args: &Map<String, Value>,
) -> Vec<LambdaFact> {
    refinement_segments(ty)
        .into_iter()
        .flat_map(|segment| facts_from_expr(&segment, result, formal_args))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn facts_from_expr(
    expr: &str,
    result: &Value,
    formal_args: &Map<String, Value>,
) -> Vec<LambdaFact> {
    let expr = strip_outer_parens(expr.trim());
    if expr.is_empty() || record_field_type_list(expr) {
        return Vec::new();
    }
    let and_parts = split_top_level(expr, "&&");
    if and_parts.len() > 1 {
        return and_parts
            .into_iter()
            .flat_map(|part| facts_from_expr(part, result, formal_args))
            .collect();
    }
    let or_parts = split_top_level(expr, "||");
    if or_parts.len() > 1 {
        return Vec::new();
    }
    if expr.starts_with('!')
        || compare_expr_shape(expr)
        || runtime_predicate_shape(expr)
        || string_predicate_shape(expr)
    {
        return Vec::new();
    }
    if let Some((name, args)) = predicate_call(expr) {
        let Some(resolved) = args
            .into_iter()
            .map(|arg| resolve_fact_arg(arg, result, formal_args))
            .collect::<Option<Vec<_>>>()
        else {
            return Vec::new();
        };
        return vec![LambdaFact::new(name, resolved)];
    }
    predicate_atom_name(expr)
        .map(|name| vec![LambdaFact::new(name, Vec::<String>::new())])
        .unwrap_or_default()
}

fn refinement_segments(ty: &str) -> Vec<String> {
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
                        segments.push(ty[start_index..index].trim().to_string());
                    }
                }
            }
            _ => {}
        }
    }
    segments
}

fn resolve_fact_arg(raw: &str, result: &Value, formal_args: &Map<String, Value>) -> Option<String> {
    let token = raw.trim();
    if token.is_empty() {
        return None;
    }
    if let Some(value) = formal_args.get(token) {
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
        return Some(canonical_fact_arg(result));
    }
    Some(token.to_string())
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

fn record_field_type_list(expr: &str) -> bool {
    split_top_level(expr, ",").into_iter().all(|field| {
        field
            .split_once(':')
            .is_some_and(|(name, ty)| is_identifier(name.trim()) && !ty.trim().is_empty())
    })
}

fn compare_expr_shape(expr: &str) -> bool {
    [">=", "<=", "!=", "==", "=", ">", "<"]
        .into_iter()
        .any(|op| {
            expr.split_once(op)
                .is_some_and(|(left, right)| !left.trim().is_empty() && !right.trim().is_empty())
        })
}

fn runtime_predicate_shape(expr: &str) -> bool {
    semantic_predicate::is_supported_expr(expr)
}

fn string_predicate_shape(expr: &str) -> bool {
    expr.split_once('(').is_some_and(|(name, _)| {
        let name = name.trim();
        name.starts_with("ends_with_")
            || name.starts_with("starts_with_")
            || name.starts_with("contains_")
    })
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

fn predicate_atom_name(expr: &str) -> Option<&str> {
    is_identifier(expr).then_some(expr.trim())
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
    fn extracts_positive_result_facts() {
        let args = json!({"repo": "puffer"}).as_object().unwrap().clone();
        let facts = facts_from_result_refinements(
            "Result{is_issue(r) && linked_to(repo) && valid_url(r) && is_folder(r)}",
            &json!("ISSUE-1"),
            &args,
        );

        assert!(facts.contains(&LambdaFact::new(
            "is_issue",
            vec![serde_json::to_string(&json!("ISSUE-1")).unwrap()]
        )));
        assert!(facts.contains(&LambdaFact::new(
            "linked_to",
            vec![serde_json::to_string(&json!("puffer")).unwrap()]
        )));
        assert_eq!(facts.len(), 2);
    }

    #[test]
    fn does_not_extract_disjunctive_facts() {
        let facts = facts_from_result_refinements(
            "Result{left_ok(x) || right_ok(x)}",
            &json!("receipt"),
            &Map::new(),
        );

        assert!(facts.is_empty());
    }
}
