mod auth;
mod discovery;
mod import;
mod model;
mod registry;
mod secure_oauth;

pub use auth::{AuthMode, AuthStore, OAuthCredential, StoredCredential};
pub use discovery::{merge_discovered_models, ModelDiscoveryClient};
pub use import::{
    detect_import_candidates, ExternalImportCandidate, ExternalImportFamily, ExternalImportSource,
};
pub use model::{
    AnthropicMessagesCompat, Modality, ModelCompat, ModelCost, ModelDescriptor,
    ModelDiscoveryConfig, ModelDiscoveryFormat, OpenAiCompletionsCompat, OpenAiResponsesCompat,
    ProviderDescriptor, ProviderSource, ProviderSourceKind, RegisteredProvider, ResponsesPath,
    ThinkingFormat,
};
pub use registry::ProviderRegistry;
