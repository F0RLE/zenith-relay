use std::{
    cmp::Reverse,
    collections::{BinaryHeap, HashMap},
    fmt,
};

const MAX_ACCOUNT_ID_BYTES: usize = 256;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QuotaRefreshQueueError {
    InvalidCapacity,
    InvalidAccountId,
    CapacityExceeded,
}

impl fmt::Display for QuotaRefreshQueueError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidCapacity => "quota refresh queue capacity must be positive",
            Self::InvalidAccountId => "quota refresh account id is invalid",
            Self::CapacityExceeded => "quota refresh queue capacity exceeded",
        })
    }
}

impl std::error::Error for QuotaRefreshQueueError {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QuotaRefreshPermit {
    pub account_id: String,
    pub due_at_ms: u64,
    pub claimed_at_ms: u64,
    generation: u64,
}

#[derive(Clone, Debug)]
pub struct QuotaRefreshQueue {
    max_entries: usize,
    next_generation: u64,
    entries: HashMap<String, QueueEntry>,
    heap: BinaryHeap<Reverse<HeapEntry>>,
}

impl QuotaRefreshQueue {
    pub fn new(max_entries: usize) -> Result<Self, QuotaRefreshQueueError> {
        if max_entries == 0 {
            return Err(QuotaRefreshQueueError::InvalidCapacity);
        }
        Ok(Self {
            max_entries,
            next_generation: 1,
            entries: HashMap::new(),
            heap: BinaryHeap::new(),
        })
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn upsert(
        &mut self,
        account_id: &str,
        due_at_ms: u64,
    ) -> Result<bool, QuotaRefreshQueueError> {
        let account_id = validated_account_id(account_id)?;
        let Some(status) = self.entries.get(account_id).map(|entry| entry.status) else {
            return self.insert(account_id, due_at_ms);
        };
        if status == QueueStatus::InFlight {
            let entry = self
                .entries
                .get_mut(account_id)
                .expect("existing quota refresh entry disappeared");
            let changed = entry.dirty_due_at_ms != Some(due_at_ms);
            entry.dirty_due_at_ms = Some(due_at_ms);
            return Ok(changed);
        }
        let unchanged = self
            .entries
            .get(account_id)
            .is_some_and(|entry| entry.due_at_ms == due_at_ms && entry.dirty_due_at_ms.is_none());
        if unchanged {
            return Ok(false);
        }
        self.schedule(account_id, due_at_ms);
        Ok(true)
    }

    pub fn mark_dirty(
        &mut self,
        account_id: &str,
        due_at_ms: u64,
    ) -> Result<bool, QuotaRefreshQueueError> {
        let account_id = validated_account_id(account_id)?;
        let Some(entry) = self.entries.get(account_id) else {
            return self.insert(account_id, due_at_ms);
        };
        let current_due = match entry.status {
            QueueStatus::Pending => Some(entry.due_at_ms),
            QueueStatus::InFlight => entry.dirty_due_at_ms,
        };
        if current_due.is_some_and(|current| current <= due_at_ms) {
            return Ok(false);
        }
        if entry.status == QueueStatus::InFlight {
            self.entries
                .get_mut(account_id)
                .expect("existing quota refresh entry disappeared")
                .dirty_due_at_ms = Some(due_at_ms);
        } else {
            self.schedule(account_id, due_at_ms);
        }
        Ok(true)
    }

    pub fn claim_due(&mut self, now_ms: u64, max_claims: usize) -> Vec<QuotaRefreshPermit> {
        let mut permits = Vec::with_capacity(max_claims.min(self.entries.len()));
        while permits.len() < max_claims {
            self.discard_stale();
            let Some(Reverse(next)) = self.heap.peek() else {
                break;
            };
            if next.due_at_ms > now_ms {
                break;
            }
            let Reverse(next) = self
                .heap
                .pop()
                .expect("peeked quota refresh entry vanished");
            let entry = self
                .entries
                .get_mut(&next.account_id)
                .expect("live quota refresh entry vanished");
            entry.status = QueueStatus::InFlight;
            permits.push(QuotaRefreshPermit {
                account_id: next.account_id,
                due_at_ms: next.due_at_ms,
                claimed_at_ms: now_ms,
                generation: next.generation,
            });
        }
        permits
    }

    pub fn complete(&mut self, permit: QuotaRefreshPermit) -> bool {
        let Some(entry) = self.entries.get(&permit.account_id) else {
            return false;
        };
        if entry.status != QueueStatus::InFlight || entry.generation != permit.generation {
            return false;
        }
        if let Some(due_at_ms) = entry.dirty_due_at_ms {
            self.schedule(&permit.account_id, due_at_ms);
        } else {
            self.entries.remove(&permit.account_id);
        }
        true
    }

    pub fn reschedule(&mut self, permit: QuotaRefreshPermit, due_at_ms: u64) -> bool {
        let Some(entry) = self.entries.get(&permit.account_id) else {
            return false;
        };
        if entry.status != QueueStatus::InFlight || entry.generation != permit.generation {
            return false;
        }
        let due_at_ms = entry
            .dirty_due_at_ms
            .map_or(due_at_ms, |dirty_due| dirty_due.min(due_at_ms));
        self.schedule(&permit.account_id, due_at_ms);
        true
    }

    pub fn remove(&mut self, account_id: &str) -> bool {
        self.entries.remove(account_id).is_some()
    }

    pub fn next_due(&mut self) -> Option<u64> {
        self.discard_stale();
        self.heap.peek().map(|entry| entry.0.due_at_ms)
    }

    fn insert(&mut self, account_id: &str, due_at_ms: u64) -> Result<bool, QuotaRefreshQueueError> {
        if self.entries.len() >= self.max_entries {
            return Err(QuotaRefreshQueueError::CapacityExceeded);
        }
        let generation = self.take_generation();
        self.entries.insert(
            account_id.to_string(),
            QueueEntry {
                status: QueueStatus::Pending,
                due_at_ms,
                generation,
                dirty_due_at_ms: None,
            },
        );
        self.heap.push(Reverse(HeapEntry {
            due_at_ms,
            generation,
            account_id: account_id.to_string(),
        }));
        self.compact_if_needed();
        Ok(true)
    }

    fn schedule(&mut self, account_id: &str, due_at_ms: u64) {
        let generation = self.take_generation();
        let entry = self
            .entries
            .get_mut(account_id)
            .expect("quota refresh schedule requires an existing entry");
        entry.status = QueueStatus::Pending;
        entry.due_at_ms = due_at_ms;
        entry.generation = generation;
        entry.dirty_due_at_ms = None;
        self.heap.push(Reverse(HeapEntry {
            due_at_ms,
            generation,
            account_id: account_id.to_string(),
        }));
        self.compact_if_needed();
    }

    fn take_generation(&mut self) -> u64 {
        let generation = self.next_generation;
        self.next_generation = self.next_generation.wrapping_add(1).max(1);
        generation
    }

    fn discard_stale(&mut self) {
        while self.heap.peek().is_some_and(|entry| {
            let entry = &entry.0;
            !self.entries.get(&entry.account_id).is_some_and(|state| {
                state.status == QueueStatus::Pending
                    && state.generation == entry.generation
                    && state.due_at_ms == entry.due_at_ms
            })
        }) {
            self.heap.pop();
        }
    }

    fn compact_if_needed(&mut self) {
        if self.heap.len() <= self.max_entries.saturating_mul(2) {
            return;
        }
        self.heap = self
            .entries
            .iter()
            .filter(|(_, entry)| entry.status == QueueStatus::Pending)
            .map(|(account_id, entry)| {
                Reverse(HeapEntry {
                    due_at_ms: entry.due_at_ms,
                    generation: entry.generation,
                    account_id: account_id.clone(),
                })
            })
            .collect();
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum QueueStatus {
    Pending,
    InFlight,
}

#[derive(Clone, Debug)]
struct QueueEntry {
    status: QueueStatus,
    due_at_ms: u64,
    generation: u64,
    dirty_due_at_ms: Option<u64>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct HeapEntry {
    due_at_ms: u64,
    generation: u64,
    account_id: String,
}

fn validated_account_id(account_id: &str) -> Result<&str, QuotaRefreshQueueError> {
    let trimmed = account_id.trim();
    if trimmed.is_empty()
        || trimmed.len() > MAX_ACCOUNT_ID_BYTES
        || trimmed != account_id
        || trimmed.chars().any(char::is_control)
    {
        Err(QuotaRefreshQueueError::InvalidAccountId)
    } else {
        Ok(trimmed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upsert_dedupes_and_discards_stale_heap_entries() {
        let mut queue = QuotaRefreshQueue::new(2).unwrap();
        assert!(queue.upsert("account-1", 10).unwrap());
        assert!(queue.upsert("account-1", 30).unwrap());
        assert_eq!(queue.len(), 1);
        assert_eq!(queue.next_due(), Some(30));
        assert!(queue.claim_due(29, 1).is_empty());
        assert_eq!(queue.claim_due(30, 2).len(), 1);
        assert!(queue.claim_due(u64::MAX, 1).is_empty());
    }

    #[test]
    fn dirty_in_flight_entry_is_requeued_after_completion() {
        let mut queue = QuotaRefreshQueue::new(1).unwrap();
        queue.upsert("account-1", 10).unwrap();
        let permit = queue.claim_due(10, 1).pop().unwrap();
        assert!(queue.mark_dirty("account-1", 12).unwrap());
        assert!(queue.complete(permit));
        assert_eq!(queue.next_due(), Some(12));
        assert_eq!(queue.claim_due(12, 1)[0].account_id, "account-1");
    }

    #[test]
    fn reschedule_honors_earlier_dirty_deadline_and_remove_invalidates_permit() {
        let mut queue = QuotaRefreshQueue::new(1).unwrap();
        queue.upsert("account-1", 10).unwrap();
        let permit = queue.claim_due(10, 1).pop().unwrap();
        queue.mark_dirty("account-1", 20).unwrap();
        assert!(queue.reschedule(permit, 30));
        assert_eq!(queue.next_due(), Some(20));
        let permit = queue.claim_due(20, 1).pop().unwrap();
        assert!(queue.remove("account-1"));
        assert!(!queue.complete(permit));
        assert!(queue.next_due().is_none());
    }

    #[test]
    fn queue_enforces_capacity_and_claim_limit_in_due_order() {
        let mut queue = QuotaRefreshQueue::new(2).unwrap();
        queue.upsert("later", 20).unwrap();
        queue.upsert("earlier", 10).unwrap();
        assert_eq!(
            queue.upsert("overflow", 0),
            Err(QuotaRefreshQueueError::CapacityExceeded)
        );
        let first = queue.claim_due(20, 1);
        assert_eq!(first[0].account_id, "earlier");
        let second = queue.claim_due(20, 1);
        assert_eq!(second[0].account_id, "later");
    }

    #[test]
    fn stale_heap_storage_remains_bounded() {
        let mut queue = QuotaRefreshQueue::new(2).unwrap();
        queue.upsert("account-1", 0).unwrap();
        for due_at_ms in 1..1_000 {
            queue.upsert("account-1", due_at_ms).unwrap();
        }
        assert!(queue.heap.len() <= 4);
        assert_eq!(queue.next_due(), Some(999));
    }

    #[test]
    fn invalid_bounds_and_ids_are_rejected() {
        assert_eq!(
            QuotaRefreshQueue::new(0).unwrap_err().to_string(),
            "quota refresh queue capacity must be positive"
        );
        let mut queue = QuotaRefreshQueue::new(1).unwrap();
        assert_eq!(
            queue.upsert(" account-1", 0),
            Err(QuotaRefreshQueueError::InvalidAccountId)
        );
    }
}
