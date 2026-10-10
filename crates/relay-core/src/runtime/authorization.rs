use super::AuthorizedRequestError;
use crate::scheduler::rotation::ExecutionCertainty;

/// Metadata and admission ownership shared by the first dispatch and its
/// proven authorization repair. Reusing it keeps retries on the same budget.
#[derive(Clone, Copy)]
pub(crate) struct AuthorizationDispatch<'a> {
    pub(crate) client_version: Option<&'a str>,
    pub(crate) identity_policy: super::AuthorizationIdentityPolicy,
    pub(crate) turn_scope: Option<&'a super::CodexTurnStateScope<'a>>,
    pub(crate) budget: Option<&'a crate::scheduler::rotation::SharedRequestBudget>,
    pub(crate) lease: Option<&'a super::CandidateLease>,
}

mod dispatch;
mod prepare;

impl AuthorizedRequestError {
    /// A transport error after execute() starts does not prove that the
    /// provider did not accept the generation. Only a connection failure is
    /// known to be pre-send; never transparently replay an unknown outcome.
    pub(crate) fn execution_certainty(&self) -> ExecutionCertainty {
        match self {
            Self::ProgressTimeout => ExecutionCertainty::Unknown,
            Self::Transport(error) if !error.is_connect() => ExecutionCertainty::Unknown,
            _ => ExecutionCertainty::NotSent,
        }
    }
}

#[cfg(test)]
use super::PreparedAuthorization;
#[cfg(test)]
use crate::providers::chatgpt::AgentIdentityCredential;
#[cfg(test)]
use prepare::agent_credential_fingerprint;
#[cfg(test)]
use reqwest::header::AUTHORIZATION;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_turn_state_identity_ignores_signature_timestamp_but_tracks_credentials() {
        let agent = AgentIdentityCredential::new(
            "MC4CAQAwBQYDK2VwBCIEIAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8g".into(),
            "synthetic-runtime".into(),
            "synthetic-task".into(),
        )
        .unwrap();
        let prepare = |agent: &AgentIdentityCredential, timestamp| PreparedAuthorization {
            header_name: AUTHORIZATION,
            authorization: agent.authorization(timestamp).unwrap(),
            identity: None,
            token_generation: None,
            token_revision: None,
            agent_task_id: agent.task_id().map(str::to_string),
            agent_credential_fingerprint: Some(agent_credential_fingerprint(agent)),
            agent_identity_revision: Some(0),
        };
        let first = prepare(&agent, 1_000);
        let later_authorization = prepare(&agent, 2_000);
        assert_ne!(first.authorization, later_authorization.authorization);
        assert_eq!(
            first.turn_state_credential(),
            later_authorization.turn_state_credential()
        );
        let changed_task = prepare(&agent.with_task_id("another-task".into()).unwrap(), 2_000);
        assert_ne!(
            first.turn_state_credential(),
            changed_task.turn_state_credential()
        );
        let changed_runtime = AgentIdentityCredential::new(
            agent.private_key().into(),
            "another-runtime".into(),
            "synthetic-task".into(),
        )
        .unwrap();
        assert_ne!(
            first.turn_state_credential(),
            prepare(&changed_runtime, 2_000).turn_state_credential()
        );
    }
}
