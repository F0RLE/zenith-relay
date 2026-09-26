use super::contracts::PreparedAdapterRequest;
use crate::tool_policy::{apply_tool_policy, ToolPolicyResult};
use crate::ToolPolicy;

impl PreparedAdapterRequest {
    /// Apply the non-destructive policy at the final catalog boundary, after
    /// input/history translation and before any upstream I/O.
    pub(crate) fn apply_tool_policy(
        &mut self,
        policy: &ToolPolicy,
    ) -> Result<ToolPolicyResult, &'static str> {
        apply_tool_policy(self.upstream_body_mut(), policy)
    }
}
