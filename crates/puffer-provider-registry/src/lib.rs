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
    ModelDescriptor, ModelDiscoveryConfig, ModelDiscoveryFormat, ProviderDescriptor,
    ProviderSource, ProviderSourceKind, RegisteredProvider,
};
pub use registry::ProviderRegistry;
