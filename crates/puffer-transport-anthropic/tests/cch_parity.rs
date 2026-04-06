use indexmap::IndexMap;
use puffer_transport_anthropic::{
    build_messages_request, AnthropicAuth, AnthropicMessage, AnthropicModelRequest,
    AnthropicRequestConfig,
};
use serde_json::json;
use xxhash_rust::xxh64::xxh64;

const FREE_CODE_CCH_SEED: u64 = 0x6E52_736A_C806_831E;
const FREE_CODE_CCH_MASK: u64 = 0x000F_FFFF;

fn compute_free_code_cch(body: &str) -> String {
    let hash = xxh64(body.as_bytes(), FREE_CODE_CCH_SEED);
    format!("{:05x}", hash & FREE_CODE_CCH_MASK)
}

#[test]
fn temp_cch_parity_matches_free_code_reference_vector() {
    let body = json!({
        "model": "claude-sonnet-4-5",
        "max_tokens": 1024,
        "system": [{
            "type": "text",
            "text": "x-anthropic-billing-header: cc_version=1.2.3.abc; cc_entrypoint=cli; cch=00000;"
        }],
        "messages": [{
            "role": "user",
            "content": "hello world from puffer"
        }]
    })
    .to_string();

    assert_eq!(compute_free_code_cch(&body), "327b2");
}

#[test]
fn temp_cch_parity_covers_current_puffer_request_body_shape() {
    let request = build_messages_request(
        &AnthropicRequestConfig {
            base_url: "https://api.anthropic.com".to_string(),
            session_id: "session-1".to_string(),
            custom_headers: IndexMap::new(),
            remote_container_id: None,
            remote_session_id: None,
            client_app: None,
            entrypoint: "cli".to_string(),
            user_type: "external".to_string(),
            version: "1.2.3".to_string(),
            workload: None,
            additional_protection: false,
            cch_enabled: true,
            auth: AnthropicAuth::ApiKey("sk-ant-test".to_string()),
            beta_header: None,
            client_request_id: None,
        },
        &AnthropicModelRequest {
            model: "claude-sonnet-4-5".to_string(),
            max_tokens: 1024,
            messages: vec![AnthropicMessage {
                role: "user".to_string(),
                content: "hello world from puffer".to_string(),
            }],
        },
    )
    .expect("build request");

    let body = json!({
        "model": "claude-sonnet-4-5",
        "max_tokens": 1024,
        "messages": [{
            "role": "user",
            "content": "hello world from puffer"
        }],
        "system": [{
            "type": "text",
            "text": request.attribution_prefix_block
        }]
    })
    .to_string();

    let cch = compute_free_code_cch(&body);
    assert_eq!(cch.len(), 5);
    assert!(body.contains("cch=00000"));
}
