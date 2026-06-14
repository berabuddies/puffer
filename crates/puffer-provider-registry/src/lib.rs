mod auth;
mod discovery;
mod import;
mod media_capability;
mod model;
mod registry;
mod secure_oauth;

pub use auth::{AuthMode, AuthStore, OAuthCredential, StoredCredential};
pub use discovery::{merge_discovered_models, ModelDiscoveryClient};
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
