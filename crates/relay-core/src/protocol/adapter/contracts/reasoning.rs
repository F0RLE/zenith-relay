use serde::{Deserialize, Serialize};
/// The internal upstream thinking contract used by a Messages bridge.
/// Persisted source bindings normalize to `Adaptive`; the enum remains part of
/// the adapter contract and focused protocol tests.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MessagesReasoningMode {
    #[default]
    Disabled,
    Budget,
    Adaptive,
}

impl MessagesReasoningMode {
    /// Returns whether the Responses-to-Messages bridge can represent the
    /// requested Codex effort on this upstream route.
    ///
    /// The bridge may advertise only efforts it can actually translate. Native
    /// Responses routes do not use this list: they preserve a provider's
    /// confirmed effort value verbatim.
    pub(crate) fn supports_effort(self, effort: &str) -> bool {
        let effort = effort.trim().to_ascii_lowercase();
        match self {
            Self::Disabled => false,
            Self::Budget | Self::Adaptive => matches!(
                effort.as_str(),
                "minimal" | "low" | "medium" | "high" | "xhigh" | "max" | "ultra"
            ),
        }
    }
}
