//! Application integration registry.
//!
//! Runtime integrations are deliberately described here instead of being
//! selected through application-name branches throughout the pool.  A new
//! client can be added by registering its descriptor first, then implementing
//! only the capabilities it supports.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ApplicationId {
    ChatGpt,
    OpenCode,
}

impl ApplicationId {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ChatGpt => "chatgpt",
            Self::OpenCode => "opencode",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplicationCapabilities {
    pub profile_recovery: bool,
    pub launch: bool,
    pub backup: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplicationDescriptor {
    pub id: ApplicationId,
    pub display_name: &'static str,
    pub capabilities: ApplicationCapabilities,
}

const APPLICATIONS: &[ApplicationDescriptor] = &[
    ApplicationDescriptor {
        id: ApplicationId::ChatGpt,
        display_name: "ChatGPT",
        capabilities: ApplicationCapabilities {
            profile_recovery: true,
            launch: true,
            backup: true,
        },
    },
    ApplicationDescriptor {
        id: ApplicationId::OpenCode,
        display_name: "OpenCode",
        capabilities: ApplicationCapabilities {
            profile_recovery: true,
            launch: true,
            backup: true,
        },
    },
];

pub fn all() -> &'static [ApplicationDescriptor] {
    APPLICATIONS
}

pub fn get(id: ApplicationId) -> &'static ApplicationDescriptor {
    APPLICATIONS
        .iter()
        .find(|descriptor| descriptor.id == id)
        .expect("every ApplicationId must be registered")
}

/// Validate the static catalog at startup so additions cannot silently omit a
/// descriptor or expose an empty identity to the UI/IPC layer.
pub fn validate() {
    debug_assert!(!APPLICATIONS.is_empty());
    for descriptor in all() {
        debug_assert!(!descriptor.id.as_str().is_empty());
        debug_assert!(!descriptor.display_name.is_empty());
    }
    debug_assert_eq!(get(ApplicationId::ChatGpt).id.as_str(), "chatgpt");
    debug_assert_eq!(get(ApplicationId::OpenCode).id.as_str(), "opencode");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_contains_one_descriptor_per_application() {
        assert_eq!(all().len(), 2);
        assert_eq!(get(ApplicationId::ChatGpt).display_name, "ChatGPT");
        assert_eq!(get(ApplicationId::OpenCode).display_name, "OpenCode");
    }

    #[test]
    fn capabilities_are_explicit() {
        let opencode = get(ApplicationId::OpenCode);
        assert!(opencode.capabilities.profile_recovery);
        assert!(opencode.capabilities.launch);
        assert!(opencode.capabilities.backup);
    }
}
