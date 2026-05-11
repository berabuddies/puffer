use super::profile::{
    EffectiveApprovalPolicy, EffectivePermissionProfile, EffectiveSandboxMode, PermissionSurface,
};
use puffer_runner_api::{FilesystemExecutionPolicy, FilesystemSandboxMode};
use std::path::PathBuf;

/// Describes the executor-facing filesystem policy derived from the effective profile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FilesystemPermissionPolicy {
    pub(crate) approval: EffectiveApprovalPolicy,
    pub(crate) sandbox_mode: EffectiveSandboxMode,
    pub(crate) workspace_roots: Vec<PathBuf>,
    pub(crate) session_granted: bool,
}

impl FilesystemPermissionPolicy {
    /// Returns true when legacy Claude-style path guards should be bypassed.
    pub(crate) fn allow_all_paths(&self) -> bool {
        matches!(self.sandbox_mode, EffectiveSandboxMode::DangerFullAccess)
    }

    /// Returns the legacy sandbox label consumed by existing filesystem executors.
    pub(crate) fn legacy_sandbox_mode(&self) -> &'static str {
        self.sandbox_mode.legacy_mode_label()
    }

    /// Converts the policy into the runner transport DTO.
    pub(crate) fn runner_policy(&self) -> FilesystemExecutionPolicy {
        FilesystemExecutionPolicy {
            sandbox_mode: match self.sandbox_mode {
                EffectiveSandboxMode::ReadOnly => FilesystemSandboxMode::ReadOnly,
                EffectiveSandboxMode::WorkspaceWrite => FilesystemSandboxMode::WorkspaceWrite,
                EffectiveSandboxMode::DangerFullAccess => FilesystemSandboxMode::DangerFullAccess,
                EffectiveSandboxMode::Custom => FilesystemSandboxMode::Custom,
            },
        }
    }
}

/// Describes the executor-facing process policy derived from the effective profile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProcessPermissionPolicy {
    pub(crate) approval: EffectiveApprovalPolicy,
    pub(crate) sandbox_mode: EffectiveSandboxMode,
    pub(crate) allow_unsandboxed_fallback: bool,
    pub(crate) excluded_commands: Vec<String>,
    pub(crate) session_granted: bool,
}

/// Describes the executor-facing network policy derived from the effective profile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NetworkPermissionPolicy {
    pub(crate) approval: EffectiveApprovalPolicy,
    pub(crate) session_granted: bool,
}

/// Bundles the executor-facing policies derived from one effective permission profile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DerivedPermissionPolicy {
    filesystem: FilesystemPermissionPolicy,
    process: ProcessPermissionPolicy,
    network: NetworkPermissionPolicy,
}

impl DerivedPermissionPolicy {
    /// Builds executor-facing policies from the normalized effective permission profile.
    pub(crate) fn from_profile(profile: &EffectivePermissionProfile) -> Self {
        let filesystem_surface = profile
            .surface(PermissionSurface::Filesystem)
            .expect("filesystem surface profile must exist");
        let process_surface = profile
            .surface(PermissionSurface::Process)
            .expect("process surface profile must exist");
        let network_surface = profile
            .surface(PermissionSurface::Network)
            .expect("network surface profile must exist");

        Self {
            filesystem: FilesystemPermissionPolicy {
                approval: filesystem_surface.default_approval,
                sandbox_mode: profile.sandbox_mode,
                workspace_roots: profile.workspace_roots.clone(),
                session_granted: filesystem_surface.session_granted,
            },
            process: ProcessPermissionPolicy {
                approval: process_surface.default_approval,
                sandbox_mode: profile.sandbox_mode,
                allow_unsandboxed_fallback: profile.allow_unsandboxed_fallback,
                excluded_commands: profile.sandbox_excluded_commands.clone(),
                session_granted: process_surface.session_granted,
            },
            network: NetworkPermissionPolicy {
                approval: network_surface.default_approval,
                session_granted: network_surface.session_granted,
            },
        }
    }

    /// Returns the derived filesystem policy.
    pub(crate) fn filesystem(&self) -> &FilesystemPermissionPolicy {
        &self.filesystem
    }

    /// Returns the derived process policy.
    pub(crate) fn process(&self) -> &ProcessPermissionPolicy {
        &self.process
    }

    /// Returns the derived network policy.
    pub(crate) fn network(&self) -> &NetworkPermissionPolicy {
        &self.network
    }

    /// Returns the compatibility bridge still consumed by legacy executors.
    pub(crate) fn legacy_bridge(&self) -> LegacyExecutorPermissionBridge {
        LegacyExecutorPermissionBridge {
            allow_all_paths: self.filesystem.allow_all_paths(),
            filesystem_sandbox_mode: self.filesystem.legacy_sandbox_mode(),
            allow_unsandboxed_fallback: self.process.allow_unsandboxed_fallback,
            excluded_commands: self.process.excluded_commands.clone(),
        }
    }
}

/// Carries the legacy execution bits still required by lower-level executors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LegacyExecutorPermissionBridge {
    pub(crate) allow_all_paths: bool,
    pub(crate) filesystem_sandbox_mode: &'static str,
    pub(crate) allow_unsandboxed_fallback: bool,
    pub(crate) excluded_commands: Vec<String>,
}

impl LegacyExecutorPermissionBridge {
    /// Rebuilds the legacy bridge from the runner DTO's flattened fields.
    pub(crate) fn from_allow_all_paths(allow_all_paths: bool) -> Self {
        let filesystem_sandbox_mode = if allow_all_paths {
            "danger-full-access"
        } else {
            "workspace-write"
        };
        Self {
            allow_all_paths,
            filesystem_sandbox_mode,
            allow_unsandboxed_fallback: false,
            excluded_commands: Vec::new(),
        }
    }
}

impl EffectiveSandboxMode {
    /// Returns the legacy sandbox mode label used by compatibility call sites.
    pub(crate) fn legacy_mode_label(self) -> &'static str {
        match self {
            Self::ReadOnly => "read-only",
            Self::WorkspaceWrite => "workspace-write",
            Self::DangerFullAccess => "danger-full-access",
            Self::Custom => "workspace-write",
        }
    }
}

impl EffectivePermissionProfile {
    /// Derives the executor-facing policies and legacy bridge inputs from the profile.
    pub(crate) fn derived_policy(&self) -> DerivedPermissionPolicy {
        DerivedPermissionPolicy::from_profile(self)
    }
}
