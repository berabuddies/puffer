use crate::runtime::claude_tools::execute_workflow_tool;
use crate::AppState;
use anyhow::{anyhow, bail, Result};
use puffer_resources::LoadedResources;
use serde_json::{json, Value};

const DEFAULT_TELEGRAM_CONNECTION: &str = "telegram-user";

enum TelegramCommandPlan {
    Help(String),
    Tool(Value),
}

/// Executes the deterministic local `/telegram` slash command.
pub(crate) fn handle_telegram_command(
    state: &mut AppState,
    resources: &LoadedResources,
    args: &str,
) -> Result<String> {
    match parse_telegram_args(args)? {
        TelegramCommandPlan::Help(help) => Ok(help),
        TelegramCommandPlan::Tool(input) => {
            let cwd = state.cwd.clone();
            execute_workflow_tool(state, resources, &cwd, "Telegram", input, None)
        }
    }
}

fn parse_telegram_args(args: &str) -> Result<TelegramCommandPlan> {
    let mut tokens = shell_words::split(args)
        .map_err(|error| anyhow!("invalid /telegram arguments: {error}"))?;
    let connection_slug = extract_connection_slug(&mut tokens)?;
    let Some(command) = tokens.first().cloned() else {
        return Ok(TelegramCommandPlan::Help(help_text()));
    };
    if matches!(command.as_str(), "-h" | "--help" | "help") {
        return Ok(TelegramCommandPlan::Help(help_text()));
    }
    tokens.remove(0);
    let input = match command.as_str() {
        "list-peers" => list_peers_input(tokens, connection_slug)?,
        "search-peers" | "peers" | "find-peer" | "find-peers" => {
            search_peers_input(tokens, connection_slug)?
        }
        "list-messages" | "messages" | "recent" => list_messages_input(tokens, connection_slug)?,
        "search-messages" | "search" => search_messages_input(tokens, connection_slug)?,
        "connect" | "login" | "login-qr" | "login-start" | "import-desktop" => {
            return Ok(TelegramCommandPlan::Help(format!(
                "Use `/connect telegram-login {connection_slug}` for Telegram auth. Lookup commands stay local here:\n\n{}",
                usage_lines()
            )));
        }
        _ => {
            tokens.insert(0, command);
            search_peers_input(tokens, connection_slug)?
        }
    };
    Ok(TelegramCommandPlan::Tool(input))
}

fn extract_connection_slug(tokens: &mut Vec<String>) -> Result<String> {
    let mut connection_slug = DEFAULT_TELEGRAM_CONNECTION.to_string();
    let mut index = 0;
    while index < tokens.len() {
        let token = tokens[index].clone();
        if let Some(value) = prefixed_value(&token, "--connection=")
            .or_else(|| prefixed_value(&token, "--connection-slug="))
            .or_else(|| prefixed_value(&token, "--account="))
            .or_else(|| prefixed_value(&token, "--account-slug="))
        {
            connection_slug = non_empty(value, "connection slug")?;
            tokens.remove(index);
            continue;
        }
        if matches!(
            token.as_str(),
            "--connection" | "--connection-slug" | "--account" | "--account-slug"
        ) {
            let Some(value) = tokens.get(index + 1).cloned() else {
                bail!("{token} requires a value");
            };
            connection_slug = non_empty(value, "connection slug")?;
            tokens.drain(index..=index + 1);
            continue;
        }
        index += 1;
    }
    Ok(connection_slug)
}

fn list_peers_input(tokens: Vec<String>, connection_slug: String) -> Result<Value> {
    let mut query = None;
    let mut peer_kind = None;
    let mut limit = 50usize;
    let mut positionals = Vec::new();
    let mut iter = tokens.into_iter();
    while let Some(token) = iter.next() {
        if token == "--query" {
            query = Some(non_empty(require_next(&mut iter, "--query")?, "query")?);
        } else if let Some(value) = prefixed_value(&token, "--query=") {
            query = Some(non_empty(value, "query")?);
        } else if token == "--kind" {
            peer_kind = Some(parse_peer_kind(&require_next(&mut iter, "--kind")?)?);
        } else if let Some(value) = prefixed_value(&token, "--kind=") {
            peer_kind = Some(parse_peer_kind(&value)?);
        } else if token == "--limit" {
            limit = parse_usize(&require_next(&mut iter, "--limit")?, "--limit")?;
        } else if let Some(value) = prefixed_value(&token, "--limit=") {
            limit = parse_usize(&value, "--limit")?;
        } else if token.starts_with('-') {
            bail!("unknown /telegram list-peers option `{token}`");
        } else {
            positionals.push(token);
        }
    }
    if query.is_none() && !positionals.is_empty() {
        query = Some(positionals.join(" "));
    }
    Ok(json!({
        "action": "list_peers",
        "connection_slug": connection_slug,
        "query": query,
        "peer_kind": peer_kind,
        "limit": limit,
    }))
}

fn search_peers_input(tokens: Vec<String>, connection_slug: String) -> Result<Value> {
    let mut input = list_peers_input(tokens, connection_slug)?;
    input["action"] = json!("search_peers");
    let query = input
        .get("query")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default();
    if query.is_empty() {
        bail!("/telegram search-peers requires a query");
    }
    Ok(input)
}

fn list_messages_input(tokens: Vec<String>, connection_slug: String) -> Result<Value> {
    let mut peer = None;
    let mut limit = 20usize;
    let mut before_id = None;
    let mut sender = None;
    let mut scan_limit = None;
    let mut succinct = true;
    let mut positionals = Vec::new();
    let mut iter = tokens.into_iter();
    while let Some(token) = iter.next() {
        if token == "--peer" {
            peer = Some(non_empty(require_next(&mut iter, "--peer")?, "peer")?);
        } else if let Some(value) = prefixed_value(&token, "--peer=") {
            peer = Some(non_empty(value, "peer")?);
        } else if token == "--limit" {
            limit = parse_usize(&require_next(&mut iter, "--limit")?, "--limit")?;
        } else if let Some(value) = prefixed_value(&token, "--limit=") {
            limit = parse_usize(&value, "--limit")?;
        } else if token == "--before-id" {
            before_id = Some(parse_i32(
                &require_next(&mut iter, "--before-id")?,
                "--before-id",
            )?);
        } else if let Some(value) = prefixed_value(&token, "--before-id=") {
            before_id = Some(parse_i32(&value, "--before-id")?);
        } else if matches!(token.as_str(), "--from" | "--sender" | "--from-user") {
            sender = Some(non_empty(require_next(&mut iter, &token)?, "sender")?);
        } else if let Some(value) = prefixed_value(&token, "--from=")
            .or_else(|| prefixed_value(&token, "--sender="))
            .or_else(|| prefixed_value(&token, "--from-user="))
        {
            sender = Some(non_empty(value, "sender")?);
        } else if token == "--scan-limit" {
            scan_limit = Some(parse_usize(
                &require_next(&mut iter, "--scan-limit")?,
                "--scan-limit",
            )?);
        } else if let Some(value) = prefixed_value(&token, "--scan-limit=") {
            scan_limit = Some(parse_usize(&value, "--scan-limit")?);
        } else if matches!(token.as_str(), "--json" | "--full") {
            succinct = false;
        } else if matches!(token.as_str(), "--succint" | "--succinct") {
            succinct = true;
        } else if token.starts_with('-') {
            bail!("unknown /telegram list-messages option `{token}`");
        } else {
            positionals.push(token);
        }
    }
    if peer.is_none() && !positionals.is_empty() {
        peer = Some(positionals.remove(0));
    }
    let peer = peer.ok_or_else(|| anyhow!("/telegram list-messages requires --peer <peer>"))?;
    Ok(json!({
        "action": "list_messages",
        "connection_slug": connection_slug,
        "peer": peer,
        "limit": limit,
        "before_id": before_id,
        "sender": sender,
        "scan_limit": scan_limit,
        "succinct": succinct,
    }))
}

fn search_messages_input(tokens: Vec<String>, connection_slug: String) -> Result<Value> {
    let mut peer = None;
    let mut query = None;
    let mut limit = 10usize;
    let mut context = 0usize;
    let mut succinct = true;
    let mut positionals = Vec::new();
    let mut iter = tokens.into_iter();
    while let Some(token) = iter.next() {
        if token == "--peer" {
            peer = Some(non_empty(require_next(&mut iter, "--peer")?, "peer")?);
        } else if let Some(value) = prefixed_value(&token, "--peer=") {
            peer = Some(non_empty(value, "peer")?);
        } else if token == "--query" {
            query = Some(non_empty(require_next(&mut iter, "--query")?, "query")?);
        } else if let Some(value) = prefixed_value(&token, "--query=") {
            query = Some(non_empty(value, "query")?);
        } else if token == "--limit" {
            limit = parse_usize(&require_next(&mut iter, "--limit")?, "--limit")?;
        } else if let Some(value) = prefixed_value(&token, "--limit=") {
            limit = parse_usize(&value, "--limit")?;
        } else if token == "--context" {
            context = parse_usize(&require_next(&mut iter, "--context")?, "--context")?;
        } else if let Some(value) = prefixed_value(&token, "--context=") {
            context = parse_usize(&value, "--context")?;
        } else if matches!(token.as_str(), "--json" | "--full") {
            succinct = false;
        } else if matches!(token.as_str(), "--succint" | "--succinct") {
            succinct = true;
        } else if token.starts_with('-') {
            bail!("unknown /telegram search-messages option `{token}`");
        } else {
            positionals.push(token);
        }
    }
    if query.is_none() && !positionals.is_empty() {
        query = Some(positionals.join(" "));
    }
    let peer = peer.ok_or_else(|| anyhow!("/telegram search-messages requires --peer <peer>"))?;
    let query = query.ok_or_else(|| anyhow!("/telegram search-messages requires a query"))?;
    Ok(json!({
        "action": "search_messages",
        "connection_slug": connection_slug,
        "peer": peer,
        "query": query,
        "limit": limit,
        "context": context,
        "succinct": succinct,
    }))
}

fn require_next(iter: &mut impl Iterator<Item = String>, option: &str) -> Result<String> {
    iter.next()
        .ok_or_else(|| anyhow!("{option} requires a value"))
}

fn parse_peer_kind(value: &str) -> Result<&'static str> {
    match value {
        "user" | "users" => Ok("user"),
        "group" | "groups" => Ok("group"),
        "channel" | "channels" => Ok("channel"),
        _ => bail!("unknown Telegram peer kind `{value}`"),
    }
}

fn parse_usize(value: &str, option: &str) -> Result<usize> {
    value
        .parse::<usize>()
        .map_err(|_| anyhow!("{option} must be a positive integer"))
}

fn parse_i32(value: &str, option: &str) -> Result<i32> {
    value
        .parse::<i32>()
        .map_err(|_| anyhow!("{option} must be an integer"))
}

fn prefixed_value(token: &str, prefix: &str) -> Option<String> {
    token.strip_prefix(prefix).map(ToString::to_string)
}

fn non_empty(value: String, label: &str) -> Result<String> {
    let value = value.trim();
    if value.is_empty() {
        bail!("{label} must not be empty");
    }
    Ok(value.to_string())
}

fn help_text() -> String {
    format!(
        "Telegram lookup commands run locally; use `/connect telegram-login <connection>` for auth.\n\n{}",
        usage_lines()
    )
}

fn usage_lines() -> &'static str {
    "/telegram [--connection localtg] search-peers <query> [--kind user|group|channel]\n\
     /telegram [--connection localtg] list-peers [--query <query>] [--limit 50]\n\
     /telegram [--connection localtg] search-messages <query> --peer <id|@username> [--context 0] [--limit 10]\n\
     /telegram [--connection localtg] list-messages --peer <id|@username> [--from <sender>] [--limit 20] [--before-id <id>]"
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool(args: &str) -> Value {
        match parse_telegram_args(args).unwrap() {
            TelegramCommandPlan::Tool(input) => input,
            TelegramCommandPlan::Help(_) => panic!("expected tool plan"),
        }
    }

    #[test]
    fn search_messages_defaults_to_fast_succinct_context() {
        let input = tool("--connection localtg search-messages karen --peer 477843728");
        assert_eq!(input["action"], "search_messages");
        assert_eq!(input["connection_slug"], "localtg");
        assert_eq!(input["peer"], "477843728");
        assert_eq!(input["query"], "karen");
        assert_eq!(input["context"], 0);
        assert_eq!(input["succinct"], true);
    }

    #[test]
    fn list_messages_builds_cursor_request() {
        let input = tool("messages --peer 477843728 --limit 30 --before-id 325");
        assert_eq!(input["action"], "list_messages");
        assert_eq!(input["limit"], 30);
        assert_eq!(input["before_id"], 325);
    }

    #[test]
    fn list_messages_accepts_sender_filter_and_scan_limit() {
        let input =
            tool(r#"messages --peer 477843728 --from "Tony Ke" --scan-limit 200 --limit 25"#);
        assert_eq!(input["action"], "list_messages");
        assert_eq!(input["sender"], "Tony Ke");
        assert_eq!(input["scan_limit"], 200);
        assert_eq!(input["limit"], 25);
    }

    #[test]
    fn bare_text_searches_peers() {
        let input = tool("--account localtg tonyke");
        assert_eq!(input["action"], "search_peers");
        assert_eq!(input["connection_slug"], "localtg");
        assert_eq!(input["query"], "tonyke");
    }
}
