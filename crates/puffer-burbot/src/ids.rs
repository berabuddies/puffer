use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub(crate) struct RunId(pub Uuid);

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct NodeId(pub u64);

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub(crate) struct TraceId(pub Uuid);

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub(crate) struct MutationId(pub Uuid);

impl RunId {
    pub(crate) fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl TraceId {
    pub(crate) fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl MutationId {
    pub(crate) fn new() -> Self {
        Self(Uuid::new_v4())
    }
}
