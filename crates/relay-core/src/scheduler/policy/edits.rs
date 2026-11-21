use super::{PoolMemberKind, PoolRoutingMode, PoolRoutingPolicy};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RotationPreset {
    Recommended,
    Even,
    AccountsFirst,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RotationMemberField {
    Weight,
    MaxConcurrency,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RotationPlacement {
    Before,
    After,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
pub enum PoolRoutingEdit {
    Mode {
        mode: PoolRoutingMode,
    },
    Preset {
        preset: RotationPreset,
    },
    Move {
        member: String,
        target: String,
        placement: RotationPlacement,
    },
    Member {
        member: String,
        field: RotationMemberField,
        value: u32,
    },
}

impl PoolRoutingPolicy {
    pub fn rotation_preset(&self) -> Option<RotationPreset> {
        if self
            .members
            .iter()
            .any(|member| member.max_concurrency != 0)
        {
            return None;
        }
        match self.mode {
            PoolRoutingMode::Automatic
                if self
                    .members
                    .windows(2)
                    .all(|pair| pair[0].weight == pair[1].weight) =>
            {
                Some(RotationPreset::Recommended)
            }
            PoolRoutingMode::RoundRobin => Some(RotationPreset::Even),
            PoolRoutingMode::Failover
                if self
                    .members
                    .windows(2)
                    .all(|pair| pair[0].kind <= pair[1].kind) =>
            {
                Some(RotationPreset::AccountsFirst)
            }
            _ => None,
        }
    }

    /// Apply intent to current inventory. Missing identities are ignored when
    /// an editor rebases after removal; an edit never recreates membership.
    pub fn apply_edits(&self, edits: &[PoolRoutingEdit]) -> Result<Self, &'static str> {
        self.validate_activation()?;
        if edits.len() > 256 {
            return Err("too many pool routing edits");
        }
        let mut policy = self.clone();
        let key = |kind, id: &str| {
            let kind = match kind {
                PoolMemberKind::Account => "account",
                PoolMemberKind::Source => "source",
            };
            format!("{kind}:{id}")
        };
        for edit in edits {
            match edit {
                PoolRoutingEdit::Mode { mode } => policy.mode = *mode,
                PoolRoutingEdit::Preset { preset } => {
                    policy.mode = match preset {
                        RotationPreset::Recommended => PoolRoutingMode::Automatic,
                        RotationPreset::Even => PoolRoutingMode::RoundRobin,
                        RotationPreset::AccountsFirst => PoolRoutingMode::Failover,
                    };
                    for member in &mut policy.members {
                        member.weight = 1;
                        member.max_concurrency = 0;
                    }
                    if *preset == RotationPreset::AccountsFirst {
                        policy.members.sort_by_key(|member| member.kind);
                    }
                }
                PoolRoutingEdit::Member {
                    member,
                    field,
                    value,
                } => {
                    let valid = match field {
                        RotationMemberField::Weight => (1..=100).contains(value),
                        RotationMemberField::MaxConcurrency => *value <= 1024,
                    };
                    if !valid {
                        return Err("pool member weight or concurrency limit is invalid");
                    }
                    if let Some(rule) = policy
                        .members
                        .iter_mut()
                        .find(|rule| key(rule.kind, &rule.id) == *member)
                    {
                        match field {
                            RotationMemberField::Weight => rule.weight = *value,
                            RotationMemberField::MaxConcurrency => rule.max_concurrency = *value,
                        }
                    }
                }
                PoolRoutingEdit::Move {
                    member,
                    target,
                    placement,
                } => {
                    if policy.mode.is_adaptive() || member == target {
                        continue;
                    }
                    let position = |id: &str| {
                        policy
                            .members
                            .iter()
                            .position(|rule| key(rule.kind, &rule.id) == id)
                    };
                    if let (Some(from), Some(to)) = (position(member), position(target)) {
                        let rule = policy.members.remove(from);
                        let index = to - usize::from(from < to)
                            + usize::from(matches!(placement, RotationPlacement::After));
                        policy.members.insert(index, rule);
                    }
                }
            }
        }
        policy.validate_activation()?;
        Ok(policy)
    }
}

#[cfg(test)]
mod tests;
