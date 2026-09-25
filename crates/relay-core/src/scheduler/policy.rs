use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

mod upgrade;

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PoolRoutingMode {
    /// Compatibility-only value for reading a pre-1.1.3 policy. It is
    /// upgraded before a policy can reach the scheduler.
    Smart,
    #[default]
    Automatic,
    InOrder,
    RoundRobin,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PoolMemberKind {
    Account,
    Source,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PoolRoutingMember {
    pub kind: PoolMemberKind,
    pub id: String,
    pub weight: u32,
    /// Zero leaves capacity unrestricted by this policy.
    pub max_concurrency: u32,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PoolRoutingPolicy {
    pub version: u8,
    pub mode: PoolRoutingMode,
    pub members: Vec<PoolRoutingMember>,
}

impl Default for PoolRoutingPolicy {
    fn default() -> Self {
        Self {
            version: 2,
            mode: PoolRoutingMode::Automatic,
            members: Vec::new(),
        }
    }
}

impl PoolRoutingPolicy {
    pub fn is_current_rotation(&self) -> bool {
        self.version == 2 && self.mode != PoolRoutingMode::Smart
    }

    pub fn validate_activation(&self) -> Result<(), &'static str> {
        self.validate()?;
        if !self.is_current_rotation() {
            return Err("unsupported runtime rotation policy version");
        }
        Ok(())
    }

    pub fn remap_member_ids(
        &mut self,
        ids: &BTreeMap<(PoolMemberKind, String), String>,
    ) -> Result<(), &'static str> {
        let mut remapped = self.clone();
        for member in &mut remapped.members {
            member.id = ids
                .get(&(member.kind, member.id.clone()))
                .ok_or("pool routing references a member missing from the preset")?
                .clone();
        }
        remapped.validate()?;
        *self = remapped;
        Ok(())
    }

    pub fn validate_update(
        &self,
        current: &Self,
        expected: Option<&Self>,
    ) -> Result<(), &'static str> {
        self.validate()?;
        if self.version != current.version {
            return Err("unsupported pool routing policy version for this runtime");
        }
        if expected != Some(current) {
            return Err("pool routing changed; reload the current policy before saving");
        }
        let identities = |policy: &Self| {
            policy
                .members
                .iter()
                .map(|m| (m.kind, m.id.clone()))
                .collect::<BTreeSet<_>>()
        };
        if identities(self) != identities(current) {
            return Err("pool membership changed; reload the current policy before saving");
        }
        Ok(())
    }
    pub fn validate(&self) -> Result<(), &'static str> {
        if !matches!(self.version, 1 | 2)
            || (self.version == 1 && self.mode == PoolRoutingMode::Automatic)
            || (self.version == 2 && self.mode == PoolRoutingMode::Smart)
        {
            return Err("unsupported pool routing policy version");
        }
        if self.members.len() > 4096 {
            return Err("pool routing policy has too many members");
        }
        let mut seen = BTreeSet::new();
        for member in &self.members {
            if member.id.trim().is_empty()
                || member.id.trim() != member.id
                || member.id.len() > 256
                || member.id.chars().any(char::is_control)
            {
                return Err("pool routing member id is invalid");
            }
            if !seen.insert((member.kind, &member.id)) {
                return Err("pool routing members must be unique");
            }
            if !(1..=100).contains(&member.weight) || member.max_concurrency > 1024 {
                return Err("pool member weight or concurrency limit is invalid");
            }
        }
        Ok(())
    }

    /// Reconcile inventory, never transient health: disabled and exhausted
    /// members keep their position, while new members append exactly once.
    pub fn reconcile(&self, members: Vec<PoolRoutingMember>) -> Self {
        let known: BTreeSet<_> = members.iter().map(|m| (m.kind, m.id.as_str())).collect();
        let mut result = self.clone();
        result
            .members
            .retain(|m| known.contains(&(m.kind, m.id.as_str())));
        let existing: BTreeSet<_> = result
            .members
            .iter()
            .map(|m| (m.kind, m.id.clone()))
            .collect();
        result.members.extend(
            members
                .into_iter()
                .filter(|m| !existing.contains(&(m.kind, m.id.clone()))),
        );
        result
    }
}

pub fn resolve_pool_routing(
    saved: Option<&PoolRoutingPolicy>,
    mut inventory: Vec<(PoolMemberKind, String, i32, u32)>,
) -> PoolRoutingPolicy {
    // Legacy roles become an initial visible order, never runtime type gates.
    let tier = |kind: PoolMemberKind, priority: i32| match kind {
        PoolMemberKind::Account => 1,
        PoolMemberKind::Source if priority >= 1_000_000 => 2,
        PoolMemberKind::Source if priority <= -1_000_000 => -1,
        PoolMemberKind::Source => 0,
    };
    inventory.sort_by(|a, b| {
        tier(b.0, b.2)
            .cmp(&tier(a.0, a.2))
            .then_with(|| b.2.cmp(&a.2))
            .then_with(|| a.1.cmp(&b.1))
    });
    let members = inventory
        .into_iter()
        .map(|(kind, id, _, weight)| PoolRoutingMember {
            kind,
            id,
            weight,
            max_concurrency: 0,
        })
        .collect();
    let mut policy = saved.cloned().unwrap_or_default();
    policy.upgrade_legacy_format();
    policy.reconcile(members)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy() -> PoolRoutingPolicy {
        resolve_pool_routing(
            None,
            vec![
                (PoolMemberKind::Source, "primary".into(), 1_000_005, 3),
                (PoolMemberKind::Source, "reserve".into(), -1_000_000, 1),
                (PoolMemberKind::Account, "account".into(), 0, 1),
            ],
        )
    }

    #[test]
    fn reconciles_inventory_without_losing_custom_order_or_limits() {
        let mut saved = policy();
        saved.mode = PoolRoutingMode::InOrder;
        saved.members.swap(0, 1);
        saved.members[0].max_concurrency = 2;
        let resolved = resolve_pool_routing(
            Some(&saved),
            vec![
                (PoolMemberKind::Source, "primary".into(), -5, 1),
                (PoolMemberKind::Account, "account".into(), -10, 1),
                (PoolMemberKind::Source, "new".into(), 2_000_000, 4),
            ],
        );
        assert_eq!(
            resolved
                .members
                .iter()
                .map(|m| m.id.as_str())
                .collect::<Vec<_>>(),
            ["account", "primary", "new"]
        );
        assert_eq!(resolved.members[0].max_concurrency, 2);
        assert_eq!(resolved.members[1].weight, 3);
        assert_eq!(resolved.mode, PoolRoutingMode::InOrder);
    }

    #[test]
    fn rejects_stale_updates_and_invalid_or_changed_membership() {
        let current = policy();
        let mut next = current.clone();
        next.members.swap(0, 1);
        assert!(next.validate_update(&current, Some(&current)).is_ok());
        assert!(next.validate_update(&current, Some(&next)).is_err());
        next.members.pop();
        assert!(next.validate_update(&current, Some(&current)).is_err());
        let mut invalid = current.clone();
        invalid.members[0].weight = 0;
        assert!(invalid.validate().is_err());
        invalid = current.clone();
        invalid.members.push(current.members[0].clone());
        assert!(invalid.validate().is_err());
    }

    #[test]
    fn remaps_portable_ids_atomically_and_rejects_collisions() {
        let mut saved = policy();
        let original = saved.clone();
        let mut ids: BTreeMap<_, _> = saved
            .members
            .iter()
            .map(|m| ((m.kind, m.id.clone()), format!("local-{}", m.id)))
            .collect();
        ids.remove(&(PoolMemberKind::Account, "account".into()));
        assert!(saved.remap_member_ids(&ids).is_err());
        assert_eq!(saved, original);
        ids.insert(
            (PoolMemberKind::Account, "account".into()),
            "local-account".into(),
        );
        saved.remap_member_ids(&ids).unwrap();
        assert!(saved.members.iter().all(|m| m.id.starts_with("local-")));
        let mut collision = original.clone();
        ids.insert(
            (PoolMemberKind::Source, "reserve".into()),
            "local-primary".into(),
        );
        assert!(collision.remap_member_ids(&ids).is_err());
        assert_eq!(collision, original);
    }
}
