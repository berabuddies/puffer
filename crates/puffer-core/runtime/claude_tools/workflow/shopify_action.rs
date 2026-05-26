use crate::AppState;
use anyhow::{bail, Context, Result};
use reqwest::blocking::Client;
use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::collections::BTreeSet;
use std::path::Path;
use std::time::Duration;

const DEFAULT_API_VERSION: &str = "2026-01";
const DEFAULT_TIMEOUT_MS: u64 = 60_000;
const FULFILLMENT_ORDERS_QUERY: &str =
    "query($id: ID!) { order(id: $id) { id fulfillmentOrders(first: 100) { nodes { id } } } }";
const FULFILLMENT_CREATE_MUTATION: &str = "mutation($fulfillment: FulfillmentInput!, $message: String) { fulfillmentCreate(fulfillment: $fulfillment, message: $message) { fulfillment { id status } userErrors { field message } } }";
const INVENTORY_LEVELS_QUERY: &str = "query($id: ID!) { inventoryItem(id: $id) { id inventoryLevels(first: 2) { nodes { location { id } } } } }";
const INVENTORY_ADJUST_MUTATION: &str = "mutation($input: InventoryAdjustQuantitiesInput!) { inventoryAdjustQuantities(input: $input) { inventoryAdjustmentGroup { id reason changes { name delta } } userErrors { field message } } }";

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ShopifyActionInput {
    action: String,
    #[serde(default, alias = "order_id")]
    order_id: Option<String>,
    #[serde(default, alias = "item_id")]
    item_id: Option<String>,
    #[serde(default)]
    delta: Option<i64>,
    #[serde(default, alias = "input_json")]
    input_json: Option<Value>,
}

#[derive(Debug, PartialEq, Eq)]
struct ShopifyConfig {
    endpoint: String,
    token: String,
}

/// Executes one strongly declared Shopify Admin API action for verified Lambda skills.
pub fn execute_shopify_action(_state: &mut AppState, _cwd: &Path, input: Value) -> Result<String> {
    let parsed: ShopifyActionInput =
        serde_json::from_value(input).context("invalid ShopifyAction input")?;
    match parsed.action.trim() {
        "fulfillmentCreate" => execute_fulfillment_create(parsed),
        "inventoryAdjustSingleLocation" => execute_inventory_adjust_single_location(parsed),
        other => bail!("unsupported ShopifyAction action `{other}`"),
    }
}

fn execute_fulfillment_create(input: ShopifyActionInput) -> Result<String> {
    let order_id = required(input.order_id, "orderId")?;
    let variables = fulfillment_variables(
        input
            .input_json
            .context("ShopifyAction inputJson is required")?,
    )?;
    let requested_fulfillment_ids = fulfillment_order_ids(&variables)?;
    let config = shopify_config_from_env()?;
    let client = Client::builder()
        .timeout(Duration::from_millis(DEFAULT_TIMEOUT_MS))
        .build()
        .context("build Shopify HTTP client")?;

    let order_response = send_shopify_graphql(
        &client,
        &config,
        json!({
            "query": FULFILLMENT_ORDERS_QUERY,
            "variables": { "id": order_id }
        }),
    )?;
    let allowed_fulfillment_ids = extract_order_fulfillment_ids(&order_response)?;
    ensure_ids_belong_to_order(&requested_fulfillment_ids, &allowed_fulfillment_ids)?;

    let mutation_response = send_shopify_graphql(
        &client,
        &config,
        json!({
            "query": FULFILLMENT_CREATE_MUTATION,
            "variables": variables
        }),
    )?;
    validate_fulfillment_create_response(&mutation_response)?;
    Ok(serde_json::to_string_pretty(&json!({
        "orderId": order_id,
        "response": mutation_response
    }))?)
}

fn execute_inventory_adjust_single_location(input: ShopifyActionInput) -> Result<String> {
    let item_id = required(input.item_id, "itemId")?;
    ensure_shopify_gid(&item_id, "InventoryItem", "itemId")?;
    let delta = input.delta.context("ShopifyAction `delta` is required")?;
    let config = shopify_config_from_env()?;
    let client = Client::builder()
        .timeout(Duration::from_millis(DEFAULT_TIMEOUT_MS))
        .build()
        .context("build Shopify HTTP client")?;

    let levels_response = send_shopify_graphql(
        &client,
        &config,
        json!({
            "query": INVENTORY_LEVELS_QUERY,
            "variables": { "id": item_id }
        }),
    )?;
    let location_id = extract_single_inventory_location_id(&levels_response)?;
    let variables = inventory_adjust_variables(&item_id, &location_id, delta);
    let mutation_response = send_shopify_graphql(
        &client,
        &config,
        json!({
            "query": INVENTORY_ADJUST_MUTATION,
            "variables": variables
        }),
    )?;
    validate_inventory_adjust_response(&mutation_response)?;
    Ok(serde_json::to_string_pretty(&json!({
        "itemId": item_id,
        "locationId": location_id,
        "delta": delta,
        "response": mutation_response
    }))?)
}

fn required(value: Option<String>, name: &str) -> Result<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .with_context(|| format!("ShopifyAction `{name}` is required"))
}

fn fulfillment_variables(input_json: Value) -> Result<Value> {
    let value = match input_json {
        Value::String(text) => serde_json::from_str::<Value>(text.trim())
            .context("ShopifyAction inputJson must be JSON")?,
        other => other,
    };
    let object = value
        .as_object()
        .context("ShopifyAction inputJson must be a JSON object")?;
    let mut variables = Map::new();
    for (key, value) in object {
        match key.as_str() {
            "fulfillment" => {
                if !value.is_object() {
                    bail!("ShopifyAction fulfillment must be a JSON object");
                }
                variables.insert(key.clone(), value.clone());
            }
            "message" => {
                if !value.is_string() && !value.is_null() {
                    bail!("ShopifyAction message must be a string when present");
                }
                variables.insert(key.clone(), value.clone());
            }
            other => bail!("ShopifyAction inputJson contains unsupported field `{other}`"),
        }
    }
    if !variables.contains_key("fulfillment") {
        bail!("ShopifyAction inputJson requires fulfillment");
    }
    if !variables.contains_key("message") {
        variables.insert("message".to_string(), Value::Null);
    }
    Ok(Value::Object(variables))
}

fn inventory_adjust_variables(item_id: &str, location_id: &str, delta: i64) -> Value {
    json!({
        "input": {
            "reason": "correction",
            "name": "available",
            "changes": [{
                "delta": delta,
                "inventoryItemId": item_id,
                "locationId": location_id
            }]
        }
    })
}

fn fulfillment_order_ids(variables: &Value) -> Result<BTreeSet<String>> {
    let items = variables
        .get("fulfillment")
        .and_then(|value| value.get("lineItemsByFulfillmentOrder"))
        .and_then(Value::as_array)
        .context("ShopifyAction fulfillment.lineItemsByFulfillmentOrder is required")?;
    if items.is_empty() {
        bail!("ShopifyAction lineItemsByFulfillmentOrder must not be empty");
    }
    let mut ids = BTreeSet::new();
    for item in items {
        let id = item
            .get("fulfillmentOrderId")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .context("ShopifyAction fulfillmentOrderId is required")?;
        if !id.starts_with("gid://shopify/FulfillmentOrder/") {
            bail!("ShopifyAction fulfillmentOrderId must be a Shopify FulfillmentOrder gid");
        }
        ids.insert(id.to_string());
    }
    Ok(ids)
}

fn extract_order_fulfillment_ids(response: &Value) -> Result<BTreeSet<String>> {
    fail_on_graphql_errors(response, "fulfillmentOrders query")?;
    let nodes = response
        .pointer("/data/order/fulfillmentOrders/nodes")
        .and_then(Value::as_array)
        .context("ShopifyAction order response did not include fulfillmentOrders")?;
    let ids = nodes
        .iter()
        .filter_map(|node| node.get("id").and_then(Value::as_str))
        .map(ToString::to_string)
        .collect::<BTreeSet<_>>();
    if ids.is_empty() {
        bail!("ShopifyAction order has no fulfillment orders");
    }
    Ok(ids)
}

fn extract_single_inventory_location_id(response: &Value) -> Result<String> {
    fail_on_graphql_errors(response, "inventoryItem inventoryLevels query")?;
    let nodes = response
        .pointer("/data/inventoryItem/inventoryLevels/nodes")
        .and_then(Value::as_array)
        .context("ShopifyAction inventory item response did not include inventoryLevels")?;
    if nodes.len() != 1 {
        bail!(
            "ShopifyAction inventoryAdjustSingleLocation requires exactly one inventory level, found {}",
            nodes.len()
        );
    }
    let location_id = nodes[0]
        .pointer("/location/id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .context("ShopifyAction inventory level did not include a location id")?;
    ensure_shopify_gid(location_id, "Location", "locationId")?;
    Ok(location_id.to_string())
}

fn ensure_ids_belong_to_order(
    requested: &BTreeSet<String>,
    allowed: &BTreeSet<String>,
) -> Result<()> {
    let missing = requested
        .difference(allowed)
        .cloned()
        .collect::<Vec<String>>();
    if !missing.is_empty() {
        bail!(
            "ShopifyAction fulfillmentOrderId does not belong to order: {}",
            missing.join(", ")
        );
    }
    Ok(())
}

fn validate_inventory_adjust_response(response: &Value) -> Result<()> {
    fail_on_graphql_errors(response, "inventoryAdjustQuantities mutation")?;
    let user_errors = response
        .pointer("/data/inventoryAdjustQuantities/userErrors")
        .and_then(Value::as_array)
        .context("ShopifyAction inventoryAdjustQuantities response did not include userErrors")?;
    if !user_errors.is_empty() {
        bail!(
            "ShopifyAction inventoryAdjustQuantities returned userErrors: {}",
            serde_json::to_string(user_errors)?
        );
    }
    if response
        .pointer("/data/inventoryAdjustQuantities/inventoryAdjustmentGroup")
        .is_none()
    {
        bail!("ShopifyAction inventoryAdjustQuantities response did not include adjustment group");
    }
    Ok(())
}

fn validate_fulfillment_create_response(response: &Value) -> Result<()> {
    fail_on_graphql_errors(response, "fulfillmentCreate mutation")?;
    let user_errors = response
        .pointer("/data/fulfillmentCreate/userErrors")
        .and_then(Value::as_array)
        .context("ShopifyAction fulfillmentCreate response did not include userErrors")?;
    if !user_errors.is_empty() {
        bail!(
            "ShopifyAction fulfillmentCreate returned userErrors: {}",
            serde_json::to_string(user_errors)?
        );
    }
    if response
        .pointer("/data/fulfillmentCreate/fulfillment")
        .is_none()
    {
        bail!("ShopifyAction fulfillmentCreate response did not include fulfillment");
    }
    Ok(())
}

fn fail_on_graphql_errors(response: &Value, operation: &str) -> Result<()> {
    if let Some(errors) = response.get("errors").filter(|value| !value.is_null()) {
        bail!(
            "ShopifyAction {operation} returned GraphQL errors: {}",
            serde_json::to_string(errors)?
        );
    }
    Ok(())
}

fn ensure_shopify_gid(value: &str, resource: &str, name: &str) -> Result<()> {
    if value.starts_with(&format!("gid://shopify/{resource}/")) {
        return Ok(());
    }
    bail!("ShopifyAction `{name}` must be a Shopify {resource} gid")
}

fn send_shopify_graphql(client: &Client, config: &ShopifyConfig, payload: Value) -> Result<Value> {
    let response = client
        .post(&config.endpoint)
        .header("Content-Type", "application/json")
        .header("X-Shopify-Access-Token", &config.token)
        .json(&payload)
        .send()
        .context("send Shopify GraphQL request")?;
    let status = response.status();
    let text = response.text().context("read Shopify GraphQL response")?;
    if !status.is_success() {
        bail!("ShopifyAction GraphQL request failed with status {status}: {text}");
    }
    serde_json::from_str(&text).context("ShopifyAction GraphQL response was not JSON")
}

fn shopify_config_from_env() -> Result<ShopifyConfig> {
    let domain = std::env::var("SHOPIFY_STORE_DOMAIN")
        .map(|value| value.trim().to_string())
        .ok()
        .filter(|value| !value.is_empty())
        .context("ShopifyAction requires SHOPIFY_STORE_DOMAIN")?;
    let token = std::env::var("SHOPIFY_ACCESS_TOKEN")
        .map(|value| value.trim().to_string())
        .ok()
        .filter(|value| !value.is_empty())
        .context("ShopifyAction requires SHOPIFY_ACCESS_TOKEN")?;
    let version = std::env::var("SHOPIFY_API_VERSION")
        .map(|value| value.trim().to_string())
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| DEFAULT_API_VERSION.to_string());
    let domain = normalize_shopify_domain(&domain)?;
    if !version
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
    {
        bail!("ShopifyAction SHOPIFY_API_VERSION contains unsupported characters");
    }
    Ok(ShopifyConfig {
        endpoint: format!("https://{domain}/admin/api/{version}/graphql.json"),
        token,
    })
}

fn normalize_shopify_domain(domain: &str) -> Result<String> {
    let normalized = domain
        .trim()
        .strip_prefix("https://")
        .or_else(|| domain.trim().strip_prefix("http://"))
        .unwrap_or(domain.trim())
        .trim_end_matches('/');
    if normalized.is_empty()
        || normalized.contains('/')
        || normalized.chars().any(char::is_whitespace)
    {
        bail!("ShopifyAction SHOPIFY_STORE_DOMAIN must be a bare domain");
    }
    Ok(normalized.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_fulfillment_variables_from_json_string() {
        let variables = fulfillment_variables(json!(
            r#"{
            "fulfillment": {
                "lineItemsByFulfillmentOrder": [
                    {"fulfillmentOrderId": "gid://shopify/FulfillmentOrder/100"}
                ],
                "notifyCustomer": true
            },
            "message": "packed"
        }"#
        ))
        .unwrap();

        assert_eq!(variables["message"], "packed");
        let ids = fulfillment_order_ids(&variables).unwrap();
        assert!(ids.contains("gid://shopify/FulfillmentOrder/100"));
    }

    #[test]
    fn rejects_unscoped_fulfillment_order_id() {
        let requested = BTreeSet::from(["gid://shopify/FulfillmentOrder/200".to_string()]);
        let allowed = BTreeSet::from(["gid://shopify/FulfillmentOrder/100".to_string()]);
        let error = ensure_ids_belong_to_order(&requested, &allowed)
            .expect_err("foreign fulfillment order ids must fail closed");

        assert!(error.to_string().contains("does not belong"));
    }

    #[test]
    fn builds_inventory_adjust_variables() {
        let variables = inventory_adjust_variables(
            "gid://shopify/InventoryItem/100",
            "gid://shopify/Location/200",
            -3,
        );

        assert_eq!(
            variables,
            json!({
                "input": {
                    "reason": "correction",
                    "name": "available",
                    "changes": [{
                        "delta": -3,
                        "inventoryItemId": "gid://shopify/InventoryItem/100",
                        "locationId": "gid://shopify/Location/200"
                    }]
                }
            })
        );
    }

    #[test]
    fn extracts_single_inventory_location() {
        let location = extract_single_inventory_location_id(&json!({
            "data": {
                "inventoryItem": {
                    "inventoryLevels": {
                        "nodes": [
                            {"location": {"id": "gid://shopify/Location/200"}}
                        ]
                    }
                }
            }
        }))
        .unwrap();

        assert_eq!(location, "gid://shopify/Location/200");
    }

    #[test]
    fn rejects_ambiguous_inventory_locations() {
        let error = extract_single_inventory_location_id(&json!({
            "data": {
                "inventoryItem": {
                    "inventoryLevels": {
                        "nodes": [
                            {"location": {"id": "gid://shopify/Location/200"}},
                            {"location": {"id": "gid://shopify/Location/300"}}
                        ]
                    }
                }
            }
        }))
        .expect_err("multiple inventory levels must fail closed");

        assert!(error.to_string().contains("exactly one inventory level"));
    }

    #[test]
    fn rejects_inventory_adjust_user_errors() {
        let error = validate_inventory_adjust_response(&json!({
            "data": {
                "inventoryAdjustQuantities": {
                    "inventoryAdjustmentGroup": null,
                    "userErrors": [{"field": ["input"], "message": "bad"}]
                }
            }
        }))
        .expect_err("Shopify inventory userErrors must fail closed");

        assert!(error.to_string().contains("userErrors"));
    }

    #[test]
    fn rejects_user_errors_in_mutation_response() {
        let error = validate_fulfillment_create_response(&json!({
            "data": {
                "fulfillmentCreate": {
                    "fulfillment": null,
                    "userErrors": [{"field": ["fulfillment"], "message": "bad"}]
                }
            }
        }))
        .expect_err("Shopify userErrors must fail closed");

        assert!(error.to_string().contains("userErrors"));
    }

    #[test]
    fn normalizes_store_domain_without_accepting_paths() {
        assert_eq!(
            normalize_shopify_domain("https://example.myshopify.com/").unwrap(),
            "example.myshopify.com"
        );
        let error = normalize_shopify_domain("example.myshopify.com/admin")
            .expect_err("path-like domains must fail closed");
        assert!(error.to_string().contains("bare domain"));
    }
}
