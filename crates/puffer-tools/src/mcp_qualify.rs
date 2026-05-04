//! Tool name qualification for MCP servers.
//!
//! Mirrors codex's `qualify_tools` / `sanitize_responses_api_tool_name` so
//! tools advertised by MCP servers show up to the model as
//! `mcp__<server>__<tool>` — sanitized to match the OpenAI Responses API
//! function-name regex (`^[a-zA-Z0-9_-]+$`), capped at 64 bytes, with a
//! short hash suffix when the sanitized form would otherwise collide.

use sha2::{Digest, Sha256};

pub const MCP_TOOL_NAME_DELIMITER: &str = "__";
pub const MAX_TOOL_NAME_LENGTH: usize = 64;
pub const CALLABLE_NAME_HASH_LEN: usize = 12;

/// One MCP tool listing paired with the qualified name we'll show the model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QualifiedMcpTool {
    /// Server identifier as used in `runner.list_mcp_servers()`.
    pub server: String,
    /// Tool name as advertised by the MCP server.
    pub tool: String,
    /// `mcp__<server>__<tool>` after sanitization + collision handling.
    pub qualified_name: String,
}

/// Source row for `qualify_tools`. Kept separate from `QualifiedMcpTool`
/// so the caller can pass through pre-listed `(server, tool)` pairs
/// without committing to a specific schema type.
#[derive(Debug, Clone)]
pub struct McpToolKey {
    pub server: String,
    pub tool: String,
}

/// Builds qualified names for a flat list of MCP `(server, tool)` rows.
///
/// Names that would collide after sanitization get a short SHA-256 suffix.
/// Order is preserved.
pub fn qualify_tools(rows: Vec<McpToolKey>) -> Vec<QualifiedMcpTool> {
    let mut out: Vec<QualifiedMcpTool> = Vec::with_capacity(rows.len());
    let mut seen: std::collections::HashMap<String, usize> = std::collections::HashMap::new();

    for row in rows {
        let base = build_qualified(&row.server, &row.tool, None);
        let qualified = match seen.get(&base) {
            None => base.clone(),
            Some(_) => build_qualified(&row.server, &row.tool, Some(&disambiguator(&row))),
        };
        seen.entry(qualified.clone()).or_insert(0);
        out.push(QualifiedMcpTool {
            server: row.server,
            tool: row.tool,
            qualified_name: qualified,
        });
    }
    out
}

/// Sanitizes a single name into the OpenAI Responses API function-name regex
/// `^[a-zA-Z0-9_-]+$`. Any other character is replaced with `_`.
pub fn sanitize_tool_name(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    if out.is_empty() {
        out.push('_');
    }
    out
}

/// Decodes a qualified name back into `(server, tool)`. Returns `None`
/// when the name is not in the expected `mcp__<server>__<tool>` form.
///
/// Note: when the original tool name contained the `__` delimiter the
/// roundtrip is ambiguous. The runtime executor stores the original
/// `(server, tool)` strings in `handler_args` rather than relying on this
/// helper for dispatch; this is provided only for diagnostics and tests.
pub fn split_qualified_name(qualified: &str) -> Option<(&str, &str)> {
    let stripped = qualified.strip_prefix("mcp")?.strip_prefix(MCP_TOOL_NAME_DELIMITER)?;
    let pos = stripped.find(MCP_TOOL_NAME_DELIMITER)?;
    Some((&stripped[..pos], &stripped[pos + MCP_TOOL_NAME_DELIMITER.len()..]))
}

fn build_qualified(server: &str, tool: &str, suffix: Option<&str>) -> String {
    let server_clean = sanitize_tool_name(server);
    let tool_clean = sanitize_tool_name(tool);
    let prefix = format!(
        "mcp{delim}{server}{delim}",
        delim = MCP_TOOL_NAME_DELIMITER,
        server = server_clean
    );
    let suffix_part = suffix
        .map(|hash| format!("{}{}", MCP_TOOL_NAME_DELIMITER, hash))
        .unwrap_or_default();
    let budget = MAX_TOOL_NAME_LENGTH.saturating_sub(prefix.len() + suffix_part.len());
    let truncated_tool = truncate_to_byte_budget(&tool_clean, budget);
    format!("{prefix}{truncated_tool}{suffix_part}")
}

fn disambiguator(row: &McpToolKey) -> String {
    let mut hasher = Sha256::new();
    hasher.update(row.server.as_bytes());
    hasher.update([0u8]);
    hasher.update(row.tool.as_bytes());
    let digest = hasher.finalize();
    let mut hex = String::with_capacity(CALLABLE_NAME_HASH_LEN);
    for byte in digest.iter().take((CALLABLE_NAME_HASH_LEN + 1) / 2) {
        hex.push_str(&format!("{:02x}", byte));
    }
    hex.truncate(CALLABLE_NAME_HASH_LEN);
    hex
}

fn truncate_to_byte_budget(input: &str, max_bytes: usize) -> String {
    if input.len() <= max_bytes {
        return input.to_string();
    }
    let mut end = max_bytes;
    while end > 0 && !input.is_char_boundary(end) {
        end -= 1;
    }
    input[..end].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(server: &str, tool: &str) -> McpToolKey {
        McpToolKey {
            server: server.to_string(),
            tool: tool.to_string(),
        }
    }

    #[test]
    fn simple_qualification_uses_double_underscore_delimiter() {
        let q = qualify_tools(vec![key("playwright", "browser_navigate")]);
        assert_eq!(q[0].qualified_name, "mcp__playwright__browser_navigate");
    }

    #[test]
    fn invalid_characters_get_sanitized_to_underscore() {
        let q = qualify_tools(vec![key("my server", "do.thing")]);
        assert_eq!(q[0].qualified_name, "mcp__my_server__do_thing");
    }

    #[test]
    fn long_tool_name_truncated_within_64_byte_budget() {
        let long_tool = "a".repeat(200);
        let q = qualify_tools(vec![key("svr", &long_tool)]);
        assert!(q[0].qualified_name.len() <= MAX_TOOL_NAME_LENGTH);
        assert!(q[0].qualified_name.starts_with("mcp__svr__"));
    }

    #[test]
    fn collisions_get_sha_suffix() {
        let q = qualify_tools(vec![
            key("a", "x"),
            key("a", "x"),
        ]);
        assert_eq!(q[0].qualified_name, "mcp__a__x");
        assert_ne!(q[1].qualified_name, "mcp__a__x");
        assert!(q[1].qualified_name.starts_with("mcp__a__x__"));
        assert_eq!(q[1].qualified_name.len(), "mcp__a__x__".len() + CALLABLE_NAME_HASH_LEN);
    }

    #[test]
    fn split_qualified_name_round_trips_simple_case() {
        let q = qualify_tools(vec![key("playwright", "navigate")]);
        let (s, t) = split_qualified_name(&q[0].qualified_name).unwrap();
        assert_eq!(s, "playwright");
        assert_eq!(t, "navigate");
    }

    #[test]
    fn empty_name_components_become_underscore() {
        assert_eq!(sanitize_tool_name(""), "_");
    }
}
