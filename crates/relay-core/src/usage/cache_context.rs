use serde::{Deserialize, Serialize};

/// Local request comparisons, not provider cache-hit or execution evidence.
/// Only classifications and counts leave the in-memory fingerprint store.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CacheContextDiagnostics {
    pub baseline: CacheContextBaseline,
    pub scope: CacheContextScope,
    pub client_changes: Vec<CacheContextSection>,
    pub upstream_changes: Vec<CacheContextSection>,
    pub relay_changes: Vec<CacheContextSection>,
    pub client_history: CacheHistoryDiagnostics,
    pub upstream_history: CacheHistoryDiagnostics,
    pub relay_history: CacheHistoryDiagnostics,
    pub candidate_changed: Option<bool>,
    pub previous_completed_age_ms: Option<u64>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CacheContextBaseline {
    FirstObservation,
    CompletedRequest,
    OverlappingRequests,
    Unavailable,
    SizeLimit,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CacheContextScope {
    ClientSession,
    CacheKey,
    Unavailable,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CacheContextSection {
    Model,
    Tools,
    Instructions,
    Reasoning,
    OutputFormat,
    Verbosity,
    ToolChoice,
    ParallelToolCalls,
    CacheKey,
    CachePolicy,
    ServiceTier,
    ContextManagement,
    Truncation,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CacheHistoryDiagnostics {
    pub comparison: CacheHistoryComparison,
    pub input_items: Option<u32>,
    /// Compact JSON bytes, never a token count or a billing estimate.
    pub input_bytes: Option<u64>,
    pub shared_prefix_items: Option<u32>,
    pub first_changed_item_kind: Option<CacheInputKind>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CacheHistoryComparison {
    #[default]
    NotCompared,
    Unchanged,
    Appended,
    Rewritten,
    Truncated,
    /// Delta-only input cannot be compared with a full history or another delta.
    Continuation,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CacheInputKind {
    Developer,
    User,
    Assistant,
    ToolResult,
    ToolCall,
    Reasoning,
    Compaction,
    AdditionalTools,
    ConfigurationUpdate,
    Other,
}
