use axum::http::HeaderMap;
use puffer_connector_core::InboundMessage;
use serde_json::Value;

use super::super::{header_value, number_or_string, pointer_string, snippet, string_field};

/// Converts a Vercel webhook payload into an inbound Puffer message.
pub(super) fn vercel_inbound(headers: &HeaderMap, payload: &Value) -> Option<InboundMessage> {
    let event = string_field(payload, "type")?;
    if !vercel_payload_shape(headers, payload, event) {
        return None;
    }

    let body = payload.get("payload")?;
    let delivery = string_field(payload, "id")
        .map(str::to_string)
        .or_else(|| payload.get("createdAt").and_then(number_or_string));
    let actor = vercel_actor(body).unwrap_or_else(|| "vercel".to_string());
    let conversation_id = vercel_conversation_id(event, body, delivery.as_deref());
    let text = vercel_message(event, body, payload, &actor);

    Some(InboundMessage {
        conversation_id,
        user_id: Some(actor),
        text,
        thread_id: None,
        is_group: false,
        bot_mentioned: true,
        from_bot: false,
    })
}

fn vercel_payload_shape(headers: &HeaderMap, payload: &Value, event: &str) -> bool {
    let known_event = event.starts_with("deployment.")
        || event.starts_with("project.")
        || event.starts_with("domain.")
        || event.starts_with("flag.")
        || event.starts_with("firewall.")
        || event.starts_with("integration.")
        || event.starts_with("billing.");
    let has_payload = payload.get("payload").is_some_and(Value::is_object);
    let has_delivery_context = string_field(payload, "id").is_some()
        || payload
            .get("createdAt")
            .and_then(number_or_string)
            .is_some()
        || header_value(headers, "x-vercel-signature").is_some();

    known_event && has_payload && has_delivery_context
}

fn vercel_conversation_id(event: &str, body: &Value, delivery: Option<&str>) -> String {
    if event.starts_with("deployment.") {
        if let Some(id) = vercel_deployment_id(body) {
            return format!("vercel:deployment:{}", normalize_vercel_part(&id));
        }
    }
    if event.starts_with("project.env-variable.") {
        let project = vercel_project_id(body).unwrap_or_else(|| "project".to_string());
        let env = string_field(body, "envVarId")
            .or_else(|| string_field(body, "environmentVariableId"))
            .unwrap_or("env");
        return format!(
            "vercel:project:{}:env:{}",
            normalize_vercel_part(&project),
            normalize_vercel_part(env)
        );
    }
    if event.starts_with("project.domain.") {
        if let Some(project) = vercel_project_id(body) {
            if let Some(domain) = vercel_domain_name(body) {
                return format!(
                    "vercel:project:{}:domain:{}",
                    normalize_vercel_part(&project),
                    normalize_vercel_part(&domain)
                );
            }
        }
    }
    if event.starts_with("project.") {
        if let Some(project) = vercel_project_id(body) {
            return format!("vercel:project:{}", normalize_vercel_part(&project));
        }
    }
    if event.starts_with("domain.") {
        if let Some(domain) = vercel_domain_name(body) {
            return format!("vercel:domain:{}", normalize_vercel_part(&domain));
        }
    }
    if event.starts_with("flag.") {
        if let Some(flag) = vercel_flag_name(body) {
            return format!("vercel:flag:{}", normalize_vercel_part(&flag));
        }
    }
    let subject = vercel_project_id(body)
        .or_else(|| vercel_deployment_id(body))
        .or_else(|| vercel_domain_name(body))
        .or_else(|| delivery.map(str::to_string))
        .unwrap_or_else(|| "event".to_string());
    format!(
        "vercel:{}:{}",
        normalize_vercel_part(event),
        normalize_vercel_part(&subject)
    )
}

fn vercel_message(event: &str, body: &Value, envelope: &Value, actor: &str) -> String {
    let mut lines = vec![format!("Vercel {event}")];
    lines.push(format!("Actor: {actor}"));
    if let Some(team) = pointer_string(body, "/team/id") {
        lines.push(format!("Team: {team}"));
    }
    append_vercel_deployment(&mut lines, body);
    append_vercel_project(&mut lines, body);
    append_vercel_domain(&mut lines, body);
    append_vercel_flag(&mut lines, body);
    append_vercel_event_fields(&mut lines, body);
    if let Some(created_at) = envelope.get("createdAt").and_then(number_or_string) {
        lines.push(format!("Created at: {created_at}"));
    }
    if let Some(region) = string_field(envelope, "region") {
        lines.push(format!("Region: {region}"));
    }
    lines.join("\n")
}

fn append_vercel_deployment(lines: &mut Vec<String>, body: &Value) {
    let Some(id) = vercel_deployment_id(body) else {
        return;
    };
    let name = pointer_string(body, "/deployment/name").unwrap_or("deployment");
    lines.push(format!("Deployment: {name} {id}"));
    if let Some(target) =
        string_field(body, "target").or_else(|| pointer_string(body, "/deployment/target"))
    {
        lines.push(format!("Target: {target}"));
    }
    if let Some(plan) = string_field(body, "plan") {
        lines.push(format!("Plan: {plan}"));
    }
    if let Some(regions) = vercel_string_list(
        body.get("regions")
            .or_else(|| body.pointer("/deployment/regions")),
    ) {
        lines.push(format!("Regions: {regions}"));
    }
    if let Some(url) = pointer_string(body, "/deployment/url") {
        lines.push(format!("URL: {}", vercel_url(url)));
    }
    if let Some(link) = pointer_string(body, "/links/deployment") {
        lines.push(format!("Dashboard: {link}"));
    }
}

fn append_vercel_project(lines: &mut Vec<String>, body: &Value) {
    let Some(project) = body.get("project") else {
        if let Some(project_id) = string_field(body, "projectId") {
            lines.push(format!("Project: {project_id}"));
        }
        return;
    };
    let id = string_field(project, "id").unwrap_or("project");
    let name = string_field(project, "name")
        .or_else(|| string_field(body, "projectSlug"))
        .unwrap_or("project");
    lines.push(format!("Project: {name} {id}"));
    if let Some(link) = pointer_string(body, "/links/project") {
        lines.push(format!("Project dashboard: {link}"));
    }
}

fn append_vercel_domain(lines: &mut Vec<String>, body: &Value) {
    if let Some(domain) = vercel_domain_name(body) {
        lines.push(format!("Domain: {domain}"));
    }
}

fn append_vercel_flag(lines: &mut Vec<String>, body: &Value) {
    if let Some(flag) = vercel_flag_name(body) {
        lines.push(format!("Flag: {}", snippet(&flag)));
    }
}

fn append_vercel_event_fields(lines: &mut Vec<String>, body: &Value) {
    if let Some(env_var) =
        string_field(body, "envVarId").or_else(|| string_field(body, "environmentVariableId"))
    {
        lines.push(format!("Environment variable: {env_var}"));
    }
    if let Some(action) = string_field(body, "action") {
        lines.push(format!("Action: {}", snippet(action)));
    }
    if let Some(from) = string_field(body, "fromDeploymentId") {
        lines.push(format!("From deployment: {from}"));
    }
    if let Some(to) = string_field(body, "toDeploymentId") {
        lines.push(format!("To deployment: {to}"));
    }
    if let Some(observability) = pointer_string(body, "/links/observability") {
        lines.push(format!("Observability: {observability}"));
    }
}

fn vercel_actor(body: &Value) -> Option<String> {
    pointer_string(body, "/user/username")
        .or_else(|| pointer_string(body, "/user/email"))
        .or_else(|| pointer_string(body, "/user/id"))
        .map(str::to_string)
}

fn vercel_deployment_id(body: &Value) -> Option<String> {
    pointer_string(body, "/deployment/id")
        .map(str::to_string)
        .or_else(|| string_field(body, "deploymentId").map(str::to_string))
        .or_else(|| string_field(body, "fromDeploymentId").map(str::to_string))
        .or_else(|| string_field(body, "toDeploymentId").map(str::to_string))
}

fn vercel_project_id(body: &Value) -> Option<String> {
    pointer_string(body, "/project/id")
        .or_else(|| string_field(body, "projectId"))
        .or_else(|| string_field(body, "projectSlug"))
        .map(str::to_string)
}

fn vercel_domain_name(body: &Value) -> Option<String> {
    pointer_string(body, "/domain/name")
        .or_else(|| string_field(body, "domainName"))
        .or_else(|| string_field(body, "domain"))
        .map(str::to_string)
}

fn vercel_flag_name(body: &Value) -> Option<String> {
    pointer_string(body, "/flag/key")
        .or_else(|| pointer_string(body, "/flag/name"))
        .or_else(|| string_field(body, "flagKey"))
        .or_else(|| string_field(body, "flagName"))
        .map(str::to_string)
}

fn vercel_string_list(value: Option<&Value>) -> Option<String> {
    let values = value?.as_array()?;
    let labels = values
        .iter()
        .filter_map(|value| value.as_str().map(str::to_string))
        .collect::<Vec<_>>();
    (!labels.is_empty()).then(|| labels.join(", "))
}

fn vercel_url(value: &str) -> String {
    if value.starts_with("http://") || value.starts_with("https://") {
        value.to_string()
    } else {
        format!("https://{value}")
    }
}

fn normalize_vercel_part(value: &str) -> String {
    let normalized = value
        .trim()
        .to_ascii_lowercase()
        .replace(':', "_")
        .replace('/', "_")
        .replace(' ', "_")
        .replace('-', "_");
    if normalized.is_empty() {
        "vercel".to_string()
    } else {
        normalized
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vercel_deployment_payload_maps_to_deployment_thread() {
        let mut headers = HeaderMap::new();
        headers.insert("x-vercel-signature", "sig".parse().unwrap());
        let payload = serde_json::json!({
            "id": "evt_123",
            "type": "deployment.ready",
            "createdAt": 1710000000000u64,
            "region": "iad1",
            "payload": {
                "team": {"id": "team_123"},
                "user": {"id": "user_123"},
                "deployment": {
                    "id": "dpl_123",
                    "name": "puffer",
                    "url": "puffer-git-main-acme.vercel.app"
                },
                "links": {
                    "deployment": "https://vercel.com/acme/puffer/deployments/dpl_123",
                    "project": "https://vercel.com/acme/puffer"
                },
                "target": "production",
                "project": {"id": "prj_123", "name": "puffer"},
                "plan": "pro",
                "regions": ["iad1", "sfo1"]
            }
        });

        let inbound = vercel_inbound(&headers, &payload).expect("vercel inbound");

        assert_eq!(inbound.conversation_id, "vercel:deployment:dpl_123");
        assert_eq!(inbound.user_id.as_deref(), Some("user_123"));
        assert!(inbound.text.contains("Vercel deployment.ready"));
        assert!(inbound.text.contains("Deployment: puffer dpl_123"));
        assert!(inbound
            .text
            .contains("URL: https://puffer-git-main-acme.vercel.app"));
        assert!(inbound.text.contains("Regions: iad1, sfo1"));
    }

    #[test]
    fn vercel_env_var_payload_uses_project_env_thread() {
        let headers = HeaderMap::new();
        let payload = serde_json::json!({
            "id": "evt_env",
            "type": "project.env-variable.updated",
            "createdAt": 1710000000000u64,
            "payload": {
                "team": {"id": "team_123"},
                "user": {"email": "tony@example.com"},
                "projectId": "prj_123",
                "envVarId": "env_123"
            }
        });

        let inbound = vercel_inbound(&headers, &payload).expect("vercel inbound");

        assert_eq!(
            inbound.conversation_id,
            "vercel:project:prj_123:env:env_123"
        );
        assert_eq!(inbound.user_id.as_deref(), Some("tony@example.com"));
        assert!(inbound.text.contains("Environment variable: env_123"));
    }

    #[test]
    fn vercel_shape_rejects_unrelated_type_payloads() {
        let headers = HeaderMap::new();
        let payload = serde_json::json!({
            "id": "evt_other",
            "type": "issue.updated",
            "createdAt": 1710000000000u64,
            "payload": {"issue": {"id": "123"}}
        });

        assert!(vercel_inbound(&headers, &payload).is_none());
    }
}
