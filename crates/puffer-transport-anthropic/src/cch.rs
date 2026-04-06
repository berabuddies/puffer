use anyhow::Result;
use serde_json::Value;
use xxhash_rust::xxh64::xxh64;

const BILLING_HEADER_PREFIX: &str = "x-anthropic-billing-header:";
const CCH_PLACEHOLDER: &str = "cch=00000";
const CCH_SEED: u64 = 0x6E52_736A_C806_831E;
const CCH_MASK: u64 = 0x000F_FFFF;

/// Replaces the billing-header CCH placeholder in an Anthropic JSON payload.
pub fn finalize_cch_payload(payload: &Value) -> Result<String> {
    if !payload_contains_billing_header_placeholder(payload) {
        return Ok(serde_json::to_string(payload)?);
    }
    let unsigned_body = serde_json::to_string(payload)?;
    let cch = compute_cch(&unsigned_body);
    let mut signed_payload = payload.clone();
    replace_billing_header_placeholder(&mut signed_payload, &cch);
    Ok(serde_json::to_string(&signed_payload)?)
}

fn compute_cch(body: &str) -> String {
    let hash = xxh64(body.as_bytes(), CCH_SEED);
    format!("{:05x}", hash & CCH_MASK)
}

fn payload_contains_billing_header_placeholder(payload: &Value) -> bool {
    payload
        .get("system")
        .and_then(Value::as_array)
        .map(|blocks| {
            blocks.iter().any(|block| {
                block
                    .get("text")
                    .and_then(Value::as_str)
                    .is_some_and(|text| {
                        text.starts_with(BILLING_HEADER_PREFIX) && text.contains(CCH_PLACEHOLDER)
                    })
            })
        })
        .unwrap_or(false)
}

fn replace_billing_header_placeholder(payload: &mut Value, cch: &str) {
    let Some(blocks) = payload.get_mut("system").and_then(Value::as_array_mut) else {
        return;
    };
    for block in blocks {
        let Some(Value::String(text)) = block.get_mut("text") else {
            continue;
        };
        if text.starts_with(BILLING_HEADER_PREFIX) && text.contains(CCH_PLACEHOLDER) {
            *text = text.replacen(CCH_PLACEHOLDER, &format!("cch={cch}"), 1);
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn finalize_cch_payload_replaces_billing_header_placeholder() {
        let payload = json!({
            "system": [{
                "type": "text",
                "text": "x-anthropic-billing-header: cc_version=1.2.3.abc; cc_entrypoint=cli; cch=00000;"
            }],
            "messages": [{
                "role": "user",
                "content": "hello world from puffer"
            }],
            "model": "claude-sonnet-4-5",
            "max_tokens": 1024
        });
        let signed = finalize_cch_payload(&payload).expect("signed body");
        assert!(signed.contains("cch="));
        assert!(!signed.contains("cch=00000"));
        assert_eq!(signed.matches("cch=").count(), 1);
    }

    #[test]
    fn finalize_cch_payload_does_not_touch_user_content_placeholder() {
        let payload = json!({
            "system": [{
                "type": "text",
                "text": "x-anthropic-billing-header: cc_version=1.2.3.abc; cc_entrypoint=cli; cch=00000;"
            }],
            "messages": [{
                "role": "user",
                "content": "literal cch=00000 in prompt"
            }],
            "model": "claude-sonnet-4-5",
            "max_tokens": 1024
        });
        let signed = finalize_cch_payload(&payload).expect("signed body");
        assert!(signed.contains("literal cch=00000 in prompt"));
        assert_eq!(signed.matches("cch=").count(), 2);
        assert_eq!(signed.matches("cch=00000").count(), 1);
    }

    #[test]
    fn finalize_cch_payload_is_noop_without_billing_header_placeholder() {
        let payload = json!({"model": "claude-sonnet-4-5"});
        assert_eq!(finalize_cch_payload(&payload).unwrap(), payload.to_string());
    }
}
