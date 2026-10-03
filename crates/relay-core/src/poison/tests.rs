use super::*;
use std::sync::{Mutex, RwLock};

#[test]
fn poisoned_locks_still_return_their_value() {
    let values = Mutex::new(7_u32);
    let _ = std::panic::catch_unwind(|| {
        let _guard = values.lock().expect("mutex");
        panic!("holder panicked");
    });
    assert!(values.is_poisoned());
    assert_eq!(*mutex(&values), 7);
    assert_eq!(*try_mutex(&values).expect("poisoned mutex"), 7);
    {
        let guard = mutex(&values);
        assert!(try_mutex(&values).is_none());
        assert_eq!(*guard, 7);
    }

    let shared = RwLock::new(String::from("quota"));
    let _ = std::panic::catch_unwind(|| {
        let _guard = shared.write().expect("write");
        panic!("holder panicked");
    });
    assert!(shared.is_poisoned());
    assert_eq!(read(&shared).as_str(), "quota");
    write(&shared).push_str("-ok");
    assert_eq!(read(&shared).as_str(), "quota-ok");
}
