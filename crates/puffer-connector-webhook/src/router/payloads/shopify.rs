use axum::http::HeaderMap;
use puffer_connector_core::InboundMessage;
use serde_json::Value;

use super::super::{header_value, number_or_string, snippet, string_field};

/// Converts a Shopify webhook payload into an inbound Puffer message.
pub(super) fn shopify_inbound(headers: &HeaderMap, payload: &Value) -> Option<InboundMessage> {
    if !shopify_payload_shape(headers, payload) {
        return None;
    }

    let topic = shopify_topic(headers, payload)?;
    let shop = shopify_shop(headers, payload);
    let subject = shopify_subject(topic, payload);
    let delivery = header_value(headers, "x-shopify-event-id")
        .or_else(|| header_value(headers, "x-shopify-webhook-id"))
        .or_else(|| string_field(payload, "event_id"))
        .or_else(|| string_field(payload, "webhook_id"));
    let conversation_id = shopify_conversation_id(&shop, topic, delivery, subject.as_ref());
    let text = shopify_message(&shop, topic, delivery, headers, payload, subject.as_ref());

    Some(InboundMessage {
        conversation_id,
        user_id: Some(shop),
        text,
        thread_id: None,
        is_group: false,
        bot_mentioned: true,
        from_bot: false,
    })
}

fn shopify_payload_shape(headers: &HeaderMap, payload: &Value) -> bool {
    header_value(headers, "x-shopify-topic").is_some()
        || header_value(headers, "x-shopify-shop-domain").is_some()
        || string_field(payload, "topic").is_some()
        || string_field(payload, "shop_domain").is_some()
}

fn shopify_topic<'a>(headers: &'a HeaderMap, payload: &'a Value) -> Option<&'a str> {
    header_value(headers, "x-shopify-topic").or_else(|| string_field(payload, "topic"))
}

fn shopify_shop(headers: &HeaderMap, payload: &Value) -> String {
    header_value(headers, "x-shopify-shop-domain")
        .or_else(|| string_field(payload, "shop_domain"))
        .or_else(|| string_field(payload, "shop"))
        .unwrap_or("shopify")
        .to_string()
}

#[derive(Clone)]
struct ShopifySubject {
    kind: &'static str,
    conversation_kind: &'static str,
    id: Option<String>,
    title: Option<String>,
    status: Option<String>,
    amount: Option<String>,
    url: Option<String>,
}

fn shopify_subject(topic: &str, payload: &Value) -> Option<ShopifySubject> {
    let normalized_topic = normalize_shopify_topic(topic);
    if normalized_topic.starts_with("orders/")
        || payload.get("order_number").is_some()
        || payload.get("total_price").is_some()
    {
        return Some(shopify_order_subject(payload));
    }
    if normalized_topic.starts_with("products/")
        || (payload.get("title").is_some() && payload.get("variants").is_some())
    {
        return Some(shopify_product_subject(payload));
    }
    if normalized_topic.starts_with("customers/")
        || payload.get("email").is_some()
        || payload.get("phone").is_some()
    {
        return Some(shopify_customer_subject(payload));
    }
    if normalized_topic.starts_with("inventory_levels/")
        || payload.get("inventory_item_id").is_some()
    {
        return Some(shopify_inventory_subject(payload));
    }
    if normalized_topic.starts_with("fulfillments/") || payload.get("tracking_number").is_some() {
        return Some(shopify_fulfillment_subject(payload));
    }
    payload.get("id").map(|_| ShopifySubject {
        kind: "resource",
        conversation_kind: "resource",
        id: payload.get("id").and_then(number_or_string),
        title: string_field(payload, "name")
            .or_else(|| string_field(payload, "title"))
            .map(str::to_string),
        status: string_field(payload, "status").map(str::to_string),
        amount: None,
        url: shopify_url(payload),
    })
}

fn shopify_order_subject(payload: &Value) -> ShopifySubject {
    ShopifySubject {
        kind: "order",
        conversation_kind: "order",
        id: payload.get("id").and_then(number_or_string),
        title: string_field(payload, "name")
            .or_else(|| string_field(payload, "order_number"))
            .map(str::to_string),
        status: string_field(payload, "financial_status")
            .or_else(|| string_field(payload, "fulfillment_status"))
            .or_else(|| string_field(payload, "cancel_reason"))
            .map(str::to_string),
        amount: shopify_amount(payload),
        url: shopify_url(payload),
    }
}

fn shopify_product_subject(payload: &Value) -> ShopifySubject {
    ShopifySubject {
        kind: "product",
        conversation_kind: "product",
        id: payload.get("id").and_then(number_or_string),
        title: string_field(payload, "title").map(str::to_string),
        status: string_field(payload, "status")
            .or_else(|| string_field(payload, "product_type"))
            .map(str::to_string),
        amount: None,
        url: shopify_url(payload),
    }
}

fn shopify_customer_subject(payload: &Value) -> ShopifySubject {
    ShopifySubject {
        kind: "customer",
        conversation_kind: "customer",
        id: payload.get("id").and_then(number_or_string),
        title: shopify_customer_label(payload),
        status: string_field(payload, "state")
            .or_else(|| string_field(payload, "email_marketing_consent"))
            .map(str::to_string),
        amount: None,
        url: shopify_url(payload),
    }
}

fn shopify_inventory_subject(payload: &Value) -> ShopifySubject {
    let item = payload
        .get("inventory_item_id")
        .and_then(number_or_string)
        .unwrap_or_else(|| "inventory".to_string());
    let location = payload.get("location_id").and_then(number_or_string);
    let title = location
        .as_ref()
        .map(|location| format!("item {item} at location {location}"))
        .unwrap_or_else(|| format!("item {item}"));
    ShopifySubject {
        kind: "inventory level",
        conversation_kind: "inventory",
        id: Some(
            location
                .map(|location| format!("{item}:{location}"))
                .unwrap_or(item),
        ),
        title: Some(title),
        status: payload.get("available").and_then(number_or_string),
        amount: None,
        url: shopify_url(payload),
    }
}

fn shopify_fulfillment_subject(payload: &Value) -> ShopifySubject {
    ShopifySubject {
        kind: "fulfillment",
        conversation_kind: "fulfillment",
        id: payload.get("id").and_then(number_or_string),
        title: payload
            .get("order_id")
            .and_then(number_or_string)
            .map(|order| format!("order {order}"))
            .or_else(|| string_field(payload, "tracking_number").map(str::to_string)),
        status: string_field(payload, "status")
            .or_else(|| string_field(payload, "shipment_status"))
            .map(str::to_string),
        amount: None,
        url: shopify_url(payload),
    }
}

fn shopify_customer_label(payload: &Value) -> Option<String> {
    let first = string_field(payload, "first_name").unwrap_or_default();
    let last = string_field(payload, "last_name").unwrap_or_default();
    let full = [first, last]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    if !full.is_empty() {
        return Some(full);
    }
    string_field(payload, "email")
        .or_else(|| string_field(payload, "phone"))
        .map(str::to_string)
}

fn shopify_conversation_id(
    shop: &str,
    topic: &str,
    delivery: Option<&str>,
    subject: Option<&ShopifySubject>,
) -> String {
    let shop = normalize_shopify_part(shop);
    let topic = normalize_shopify_topic(topic).replace('/', "_");
    if let Some(subject) = subject {
        if let Some(id) = &subject.id {
            return format!(
                "shopify:{shop}:{topic}:{}:{}",
                subject.conversation_kind,
                normalize_shopify_part(id)
            );
        }
    }
    format!(
        "shopify:{shop}:{topic}:{}",
        delivery
            .map(normalize_shopify_part)
            .unwrap_or_else(|| "event".to_string())
    )
}

fn shopify_message(
    shop: &str,
    topic: &str,
    delivery: Option<&str>,
    headers: &HeaderMap,
    payload: &Value,
    subject: Option<&ShopifySubject>,
) -> String {
    let mut lines = vec![format!("Shopify {topic}"), format!("Shop: {shop}")];
    if let Some(subject) = subject {
        lines.push(shopify_subject_line(subject));
        if let Some(url) = &subject.url {
            lines.push(format!("URL: {url}"));
        }
        if let Some(amount) = &subject.amount {
            lines.push(format!("Amount: {amount}"));
        }
    }
    if let Some(email) = string_field(payload, "email") {
        lines.push(format!("Email: {email}"));
    }
    if let Some(triggered_at) = header_value(headers, "x-shopify-triggered-at")
        .or_else(|| string_field(payload, "triggered_at"))
        .or_else(|| string_field(payload, "updated_at"))
        .or_else(|| string_field(payload, "created_at"))
    {
        lines.push(format!("Triggered: {triggered_at}"));
    }
    if let Some(delivery) = delivery {
        lines.push(format!("Event: {delivery}"));
    }
    if let Some(note) = string_field(payload, "note").map(snippet) {
        lines.push(String::new());
        lines.push(note);
    }
    lines.join("\n")
}

fn shopify_subject_line(subject: &ShopifySubject) -> String {
    let mut details = Vec::new();
    if let Some(id) = &subject.id {
        details.push(normalize_shopify_part(id));
    }
    if let Some(title) = &subject.title {
        details.push(snippet(title));
    }
    if let Some(status) = &subject.status {
        details.push(status.to_string());
    }
    if details.is_empty() {
        format!("Subject: {}", subject.kind)
    } else {
        format!("Subject: {} {}", subject.kind, details.join(" "))
    }
}

fn shopify_amount(payload: &Value) -> Option<String> {
    let amount = string_field(payload, "total_price")
        .or_else(|| string_field(payload, "current_total_price"))
        .or_else(|| string_field(payload, "subtotal_price"))?;
    let currency = string_field(payload, "currency")
        .or_else(|| string_field(payload, "presentment_currency"))
        .unwrap_or("currency");
    Some(format!("{amount} {currency}"))
}

fn shopify_url(payload: &Value) -> Option<String> {
    string_field(payload, "order_status_url")
        .or_else(|| string_field(payload, "admin_graphql_api_id"))
        .or_else(|| string_field(payload, "admin_url"))
        .or_else(|| string_field(payload, "url"))
        .map(str::to_string)
}

fn normalize_shopify_topic(value: &str) -> String {
    value.trim().to_ascii_lowercase()
}

fn normalize_shopify_part(value: &str) -> String {
    value
        .trim()
        .to_ascii_lowercase()
        .replace("https://", "")
        .replace("http://", "")
        .replace(':', "_")
        .replace('/', "_")
        .replace(' ', "_")
        .replace('-', "_")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shopify_order_payload_maps_to_inbound_message() {
        let mut headers = HeaderMap::new();
        headers.insert("x-shopify-topic", "orders/create".parse().unwrap());
        headers.insert(
            "x-shopify-shop-domain",
            "puffer-test.myshopify.com".parse().unwrap(),
        );
        headers.insert("x-shopify-event-id", "event-1".parse().unwrap());
        headers.insert(
            "x-shopify-triggered-at",
            "2026-05-25T10:30:00Z".parse().unwrap(),
        );
        let payload = serde_json::json!({
            "id": 450789469,
            "name": "#1001",
            "email": "ada@example.com",
            "financial_status": "paid",
            "total_price": "59.99",
            "currency": "USD",
            "order_status_url": "https://example.com/orders/1001",
            "note": "Gift wrap, please."
        });

        let inbound = shopify_inbound(&headers, &payload).expect("shopify inbound");

        assert_eq!(
            inbound.conversation_id,
            "shopify:puffer_test.myshopify.com:orders_create:order:450789469"
        );
        assert_eq!(
            inbound.user_id.as_deref(),
            Some("puffer-test.myshopify.com")
        );
        assert!(inbound.text.contains("Shopify orders/create"));
        assert!(inbound.text.contains("Subject: order 450789469 #1001 paid"));
        assert!(inbound.text.contains("Amount: 59.99 USD"));
        assert!(inbound.text.contains("Email: ada@example.com"));
        assert!(inbound.text.contains("Gift wrap, please."));
    }

    #[test]
    fn shopify_product_payload_maps_to_inbound_message() {
        let mut headers = HeaderMap::new();
        headers.insert("x-shopify-topic", "products/update".parse().unwrap());
        headers.insert(
            "x-shopify-shop-domain",
            "puffer-test.myshopify.com".parse().unwrap(),
        );
        headers.insert("x-shopify-webhook-id", "webhook-1".parse().unwrap());
        let payload = serde_json::json!({
            "id": "788032119674292922",
            "title": "Puffer Hoodie",
            "status": "active",
            "admin_graphql_api_id": "gid://shopify/Product/788032119674292922",
            "variants": []
        });

        let inbound = shopify_inbound(&headers, &payload).expect("shopify inbound");

        assert_eq!(
            inbound.conversation_id,
            "shopify:puffer_test.myshopify.com:products_update:product:788032119674292922"
        );
        assert!(inbound.text.contains("Shopify products/update"));
        assert!(inbound
            .text
            .contains("Subject: product 788032119674292922 Puffer Hoodie active"));
        assert!(inbound
            .text
            .contains("URL: gid://shopify/Product/788032119674292922"));
        assert!(inbound.text.contains("Event: webhook-1"));
    }

    #[test]
    fn shopify_shape_requires_shopify_header_or_payload_hint() {
        let headers = HeaderMap::new();
        let payload = serde_json::json!({"id": 123, "name": "#1001"});

        assert!(shopify_inbound(&headers, &payload).is_none());
    }
}
