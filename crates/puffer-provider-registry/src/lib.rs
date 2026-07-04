mod auth;
mod discovery;
mod discovery_cache;
mod import;
mod input_capability;
mod media_capability;
mod model;
mod registry;
mod secure_oauth;

pub use auth::{AuthMode, AuthStore, OAuthCredential, StoredCredential};
pub use discovery::{merge_discovered_models, ModelDiscoveryClient};

/// Drops a provider's cached discovery entry so its models are re-discovered on
/// the next run. Call when a provider's account/credential changes (e.g.
/// switching GitHub Copilot accounts) so a stale, account-specific model list
/// isn't served to a different account.
pub fn invalidate_provider_discovery_cache(provider_id: &str) {
    discovery_cache::invalidate_cached_provider(provider_id);
}
pub use import::{
    detect_import_candidates, ExternalImportCandidate, ExternalImportFamily, ExternalImportSource,
};
pub use media_capability::{
    Axis, AxisRole, ControlKind, MediaMap, MediaRatioMap, MediaSizeMap, Variant, Variants,
    WireType, CANONICAL_MEDIA_RATIOS,
};
pub use model::{
    AnthropicMessagesCompat, MediaBatchDescriptor, MediaBatchMode, MediaDiscoveryDescriptor,
    MediaDiscoveryKind, MediaExecutionDescriptor, MediaExecutionKind, MediaKindDescriptor,
    MediaModelDescriptor, MediaOperation, Modality, ModelCompat, ModelCost, ModelDescriptor,
    ModelDiscoveryConfig, ModelDiscoveryFormat, OpenAiCompletionsCompat, OpenAiResponsesCompat,
    ProviderDescriptor, ProviderMediaDescriptor, ProviderSource, ProviderSourceKind,
    RegisteredProvider, ResponsesPath, ThinkingFormat, VideoPromptFormat,
};
pub use registry::{canonical_provider_id, ProviderRegistry};
