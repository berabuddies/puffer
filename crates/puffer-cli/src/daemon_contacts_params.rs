//! Request parameter shapes for desktop contact RPCs.

use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub(super) struct ContactListParams {
    #[serde(default)]
    pub(super) limit: Option<usize>,
    #[serde(default)]
    pub(super) cursor: Option<String>,
    #[serde(default)]
    pub(super) query: Option<String>,
    #[serde(default)]
    pub(super) connector: Option<String>,
    #[serde(default)]
    pub(super) sort: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct ContactSaveParams {
    #[serde(default)]
    pub(super) id: Option<String>,
    pub(super) name: String,
    #[serde(default)]
    pub(super) description: String,
    #[serde(default)]
    pub(super) avatar: Option<String>,
    #[serde(default)]
    pub(super) contact_ids: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct ContactDeleteParams {
    pub(super) id: String,
}

#[derive(Debug, Deserialize)]
pub(super) struct ContactContextParams {
    #[serde(default)]
    pub(super) contact_ids: Vec<String>,
    #[serde(default)]
    pub(super) limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub(super) struct ContactInferParams {
    #[serde(default)]
    pub(super) limit: Option<usize>,
    #[serde(default)]
    pub(super) model: Option<String>,
    #[serde(default, alias = "traceId")]
    pub(super) trace_id: Option<String>,
}
