use crate::AppState;
use anyhow::{bail, Context, Result};
use reqwest::blocking::Client;
use reqwest::Method;
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::Path;
use std::time::Duration;

const SPOTIFY_API_BASE: &str = "https://api.spotify.com";
const DEFAULT_TIMEOUT_MS: u64 = 60_000;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SpotifyActionInput {
    resource: String,
    action: String,
    #[serde(default, alias = "context_uri")]
    context_uri: Option<String>,
    #[serde(default)]
    uris: Option<Value>,
    #[serde(default)]
    uri: Option<String>,
    #[serde(default, alias = "volume_percent")]
    volume_percent: Option<u8>,
    #[serde(default, alias = "device_id")]
    device_id: Option<String>,
    #[serde(default)]
    play: Option<bool>,
    #[serde(default, alias = "playlist_id")]
    playlist_id: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    public: Option<bool>,
    #[serde(default)]
    collaborative: Option<bool>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    limit: Option<u32>,
}

#[derive(Debug, PartialEq, Eq)]
struct SpotifyApiRequest {
    method: Method,
    path: String,
    query: Vec<(String, String)>,
    body: Option<Value>,
}

/// Executes one strongly declared Spotify Web API action for verified Lambda skills.
pub fn execute_spotify_action(_state: &mut AppState, _cwd: &Path, input: Value) -> Result<String> {
    let parsed: SpotifyActionInput =
        serde_json::from_value(input).context("invalid SpotifyAction input")?;
    let token = spotify_token()?;
    let client = Client::builder()
        .timeout(Duration::from_millis(DEFAULT_TIMEOUT_MS))
        .build()
        .context("build Spotify HTTP client")?;
    let current_user = if playlist_create_needs_user(&parsed) {
        Some(fetch_current_user_id(&client, &token)?)
    } else {
        None
    };
    let request = spotify_api_request(&parsed, current_user.as_deref())?;
    send_spotify_request(&client, &token, request)
}

fn playlist_create_needs_user(input: &SpotifyActionInput) -> bool {
    input.resource.trim() == "playlists" && input.action.trim() == "create"
}

fn spotify_api_request(
    input: &SpotifyActionInput,
    current_user_id: Option<&str>,
) -> Result<SpotifyApiRequest> {
    let resource = input.resource.trim();
    let action = input.action.trim();
    match (resource, action) {
        ("playback", "play") => playback_play_request(input),
        ("playback", "pause") => simple_request(Method::PUT, "/v1/me/player/pause"),
        ("playback", "next") => simple_request(Method::POST, "/v1/me/player/next"),
        ("playback", "previous") => simple_request(Method::POST, "/v1/me/player/previous"),
        ("playback", "set_volume") => volume_request(input),
        ("devices", "transfer") => transfer_request(input),
        ("queue", "add") => queue_add_request(input),
        ("playlists", "create") => playlist_create_request(input, current_user_id),
        ("playlists", "add_items") => playlist_add_items_request(input),
        ("library", _) => library_request(input),
        _ => bail!("unsupported SpotifyAction `{resource}.{action}`"),
    }
}

fn simple_request(method: Method, path: &str) -> Result<SpotifyApiRequest> {
    Ok(SpotifyApiRequest {
        method,
        path: path.to_string(),
        query: Vec::new(),
        body: None,
    })
}

fn playback_play_request(input: &SpotifyActionInput) -> Result<SpotifyApiRequest> {
    let uris = parse_spotify_uri_list(input.uris.as_ref(), input.uri.as_deref(), Some("track"))?;
    let context_uri = input
        .context_uri
        .as_deref()
        .map(|value| spotify_uri(value, None))
        .transpose()?;
    let body = if !uris.is_empty() && uris.iter().all(|uri| uri.starts_with("spotify:track:")) {
        json!({ "uris": uris })
    } else if let Some(context_uri) = context_uri {
        json!({ "context_uri": context_uri })
    } else if !uris.is_empty() {
        json!({ "uris": uris })
    } else {
        json!({})
    };
    Ok(SpotifyApiRequest {
        method: Method::PUT,
        path: "/v1/me/player/play".to_string(),
        query: Vec::new(),
        body: Some(body),
    })
}

fn volume_request(input: &SpotifyActionInput) -> Result<SpotifyApiRequest> {
    let volume = input
        .volume_percent
        .filter(|value| *value <= 100)
        .context("SpotifyAction volume_percent must be between 0 and 100")?;
    Ok(SpotifyApiRequest {
        method: Method::PUT,
        path: "/v1/me/player/volume".to_string(),
        query: vec![("volume_percent".to_string(), volume.to_string())],
        body: None,
    })
}

fn transfer_request(input: &SpotifyActionInput) -> Result<SpotifyApiRequest> {
    let device_id = required(input.device_id.as_deref(), "device_id")?;
    Ok(SpotifyApiRequest {
        method: Method::PUT,
        path: "/v1/me/player".to_string(),
        query: Vec::new(),
        body: Some(json!({
            "device_ids": [device_id],
            "play": input.play.unwrap_or(false),
        })),
    })
}

fn queue_add_request(input: &SpotifyActionInput) -> Result<SpotifyApiRequest> {
    let raw_uri = input
        .uri
        .as_deref()
        .or_else(|| string_value(input.uris.as_ref()))
        .context("SpotifyAction uri is required")?;
    Ok(SpotifyApiRequest {
        method: Method::POST,
        path: "/v1/me/player/queue".to_string(),
        query: vec![("uri".to_string(), spotify_uri(raw_uri, Some("track"))?)],
        body: None,
    })
}

fn playlist_create_request(
    input: &SpotifyActionInput,
    current_user_id: Option<&str>,
) -> Result<SpotifyApiRequest> {
    let user_id = required(current_user_id, "current_user_id")?;
    let name = required(input.name.as_deref(), "name")?;
    Ok(SpotifyApiRequest {
        method: Method::POST,
        path: format!("/v1/users/{user_id}/playlists"),
        query: Vec::new(),
        body: Some(json!({
            "name": name,
            "public": input.public.unwrap_or(false),
            "collaborative": input.collaborative.unwrap_or(false),
            "description": input.description.clone().unwrap_or_default(),
        })),
    })
}

fn playlist_add_items_request(input: &SpotifyActionInput) -> Result<SpotifyApiRequest> {
    let playlist_id = spotify_id(
        required(input.playlist_id.as_deref(), "playlist_id")?,
        "playlist",
    )?;
    let uris = parse_spotify_uri_list(input.uris.as_ref(), input.uri.as_deref(), Some("track"))?;
    if uris.is_empty() {
        bail!("SpotifyAction uris must contain at least one track URI");
    }
    Ok(SpotifyApiRequest {
        method: Method::POST,
        path: format!("/v1/playlists/{playlist_id}/tracks"),
        query: Vec::new(),
        body: Some(json!({ "uris": uris })),
    })
}

fn library_request(input: &SpotifyActionInput) -> Result<SpotifyApiRequest> {
    let kind = required(input.kind.as_deref(), "kind")?;
    let entity = match kind {
        "tracks" => "track",
        "albums" => "album",
        _ => bail!("SpotifyAction library kind must be tracks or albums"),
    };
    let path = format!("/v1/me/{kind}");
    match input.action.trim() {
        "list" => Ok(SpotifyApiRequest {
            method: Method::GET,
            path,
            query: vec![(
                "limit".to_string(),
                input.limit.unwrap_or(20).clamp(1, 50).to_string(),
            )],
            body: None,
        }),
        "save" | "remove" | "contains" => {
            let ids = parse_spotify_id_list(input.uris.as_ref(), input.uri.as_deref(), entity)?;
            if ids.is_empty() {
                bail!("SpotifyAction uris must contain at least one {entity} id");
            }
            let method = match input.action.trim() {
                "save" => Method::PUT,
                "remove" => Method::DELETE,
                "contains" => Method::GET,
                _ => unreachable!(),
            };
            Ok(SpotifyApiRequest {
                method,
                path: if input.action.trim() == "contains" {
                    format!("{path}/contains")
                } else {
                    path
                },
                query: vec![("ids".to_string(), ids.join(","))],
                body: None,
            })
        }
        other => bail!("unsupported SpotifyAction library action `{other}`"),
    }
}

fn required<'a>(value: Option<&'a str>, name: &str) -> Result<&'a str> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .with_context(|| format!("SpotifyAction `{name}` is required"))
}

fn parse_spotify_uri_list(
    value: Option<&Value>,
    fallback: Option<&str>,
    default_kind: Option<&str>,
) -> Result<Vec<String>> {
    parse_string_list(value, fallback)?
        .into_iter()
        .map(|item| spotify_uri(&item, default_kind))
        .collect()
}

fn parse_spotify_id_list(
    value: Option<&Value>,
    fallback: Option<&str>,
    expected_kind: &str,
) -> Result<Vec<String>> {
    parse_string_list(value, fallback)?
        .into_iter()
        .map(|item| spotify_id(&item, expected_kind))
        .collect()
}

fn parse_string_list(value: Option<&Value>, fallback: Option<&str>) -> Result<Vec<String>> {
    let Some(value) = value else {
        return Ok(fallback
            .map(|item| vec![item.trim().to_string()])
            .unwrap_or_default());
    };
    match value {
        Value::Array(items) => items
            .iter()
            .map(|item| {
                item.as_str()
                    .map(str::trim)
                    .filter(|item| !item.is_empty())
                    .map(ToString::to_string)
                    .context("SpotifyAction URI arrays must contain strings")
            })
            .collect(),
        Value::String(text) => parse_string_or_json_list(text),
        Value::Null => Ok(Vec::new()),
        _ => bail!("SpotifyAction uris must be a string or string array"),
    }
}

fn parse_string_or_json_list(text: &str) -> Result<Vec<String>> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }
    if let Ok(value) = serde_json::from_str::<Value>(trimmed) {
        return parse_string_list(Some(&value), None);
    }
    Ok(trimmed
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(ToString::to_string)
        .collect())
}

fn string_value(value: Option<&Value>) -> Option<&str> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn spotify_uri(raw: &str, default_kind: Option<&str>) -> Result<String> {
    let text = raw.trim();
    if text.starts_with("spotify:") {
        return Ok(text.to_string());
    }
    if let Some((kind, id)) = spotify_url_kind_and_id(text) {
        return Ok(format!("spotify:{kind}:{id}"));
    }
    let Some(kind) = default_kind else {
        bail!("Spotify URI `{text}` must be a spotify URI or open.spotify.com URL");
    };
    Ok(format!("spotify:{kind}:{text}"))
}

fn spotify_id(raw: &str, expected_kind: &str) -> Result<String> {
    let text = raw.trim();
    if let Some(rest) = text.strip_prefix("spotify:") {
        let mut parts = rest.split(':');
        let kind = parts.next().unwrap_or_default();
        let id = parts.next().unwrap_or_default();
        if kind != expected_kind || id.is_empty() {
            bail!("Spotify URI `{text}` is not a {expected_kind} URI");
        }
        return Ok(id.to_string());
    }
    if let Some((kind, id)) = spotify_url_kind_and_id(text) {
        if kind != expected_kind {
            bail!("Spotify URL `{text}` is not a {expected_kind} URL");
        }
        return Ok(id);
    }
    if text.is_empty() {
        bail!("Spotify id is empty");
    }
    Ok(text.to_string())
}

fn spotify_url_kind_and_id(text: &str) -> Option<(String, String)> {
    let marker = "open.spotify.com/";
    let index = text.find(marker)?;
    let tail = &text[index + marker.len()..];
    let mut parts = tail.split('/');
    let kind = parts.next()?.trim();
    let id = parts
        .next()?
        .split(['?', '#'])
        .next()
        .unwrap_or_default()
        .trim();
    if kind.is_empty() || id.is_empty() {
        return None;
    }
    Some((kind.to_string(), id.to_string()))
}

fn spotify_token() -> Result<String> {
    std::env::var("PUFFER_SPOTIFY_ACCESS_TOKEN")
        .or_else(|_| std::env::var("SPOTIFY_ACCESS_TOKEN"))
        .map(|value| value.trim().to_string())
        .ok()
        .filter(|value| !value.is_empty())
        .context("SpotifyAction requires PUFFER_SPOTIFY_ACCESS_TOKEN or SPOTIFY_ACCESS_TOKEN")
}

fn fetch_current_user_id(client: &Client, token: &str) -> Result<String> {
    let response = client
        .get(format!("{SPOTIFY_API_BASE}/v1/me"))
        .bearer_auth(token)
        .send()
        .context("send Spotify current-user request")?;
    let status = response.status();
    let text = response
        .text()
        .context("read Spotify current-user response")?;
    if !status.is_success() {
        bail!(
            "Spotify current-user request failed with status {}: {}",
            status.as_u16(),
            text
        );
    }
    let value = serde_json::from_str::<Value>(&text).context("decode Spotify current user")?;
    value
        .get("id")
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .context("Spotify current-user response missing id")
}

fn send_spotify_request(
    client: &Client,
    token: &str,
    request: SpotifyApiRequest,
) -> Result<String> {
    let url = format!("{SPOTIFY_API_BASE}{}", request.path);
    let mut builder = client
        .request(request.method.clone(), url)
        .bearer_auth(token)
        .query(&request.query);
    if let Some(body) = request.body {
        builder = builder.json(&body);
    }
    let response = builder.send().context("send Spotify API request")?;
    let status = response.status();
    let text = response.text().context("read Spotify API response")?;
    if !status.is_success() {
        bail!(
            "Spotify API request {} failed with status {}: {}",
            request.path,
            status.as_u16(),
            text
        );
    }
    if text.trim().is_empty() {
        return Ok(serde_json::to_string_pretty(&json!({
            "ok": true,
            "status": status.as_u16(),
            "path": request.path,
        }))?);
    }
    let response = serde_json::from_str::<Value>(&text)
        .with_context(|| format!("Spotify API `{}` returned non-JSON response", request.path))?;
    Ok(serde_json::to_string_pretty(&json!({
        "ok": true,
        "status": status.as_u16(),
        "path": request.path,
        "response": response,
    }))?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn playback_play_prefers_track_uris() {
        let input = SpotifyActionInput {
            resource: "playback".to_string(),
            action: "play".to_string(),
            context_uri: Some("spotify:album:album1".to_string()),
            uris: Some(json!(["spotify:track:track1"])),
            uri: None,
            volume_percent: None,
            device_id: None,
            play: None,
            playlist_id: None,
            name: None,
            public: None,
            collaborative: None,
            description: None,
            kind: None,
            limit: None,
        };

        let request = spotify_api_request(&input, None).unwrap();

        assert_eq!(request.method, Method::PUT);
        assert_eq!(request.path, "/v1/me/player/play");
        assert_eq!(
            request.body,
            Some(json!({"uris": ["spotify:track:track1"]}))
        );
    }

    #[test]
    fn transfer_targets_declared_device() {
        let input = SpotifyActionInput {
            resource: "devices".to_string(),
            action: "transfer".to_string(),
            context_uri: None,
            uris: None,
            uri: None,
            volume_percent: None,
            device_id: Some("dev1".to_string()),
            play: Some(true),
            playlist_id: None,
            name: None,
            public: None,
            collaborative: None,
            description: None,
            kind: None,
            limit: None,
        };

        let request = spotify_api_request(&input, None).unwrap();

        assert_eq!(request.method, Method::PUT);
        assert_eq!(request.path, "/v1/me/player");
        assert_eq!(
            request.body,
            Some(json!({"device_ids": ["dev1"], "play": true}))
        );
    }

    #[test]
    fn library_save_normalizes_track_ids() {
        let input = SpotifyActionInput {
            resource: "library".to_string(),
            action: "save".to_string(),
            context_uri: None,
            uris: Some(json!("spotify:track:t1,https://open.spotify.com/track/t2")),
            uri: None,
            volume_percent: None,
            device_id: None,
            play: None,
            playlist_id: None,
            name: None,
            public: None,
            collaborative: None,
            description: None,
            kind: Some("tracks".to_string()),
            limit: None,
        };

        let request = spotify_api_request(&input, None).unwrap();

        assert_eq!(request.method, Method::PUT);
        assert_eq!(request.path, "/v1/me/tracks");
        assert_eq!(
            request.query,
            vec![("ids".to_string(), "t1,t2".to_string())]
        );
    }
}
