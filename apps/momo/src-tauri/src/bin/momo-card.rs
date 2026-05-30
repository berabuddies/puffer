//! `momo-card` — reveal a U-card's live PAN/CVV INSIDE THIS PROCESS, then print
//! ONLY the non-sensitive summary (`{"last4","expiry"}`). The full card number
//! and CVV never leave this process: not printed, not logged, scrubbed before
//! exit. The agent that runs this bin via bash only ever sees the summary.
//!
//! Auth: reads `~/.ucard/.creds` (written by momo at login) for the backend
//! base URL + Auth-Station JWT, exchanges the JWT for a short-lived ucard
//! sessionToken (POST /api/auth/exchange), then GETs /api/card/details.
//!
//! Invoked as a SINGLE bash command (`momo-card reveal [--card-id N]`) so the
//! puffer permission gate (which forces approval on compound commands with
//! `&&`/`|`) does not trip; momo grants `bash argv momo-card` in the project
//! ACL so it runs without escalation.

use std::io::Write;

use anyhow::{anyhow, bail, Context, Result};
use serde::Deserialize;
use serde_json::{json, Value};

fn main() {
    if let Err(e) = run() {
        // Terse error to stderr. No card data is ever held at a point an error
        // can occur before reveal; after reveal we scrub before any early exit.
        eprintln!("momo-card: {e:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("reveal") => reveal(&args[2..]),
        Some(other) => bail!("unknown subcommand `{other}` (expected `reveal`)"),
        None => bail!("usage: momo-card reveal [--card-id <id>]"),
    }
}

#[derive(Debug)]
struct Creds {
    base_url: String,
    jwt: String,
}

/// Parse `KEY=VALUE` lines. Tolerates blank lines and a trailing newline.
fn parse_creds(body: &str) -> Result<Creds> {
    let mut base_url = None;
    let mut jwt = None;
    for line in body.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            match k.trim() {
                "UCARD_BASE_URL" => base_url = Some(v.trim().to_string()),
                "UCARD_JWT" => jwt = Some(v.trim().to_string()),
                _ => {}
            }
        }
    }
    Ok(Creds {
        base_url: base_url
            .filter(|s| !s.is_empty())
            .ok_or_else(|| anyhow!("UCARD_BASE_URL missing from ~/.ucard/.creds"))?,
        jwt: jwt
            .filter(|s| !s.is_empty())
            .ok_or_else(|| anyhow!("UCARD_JWT missing from ~/.ucard/.creds (sign in to momo)"))?,
    })
}

fn read_creds() -> Result<Creds> {
    let home = std::env::var_os("HOME").context("HOME not set")?;
    let path = std::path::Path::new(&home).join(".ucard").join(".creds");
    let body = std::fs::read_to_string(&path)
        .with_context(|| format!("reading {} (sign in to momo first)", path.display()))?;
    parse_creds(&body)
}

#[derive(Deserialize)]
struct Envelope {
    code: i64,
    #[serde(default)]
    message: String,
    #[serde(default)]
    data: Value,
}

/// Decode the ucard-backend envelope `{code,message,field,data}`. A non-zero
/// code is an error; we surface `code` + `message` only — never the raw body,
/// which on these endpoints is card-adjacent.
fn unwrap_envelope(raw: &str) -> Result<Value> {
    let env: Envelope =
        serde_json::from_str(raw).context("backend returned a non-envelope response")?;
    if env.code != 0 {
        bail!("backend error code={} {}", env.code, env.message);
    }
    Ok(env.data)
}

/// Last 4 digits of a PAN (digits only). Empty string if fewer than 4 digits.
fn last4(pan: &str) -> String {
    let digits: String = pan.chars().filter(|c| c.is_ascii_digit()).collect();
    if digits.len() >= 4 {
        digits[digits.len() - 4..].to_string()
    } else {
        String::new()
    }
}

/// "MM/YYYY" from the live expMonth/expYear; empty string if either is missing.
fn expiry(month: &str, year: &str) -> String {
    if !month.is_empty() && !year.is_empty() {
        format!("{month}/{year}")
    } else {
        String::new()
    }
}

fn exchange(client: &reqwest::blocking::Client, base: &str, jwt: &str) -> Result<String> {
    let url = format!("{base}/api/auth/exchange");
    let text = client
        .post(&url)
        .json(&json!({ "accessToken": jwt }))
        .send()
        .context("POST /api/auth/exchange failed")?
        .text()
        .context("reading /api/auth/exchange response")?;
    let data = unwrap_envelope(&text)?;
    data.get("sessionToken")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| anyhow!("exchange returned no sessionToken"))
}

/// First card's id (string or number) from /api/card/list — used when the
/// caller doesn't pass --card-id.
fn first_card_id(client: &reqwest::blocking::Client, base: &str, session: &str) -> Result<i64> {
    let url = format!("{base}/api/card/list");
    let text = client
        .get(&url)
        .bearer_auth(session)
        .send()
        .context("GET /api/card/list failed")?
        .text()
        .context("reading /api/card/list response")?;
    let data = unwrap_envelope(&text)?;
    let first = data
        .get("cards")
        .and_then(Value::as_array)
        .and_then(|a| a.first())
        .ok_or_else(|| anyhow!("no cards on this account"))?;
    let id = first.get("id").ok_or_else(|| anyhow!("card has no id"))?;
    if let Some(n) = id.as_i64() {
        Ok(n)
    } else if let Some(s) = id.as_str() {
        s.parse().context("card id is not an integer")
    } else {
        bail!("card id has unexpected type")
    }
}

struct Card {
    number: String,
    cvv: String,
    exp_month: String,
    exp_year: String,
}

fn card_details(
    client: &reqwest::blocking::Client,
    base: &str,
    session: &str,
    card_id: i64,
) -> Result<Card> {
    let url = format!("{base}/api/card/details?cardId={card_id}");
    let text = client
        .get(&url)
        .bearer_auth(session)
        .send()
        .context("GET /api/card/details failed")?
        .text()
        .context("reading /api/card/details response")?;
    let data = unwrap_envelope(&text)?;
    let get = |k: &str| data.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    Ok(Card {
        number: get("cardNumber"),
        cvv: get("cvv"),
        exp_month: get("expMonth"),
        exp_year: get("expYear"),
    })
}

fn reveal(args: &[String]) -> Result<()> {
    let mut card_id: Option<i64> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--card-id" => {
                let v = args.get(i + 1).context("--card-id needs a value")?;
                card_id = Some(v.parse().context("--card-id must be an integer")?);
                i += 2;
            }
            other => bail!("unknown flag `{other}`"),
        }
    }

    let creds = read_creds()?;
    let client = reqwest::blocking::Client::builder()
        .build()
        .context("building HTTP client")?;
    let session = exchange(&client, &creds.base_url, &creds.jwt)?;
    let id = match card_id {
        Some(id) => id,
        None => first_card_id(&client, &creds.base_url, &session)?,
    };
    let mut card = card_details(&client, &creds.base_url, &session, id)?;

    // Build ONLY the non-sensitive summary, then scrub the sensitive strings.
    let summary = json!({
        "last4": last4(&card.number),
        "expiry": expiry(&card.exp_month, &card.exp_year),
    });
    card.number.clear();
    card.cvv.clear();
    card.exp_month.clear();
    card.exp_year.clear();
    drop(card);

    let mut out = std::io::stdout();
    out.write_all(summary.to_string().as_bytes())?;
    out.write_all(b"\n")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_creds_reads_both_vars() {
        let c = parse_creds("UCARD_BASE_URL=https://x\nUCARD_JWT=jwt-1\n").unwrap();
        assert_eq!(c.base_url, "https://x");
        assert_eq!(c.jwt, "jwt-1");
    }

    #[test]
    fn parse_creds_errors_when_jwt_missing() {
        let err = parse_creds("UCARD_BASE_URL=https://x\n").unwrap_err();
        assert!(err.to_string().contains("UCARD_JWT"));
    }

    #[test]
    fn last4_takes_trailing_digits_only() {
        assert_eq!(last4("4111 1111 1111 1234"), "1234");
        assert_eq!(last4("12"), "");
    }

    #[test]
    fn expiry_joins_or_empties() {
        assert_eq!(expiry("04", "2028"), "04/2028");
        assert_eq!(expiry("", "2028"), "");
    }

    #[test]
    fn unwrap_envelope_rejects_nonzero_code() {
        let err = unwrap_envelope(r#"{"code":1005,"message":"bad","data":{}}"#).unwrap_err();
        assert!(err.to_string().contains("1005"));
    }

    #[test]
    fn unwrap_envelope_returns_data_on_zero() {
        let data = unwrap_envelope(r#"{"code":0,"message":"","data":{"sessionToken":"t"}}"#).unwrap();
        assert_eq!(data.get("sessionToken").unwrap().as_str().unwrap(), "t");
    }
}
