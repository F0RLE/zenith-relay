use serde_json::Value;

const MAX_REPORTED_MODELS: usize = 64;
const MAX_REPORTED_MODEL_ID_BYTES: usize = 256;

/// Bounded observations from one access response, not a new entitlement source.
#[derive(Clone, Debug, Default)]
pub struct BasisPointsCatalogDiagnostics {
    pub access_allowed: bool,
    pub received_models: usize,
    pub accepted_models: usize,
    pub models: Vec<BasisPointsModelDiagnostic>,
}

#[derive(Clone, Debug)]
pub struct BasisPointsModelDiagnostic {
    pub row: usize,
    pub model_id: Option<String>,
    pub outcome: BasisPointsModelOutcome,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BasisPointsModelOutcome {
    Accepted,
    AccessDenied,
    MalformedRow,
    InvalidId,
    Restricted,
    Unavailable,
    Disabled,
    PickerDisabled,
    PolicyBlocked,
    Duplicate,
}

impl BasisPointsModelOutcome {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Accepted => "accepted",
            Self::AccessDenied => "access_denied",
            Self::MalformedRow => "malformed_row",
            Self::InvalidId => "invalid_id",
            Self::Restricted => "restricted",
            Self::Unavailable => "unavailable",
            Self::Disabled => "disabled",
            Self::PickerDisabled => "picker_disabled",
            Self::PolicyBlocked => "policy_blocked",
            Self::Duplicate => "duplicate",
        }
    }
}

impl BasisPointsCatalogDiagnostics {
    pub fn excluded_models(&self) -> usize {
        self.received_models.saturating_sub(self.accepted_models)
    }

    pub fn omitted_models(&self) -> usize {
        self.received_models.saturating_sub(self.models.len())
    }

    pub(super) fn model_id(&self, raw_model: &Value) -> Option<String> {
        if self.models.len() >= MAX_REPORTED_MODELS {
            return None;
        }
        // Keep only a bounded valid identifier. Other provider fields and
        // malformed IDs never enter the diagnostic projection.
        raw_model
            .get("id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|id| id.len() <= MAX_REPORTED_MODEL_ID_BYTES && crate::is_valid_model_token(id))
            .map(str::to_owned)
    }

    pub(super) fn observe(
        &mut self,
        row: usize,
        model_id: Option<String>,
        outcome: BasisPointsModelOutcome,
    ) {
        if outcome == BasisPointsModelOutcome::Accepted {
            self.accepted_models += 1;
        }
        if self.models.len() >= MAX_REPORTED_MODELS {
            return;
        }
        self.models.push(BasisPointsModelDiagnostic {
            row,
            model_id,
            outcome,
        });
    }
}

#[cfg(test)]
mod tests;
