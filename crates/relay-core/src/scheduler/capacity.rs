use serde::{Deserialize, Serialize};
use std::cmp::Ordering;

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "state", content = "remaining")]
pub enum CandidateQuota {
    #[default]
    Unknown,
    Available(u64),
    Exhausted,
    Stale,
}

impl CandidateQuota {
    pub(crate) fn is_eligible(self) -> bool {
        matches!(self, Self::Unknown | Self::Available(1..))
    }

    pub(crate) fn compare_preference(self, other: Self) -> Ordering {
        match (self, other) {
            (Self::Available(left), Self::Available(right)) => left.cmp(&right),
            (Self::Available(_), _) => Ordering::Greater,
            (_, Self::Available(_)) => Ordering::Less,
            _ => Ordering::Equal,
        }
    }
}
