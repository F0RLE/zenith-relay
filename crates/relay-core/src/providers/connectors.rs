//! Public account-connector metadata, not credential storage or admission.
//! A provider family and an issuing OAuth client are different identities.
use super::chatgpt::OAuthClientKind;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountProviderFamily {
    Openai,
    Anthropic,
    Google,
    Moonshot,
    Xai,
    Meta,
    Cognition,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountSignInFlow {
    AuthorizationCodePkce,
    DynamicClientPkce,
    DeviceAuthorization,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountConnectorDescriptor {
    pub id: String,
    pub provider_family: AccountProviderFamily,
    /// The current OpenAI connector's persisted discriminator. Future provider
    /// credentials must use their own typed authority, not this discriminator.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oauth_client_kind: Option<OAuthClientKind>,
    pub sign_in_flow: AccountSignInFlow,
}

impl AccountConnectorDescriptor {
    pub fn for_openai_client(client: OAuthClientKind) -> Self {
        let (id, sign_in_flow) = match client {
            OAuthClientKind::Codex => ("openai.codex", AccountSignInFlow::AuthorizationCodePkce),
            OAuthClientKind::Siwc => ("openai.siwc", AccountSignInFlow::DynamicClientPkce),
            OAuthClientKind::ExcelBps => {
                ("openai.excel_bps", AccountSignInFlow::AuthorizationCodePkce)
            }
        };
        Self {
            id: id.into(),
            provider_family: AccountProviderFamily::Openai,
            oauth_client_kind: Some(client),
            sign_in_flow,
        }
    }
}

/// Only implemented sign-in methods are advertised. Reserved provider-family
/// identifiers never imply a login flow, model access or pool eligibility.
pub fn available_account_connectors() -> Vec<AccountConnectorDescriptor> {
    [
        OAuthClientKind::Codex,
        OAuthClientKind::Siwc,
        OAuthClientKind::ExcelBps,
    ]
    .into_iter()
    .map(AccountConnectorDescriptor::for_openai_client)
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_existing_openai_connections_are_advertised_with_distinct_ids() {
        let descriptors = available_account_connectors();
        let ids = descriptors
            .iter()
            .map(|item| item.id.as_str())
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(ids.len(), 3);
        assert!(
            descriptors
                .iter()
                .all(|item| item.provider_family == AccountProviderFamily::Openai)
        );
        assert_eq!(
            descriptors[1].sign_in_flow,
            AccountSignInFlow::DynamicClientPkce
        );
        for item in descriptors {
            let encoded = serde_json::to_value(&item).unwrap();
            assert_eq!(encoded["providerFamily"], "openai");
            assert_eq!(
                serde_json::from_value::<AccountConnectorDescriptor>(encoded).unwrap(),
                item
            );
        }
    }
}
