use super::*;

fn token(value: u8) -> Token {
    Token::new([value; 32])
}

fn issue(start: &Start<'_>, state: u8, now: Instant) -> bool {
    start.issue(
        || now,
        token(state),
        Nonce::new("fixture".into()),
        PkceCodeVerifier::new("fixture".into()),
        LoginContext {
            revision: 1,
            deadline_ms: 180_000,
        },
    )
}

#[test]
fn browser_budget_preserves_pending_and_reopens_at_its_own_boundary() {
    let a = Attempts::default();
    let now = Instant::now();
    for n in 0..MAX_BROWSER_STARTS {
        let s = a.start(Some(token(1)), token(1), || now).unwrap();
        assert!(issue(&s, n as u8, now));
    }
    let short = Duration::from_millis(1);
    let last = now + BROWSER_WINDOW - short;
    assert!(matches!(
        a.start(Some(token(1)), token(2), || last),
        Err(wait) if wait == short
    ));
    let other = a.start(None, token(2), || last).unwrap();
    assert!(issue(&other, 2, last));
    drop(other);
    assert!(a.claim(&token(2), &token(2), true, || last).is_ok());
    let latest_state = token((MAX_BROWSER_STARTS - 1) as u8);
    assert!(a.claim(&token(1), &latest_state, true, || last).is_ok());
    assert!(
        a.start(Some(token(1)), token(2), || now + BROWSER_WINDOW)
            .is_ok()
    );
    assert_eq!(a.book.lock().unwrap().starts.len(), MAX_BROWSER_STARTS + 2);
}

#[test]
fn process_window_bounds_storage_and_delays_only_new_starts() {
    let a = Attempts::default();
    let now = Instant::now();
    for n in 1..=MAX_STARTS as u8 {
        let start = a.start(Some(token(0)), token(n), || now).unwrap();
        assert_eq!(*start.cookie, *token(n)); // Unknown caller-supplied handles are not adopted.
        assert!(issue(&start, n, now));
    }
    {
        let book = a.book.lock().unwrap();
        assert_eq!(book.starts.len(), MAX_STARTS);
        assert_eq!(book.pending.len(), MAX_STARTS);
    }
    let tick = Duration::from_nanos(1);
    let last = now + LIFE - tick;
    assert!(matches!(a.start(None, token(99), || last), Err(wait) if wait == tick));
    assert!(a.claim(&token(1), &token(1), true, || last).is_ok());
    assert!(matches!(
        a.claim(&token(2), &token(2), true, || now + LIFE),
        Err(ClaimError::Denied)
    ));
    let fresh = a.start(None, token(99), || now + LIFE).unwrap();
    let book = a.book.lock().unwrap();
    assert_eq!(book.starts.len(), 1);
    assert!(book.pending.is_empty());
    assert!(book.preparing.contains(&*fresh.cookie));
}

#[test]
fn start_preparation_is_bounded_and_does_not_remove_existing_state_on_failure() {
    let a = Attempts::default();
    let now = Instant::now();
    let first = a.start(None, token(1), || now).unwrap();
    assert!(issue(&first, 1, now));
    assert!(matches!(
        a.start(Some(token(1)), token(8), || now),
        Err(wait) if wait == RETRY_SOON
    ));
    drop(first);
    let preparing: Vec<_> = (1..=MAX_PREPARING as u8)
        .map(|n| a.start(Some(token(n)), token(n), || now).unwrap())
        .collect();
    assert!(matches!(
        a.start(None, token(9), || now),
        Err(wait) if wait == RETRY_SOON
    ));
    drop(preparing);
    assert!(a.claim(&token(1), &token(1), true, || now).is_ok());
    let next = a.start(None, token(9), || now).unwrap();
    assert!(!issue(&next, 9, now + LIFE));
}

#[test]
fn busy_exchange_does_not_consume_state_or_block_valid_denials() {
    let a = Attempts::default();
    let now = Instant::now();
    let next = MAX_EXCHANGES as u8 + 1;
    for n in 1..=next + 1 {
        let start = a.start(None, token(n), || now).unwrap();
        assert!(issue(&start, n, now));
    }
    let active: Vec<_> = (1..next)
        .map(|n| a.claim(&token(n), &token(n), true, || now).ok().unwrap())
        .collect();
    assert!(matches!(
        a.claim(&token(next), &token(0), true, || now),
        Err(ClaimError::Denied)
    ));
    assert!(matches!(
        a.claim(&token(next), &token(next), true, || now),
        Err(ClaimError::Busy)
    ));
    assert!(
        a.claim(&token(next + 1), &token(next + 1), false, || now)
            .is_ok()
    );
    drop(active);
    assert!(a.claim(&token(next), &token(next), true, || now).is_ok());
    assert!(matches!(
        a.claim(&token(next), &token(next), true, || now),
        Err(ClaimError::Denied)
    ));
}

#[test]
fn competing_callbacks_claim_only_once() {
    let a = Attempts::default();
    let now = Instant::now();
    assert!(issue(&a.start(None, token(1), || now).unwrap(), 2, now));
    let barrier = std::sync::Barrier::new(2);
    std::thread::scope(|scope| {
        let callback = || {
            barrier.wait();
            a.claim(&token(1), &token(2), true, || now).is_ok()
        };
        let first = scope.spawn(callback);
        assert_ne!(callback(), first.join().unwrap());
    });
}
