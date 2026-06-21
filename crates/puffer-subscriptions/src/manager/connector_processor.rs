//! Connector stream event processor owned by the subscription manager.

use crate::action::ActionDispatcher;
use crate::classify::Classifier;
use crate::connection::ConnectionStore;
use crate::connector_stream::ConnectorEventProcessor;
use crate::history::WorkflowHistoryStore;
use crate::proxy::{
    builtin_agent_proxy, handle_agent_proxy_event as decide_agent_proxy_event, AgentProxyDecision,
    AgentProxyStore,
};
use crate::router::{
    process_envelope_batch_result_with_monitor_digest, process_envelope_result_with_monitor_digest,
    MonitorDigestQueue,
};
use crate::self_gate::SelfMessageGate;
use crate::spec::ActionSpec;
use crate::store::SubscriptionStore;
use anyhow::Result;
use puffer_subscriber_runtime::EventEnvelope;
use std::sync::Arc;

pub(super) struct ManagerConnectorEventProcessor {
    store: Arc<SubscriptionStore>,
    connection_store: Arc<ConnectionStore>,
    history_store: Arc<WorkflowHistoryStore>,
    proxy_store: Arc<AgentProxyStore>,
    dispatcher: Arc<dyn ActionDispatcher>,
    classifier: Arc<dyn Classifier>,
    self_gate: Arc<dyn SelfMessageGate>,
    monitor_digest: MonitorDigestQueue,
}

impl ManagerConnectorEventProcessor {
    pub(super) fn new(
        store: Arc<SubscriptionStore>,
        connection_store: Arc<ConnectionStore>,
        history_store: Arc<WorkflowHistoryStore>,
        proxy_store: Arc<AgentProxyStore>,
        dispatcher: Arc<dyn ActionDispatcher>,
        classifier: Arc<dyn Classifier>,
        self_gate: Arc<dyn SelfMessageGate>,
        monitor_digest: MonitorDigestQueue,
    ) -> Self {
        Self {
            store,
            connection_store,
            history_store,
            proxy_store,
            dispatcher,
            classifier,
            self_gate,
            monitor_digest,
        }
    }

    fn process_agent_proxy(
        &self,
        connector_slug: &str,
        connection_slug: &str,
        envelope: &EventEnvelope,
    ) -> Result<()> {
        match decide_agent_proxy_event(
            connector_slug,
            connection_slug,
            &envelope.event.payload,
            &self.proxy_store,
        )? {
            AgentProxyDecision::Ignore => Ok(()),
            AgentProxyDecision::ConnectorAction { action, input } => {
                self.dispatch_connector_action(connector_slug, &action, input, envelope)
            }
            AgentProxyDecision::BindAgent { reply, .. } => {
                let _ = self.connection_store.update(connection_slug, |record| {
                    record.set_has_consumer(true);
                });
                if let Some(input) = reply {
                    self.dispatch_connector_action(
                        connector_slug,
                        "send_message",
                        input,
                        envelope,
                    )?;
                }
                Ok(())
            }
            AgentProxyDecision::RouteToAgent {
                target,
                message,
                binding,
            } => {
                let Some(proxy) = builtin_agent_proxy(connector_slug) else {
                    return Ok(());
                };
                let prompt = proxy.route_prompt(&target, &message);
                let result = self.dispatcher.dispatch(
                    &ActionSpec::TriageAgent {
                        prompt,
                        model: None,
                    },
                    envelope,
                );
                if !result.success {
                    anyhow::bail!("{}", result.summary);
                }
                let input = proxy.render_agent_reply(&result.summary, &binding);
                self.dispatch_connector_action(connector_slug, "send_message", input, envelope)
            }
        }
    }

    fn dispatch_connector_action(
        &self,
        connector_slug: &str,
        action: &str,
        input: serde_json::Value,
        envelope: &EventEnvelope,
    ) -> Result<()> {
        let result = self.dispatcher.dispatch(
            &ActionSpec::ConnectorAct {
                connector_slug: connector_slug.to_string(),
                action: action.to_string(),
                input,
            },
            envelope,
        );
        if result.success {
            Ok(())
        } else {
            anyhow::bail!("{}", result.summary)
        }
    }
}

impl ConnectorEventProcessor for ManagerConnectorEventProcessor {
    fn process_connector_event(
        &self,
        connector_slug: &str,
        connection_slug: &str,
        envelope: &EventEnvelope,
    ) -> Result<()> {
        self.process_agent_proxy(connector_slug, connection_slug, envelope)?;
        let result = process_envelope_result_with_monitor_digest(
            envelope,
            &self.store,
            Some(&self.history_store),
            &self.dispatcher,
            &self.classifier,
            &self.self_gate,
            Some(&self.monitor_digest),
            None,
        );
        if result.failed > 0 {
            anyhow::bail!(
                "{} workflow action(s) failed while processing connector event",
                result.failed
            );
        }
        Ok(())
    }

    fn process_connector_events(
        &self,
        connector_slug: &str,
        connection_slug: &str,
        envelopes: &[EventEnvelope],
    ) -> Result<()> {
        for envelope in envelopes {
            self.process_agent_proxy(connector_slug, connection_slug, envelope)?;
        }
        let result = process_envelope_batch_result_with_monitor_digest(
            envelopes,
            &self.store,
            Some(&self.history_store),
            &self.dispatcher,
            &self.classifier,
            &self.self_gate,
            Some(&self.monitor_digest),
            None,
        );
        if result.failed > 0 {
            anyhow::bail!(
                "{} workflow action(s) failed while processing connector event batch",
                result.failed
            );
        }
        Ok(())
    }
}
