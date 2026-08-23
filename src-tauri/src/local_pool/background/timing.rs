use std::time::Duration;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum DueWait {
    Ready,
    Notify,
    Sleep(Duration),
}

pub(super) fn due_wait(next_due_at_ms: Option<u64>, now_ms: u64) -> DueWait {
    match next_due_at_ms {
        None => DueWait::Notify,
        Some(due_at_ms) if due_at_ms <= now_ms => DueWait::Ready,
        Some(due_at_ms) => DueWait::Sleep(Duration::from_millis(due_at_ms - now_ms)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn due_wait_uses_deadline_or_notification_without_polling() {
        assert_eq!(due_wait(None, 100), DueWait::Notify);
        assert_eq!(due_wait(Some(100), 100), DueWait::Ready);
        assert_eq!(due_wait(Some(90), 100), DueWait::Ready);
        assert_eq!(
            due_wait(Some(150), 100),
            DueWait::Sleep(Duration::from_millis(50))
        );
    }
}
