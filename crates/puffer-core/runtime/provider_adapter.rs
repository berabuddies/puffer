//! Neutral provider adapter trait that decouples agent-loop dispatch
//! from vendor-specific request shaping.
//!
//! Inspired by pi-mono's `ApiProvider<TApi>` registry pattern (see
//! `pi-mono/packages/ai/src/api-registry.ts`). Each adapter owns its
//! wire format, SSE parser, and tool-call extraction — the loop only
//! sees the neutral `ConversationItem` / `TurnStreamEvent` /
//! `TurnExecution` types defined in `runtime.rs`.
//!
//! Adding a new provider:
//! 1. Implement `ProviderAdapter` for a unit struct in the provider's
//!    module (e.g. `runtime/foo.rs`).
//! 2. Add a `match` arm in `adapter_for_api` returning a static
//!    reference to it.
//! 3. No edits to `runtime.rs` dispatch.

use super::anthropic::AnthropicAdapter;
use super::openai::{OpenAICompletionsAdapter, OpenAIResponsesAdapter};
use super::{TurnExecution, TurnRequestOptions, TurnStreamEvent};
use crate::AppState;
use anyhow::Result;
use puffer_provider_registry::{AuthStore, ProviderDescriptor, ProviderRegistry};
use puffer_resources::LoadedResources;

pub(crate) trait ProviderAdapter: Send + Sync {
    /// Stable identifier shown in error messages — usually the same
    /// string the registry stores in `ModelDescriptor::api`.
    fn api_id(&self) -> &'static str;

    /// Executes one turn without streaming events.
    fn execute_turn(
        &self,
        state: &mut AppState,
        resources: &LoadedResources,
        providers: &ProviderRegistry,
        provider: &ProviderDescriptor,
        model_id: String,
        auth_store: &mut AuthStore,
        input: &str,
        options: TurnRequestOptions<'_>,
    ) -> Result<TurnExecution>;

    /// Executes one turn with incremental `TurnStreamEvent`s. The default
    /// impl ignores events and falls back to non-streaming, which keeps
    /// adapters that do not (yet) support streaming functional.
    fn execute_turn_streaming(
        &self,
        state: &mut AppState,
        resources: &LoadedResources,
        providers: &ProviderRegistry,
        provider: &ProviderDescriptor,
        model_id: String,
        auth_store: &mut AuthStore,
        input: &str,
        options: TurnRequestOptions<'_>,
        _on_event: &mut dyn FnMut(TurnStreamEvent),
    ) -> Result<TurnExecution> {
        self.execute_turn(
            state, resources, providers, provider, model_id, auth_store, input, options,
        )
    }
}

/// Looks up the adapter for the given API id.
///
/// Returns `None` when no adapter is registered — `runtime.rs` then
/// bails with the canonical "provider X with api Y is not executable
/// yet" message so user-facing errors stay stable.
pub(crate) fn adapter_for_api(api: &str) -> Option<&'static dyn ProviderAdapter> {
    match api {
        "anthropic-messages" => Some(&AnthropicAdapter),
        "openai-responses" | "azure-openai-responses" | "openai-codex-responses" => {
            Some(&OpenAIResponsesAdapter)
        }
        "openai-completions" => Some(&OpenAICompletionsAdapter),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_resolves_known_apis_to_distinct_adapters() {
        // Each registered API id must resolve to *some* adapter, and the
        // resolved adapter must report the correct family-canonical id.
        let cases: &[(&str, &str)] = &[
            ("anthropic-messages", "anthropic-messages"),
            ("openai-responses", "openai-responses"),
            ("azure-openai-responses", "openai-responses"),
            ("openai-codex-responses", "openai-responses"),
            ("openai-completions", "openai-completions"),
        ];
        for (api, expected_canonical) in cases {
            let adapter = adapter_for_api(api).unwrap_or_else(|| {
                panic!("registry returned None for known api {api}");
            });
            assert_eq!(
                adapter.api_id(),
                *expected_canonical,
                "{api} resolved to adapter with api_id {}",
                adapter.api_id()
            );
        }
    }

    #[test]
    fn registry_returns_none_for_unknown_api() {
        // Unknown apis must return None so dispatch can emit the
        // canonical "not executable yet" message instead of silently
        // routing to the wrong adapter.
        assert!(adapter_for_api("gemini-generate").is_none());
        assert!(adapter_for_api("").is_none());
        assert!(
            adapter_for_api("anthropic").is_none(),
            "must require full id"
        );
    }
}
