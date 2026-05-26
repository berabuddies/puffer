use super::*;
use crate::runtime::lambda_gate::{LambdaGateState, LambdaHostEnv};

#[test]
fn lambda_host_call_binds_shopify_fulfillment_contract() {
    let mut state = temp_state();
    let cwd = state.cwd.clone();
    let host = LambdaHostEnv::from_json_str(
        r#"{"effects":["proc","net_w"],"domains":[],"tools":[{"name":"fulfillment_create","concreteTools":["ShopifyAction"],"concreteInputContracts":{"ShopifyAction":{"action":"fulfillmentCreate","orderId":{"$arg":"order_id"},"inputJson":{"$arg":"input_json"}}},"params":[{"name":"order_id","ty":"str"},{"name":"input_json","ty":"str"}],"result":"Result<FulfillmentResult{gql_success(r)}, ShopifyErr>","effects":["proc","net_w"]}]}"#,
    )
    .unwrap();
    state.lambda_gate = Some(LambdaGateState::with_host_caps(host));
    let resources = LoadedResources {
        tools: vec![
            loaded_tool(
                "LambdaHostCall",
                "Admit Lambda host call",
                "runtime:lambda_host_call",
            ),
            loaded_tool(
                "ShopifyAction",
                "Shopify action",
                "runtime:workflow:shopify_action",
            ),
        ],
        ..LoadedResources::default()
    };
    let registry = ToolRegistry::from_resources(&resources);
    let providers = empty_providers();
    let request_config = test_openai_request_config();
    let input_json = r#"{"fulfillment":{"lineItemsByFulfillmentOrder":[{"fulfillmentOrderId":"gid://shopify/FulfillmentOrder/100"}]}}"#;

    let admitted = execute_tool_call(
        &mut state,
        &resources,
        &providers,
        &mut AuthStore::default(),
        &registry,
        "gpt-5",
        &cwd,
        ToolExecutionBackend::OpenAi {
            request_config: &request_config,
            structured_output: None,
        },
        None,
        "LambdaHostCall",
        json!({
            "host_tool": "fulfillment_create",
            "args": {
                "order_id": "gid://shopify/Order/1",
                "input_json": input_json
            },
            "tool": "ShopifyAction",
            "input": {
                "action": "fulfillmentCreate",
                "orderId": "gid://shopify/Order/1",
                "inputJson": input_json
            },
        }),
    )
    .unwrap();

    assert!(admitted.success);
    let pending = state.pending_lambda_host_call.as_ref().unwrap();
    assert_eq!(pending.concrete_tool(), "ShopifyAction");
}

#[test]
fn lambda_host_call_binds_shopify_inventory_adjust_contract() {
    let mut state = temp_state();
    let cwd = state.cwd.clone();
    let host = LambdaHostEnv::from_json_str(
        r#"{"effects":["proc","net_w"],"domains":[],"tools":[{"name":"inventory_adjust","concreteTools":["ShopifyAction"],"concreteInputContracts":{"ShopifyAction":{"action":"inventoryAdjustSingleLocation","itemId":{"$arg":"item_id"},"delta":{"$int_arg":"delta"}}},"params":[{"name":"item_id","ty":"str"},{"name":"delta","ty":"int"}],"result":"Result<GqlResponse{gql_success(r)}, ShopifyErr>","effects":["proc","net_w"]}]}"#,
    )
    .unwrap();
    state.lambda_gate = Some(LambdaGateState::with_host_caps(host));
    let resources = LoadedResources {
        tools: vec![
            loaded_tool(
                "LambdaHostCall",
                "Admit Lambda host call",
                "runtime:lambda_host_call",
            ),
            loaded_tool(
                "ShopifyAction",
                "Shopify action",
                "runtime:workflow:shopify_action",
            ),
        ],
        ..LoadedResources::default()
    };
    let registry = ToolRegistry::from_resources(&resources);
    let providers = empty_providers();
    let request_config = test_openai_request_config();

    let admitted = execute_tool_call(
        &mut state,
        &resources,
        &providers,
        &mut AuthStore::default(),
        &registry,
        "gpt-5",
        &cwd,
        ToolExecutionBackend::OpenAi {
            request_config: &request_config,
            structured_output: None,
        },
        None,
        "LambdaHostCall",
        json!({
            "host_tool": "inventory_adjust",
            "args": {
                "item_id": "gid://shopify/InventoryItem/100",
                "delta": -3
            },
            "tool": "ShopifyAction",
            "input": {
                "action": "inventoryAdjustSingleLocation",
                "itemId": "gid://shopify/InventoryItem/100",
                "delta": -3
            },
        }),
    )
    .unwrap();

    assert!(admitted.success);
    let pending = state.pending_lambda_host_call.as_ref().unwrap();
    assert_eq!(pending.concrete_tool(), "ShopifyAction");
}
