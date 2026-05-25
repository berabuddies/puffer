---
name: shopify-webhook
description: Configure the Shopify webhook connector preset with /connect so order, product, customer, inventory, and fulfillment events reach Puffer.
---

# Shopify Webhook Connector

Use this skill when the user wants Shopify order, product, customer, inventory,
fulfillment, or app lifecycle webhook events to reach Puffer workflows or agent
sessions.

## Setup

Run:

```text
/connect shopify-webhook <connection-name>
```

If no connection name is supplied, ask for one. The deterministic default is:

```text
/connect shopify-webhook shopify-webhook
```

The setup flow asks for:

- bind address, such as `127.0.0.1:9999`
- webhook URL path, defaulting to `/shopify` when left blank

The command writes `[connectors.webhook]` in `.puffer/connectors.toml` because
the Shopify preset is served by the generic webhook connector. Start it with:

```text
puffer serve
```

## Shopify Configuration

Create Shopify webhook subscriptions for the topics the user cares about, such
as:

- `orders/create`, `orders/updated`, or `orders/cancelled`
- `products/create` or `products/update`
- `customers/create` or `customers/update`
- `inventory_levels/update`
- `fulfillments/create` or `fulfillments/update`
- `app/uninstalled`

For REST Admin API webhook subscriptions, use an HTTPS address pointing at the
public `puffer serve` listener and path, with `format` set to `json`.

Example callback URL:

```text
https://example.com/shopify
```

Shopify HTTP deliveries include headers such as `X-Shopify-Topic`,
`X-Shopify-Shop-Domain`, `X-Shopify-Webhook-Id`, `X-Shopify-Triggered-At`, and
`X-Shopify-Event-Id`. Puffer uses those headers for routing and readable message
summaries. The preset does not verify `X-Shopify-Hmac-Sha256` yet because the
setup flow does not collect the Shopify app secret.

## Runtime Behavior

This preset is setup-only in the workflow connector catalog. It does not expose
a connector protocol subscriber, workflow trigger runtime, or outbound action
yet. It prepares the serve-mode webhook endpoint so incoming Shopify JSON
payloads can be normalized into readable Puffer messages.

Puffer groups conversations by shop, topic, and the first stable Shopify subject
in the delivery, for example:

- `shopify:<shop>:orders_create:order:<order-id>`
- `shopify:<shop>:products_update:product:<product-id>`
- `shopify:<shop>:customers_update:customer:<customer-id>`
- `shopify:<shop>:inventory_levels_update:inventory:<item-location>`

Messages include topic, shop, subject, status, amount, URL, email, timestamp,
event id, and note text when those fields are present.
