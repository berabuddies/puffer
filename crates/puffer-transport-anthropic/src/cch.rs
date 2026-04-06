use anyhow::Result;
use xxhash_rust::xxh64::xxh64;

const CCH_PLACEHOLDER: &str = "cch=00000";
const CCH_SEED: u64 = 0x6E52_736A_C806_831E;
const CCH_MASK: u64 = 0x000F_FFFF;

/// Replaces the Claude-style CCH placeholder in a request body with a calculated value.
pub fn finalize_cch_body(body: &str) -> Result<String> {
    if !body.contains(CCH_PLACEHOLDER) {
        return Ok(body.to_string());
    }
    let cch = compute_cch(body);
    Ok(body.replacen(CCH_PLACEHOLDER, &format!("cch={cch}"), 1))
}

fn compute_cch(body: &str) -> String {
    let hash = xxh64(body.as_bytes(), CCH_SEED);
    format!("{:05x}", hash & CCH_MASK)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finalize_cch_body_replaces_first_placeholder_with_calculated_value() {
        let body = r#"{"system":[{"type":"text","text":"x-anthropic-billing-header: cc_version=1.2.3.abc; cc_entrypoint=cli; cch=00000;"}],"messages":[{"role":"user","content":"hello world from puffer"}],"model":"claude-sonnet-4-5","max_tokens":1024}"#;
        let signed = finalize_cch_body(body).expect("signed body");
        assert!(signed.contains("cch="));
        assert!(!signed.contains("cch=00000"));
        assert_eq!(signed.matches("cch=").count(), 1);
    }

    #[test]
    fn finalize_cch_body_is_noop_without_placeholder() {
        let body = r#"{"model":"claude-sonnet-4-5"}"#;
        assert_eq!(finalize_cch_body(body).unwrap(), body);
    }
}
