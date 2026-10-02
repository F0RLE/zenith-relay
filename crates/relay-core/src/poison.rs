//! Recovers a standard-library lock after the previous holder panicked.
//! The guarded data stays available; the poison flag only records that panic.

use std::sync::{Mutex, MutexGuard, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard};

pub fn mutex<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Tries a mutex without waiting. A poisoned lock still returns its value.
/// `None` means another thread currently holds it.
pub fn try_mutex<T>(mutex: &Mutex<T>) -> Option<MutexGuard<'_, T>> {
    match mutex.try_lock() {
        Ok(guard) => Some(guard),
        Err(std::sync::TryLockError::Poisoned(poisoned)) => Some(poisoned.into_inner()),
        Err(std::sync::TryLockError::WouldBlock) => None,
    }
}

pub fn read<T>(lock: &RwLock<T>) -> RwLockReadGuard<'_, T> {
    lock.read().unwrap_or_else(PoisonError::into_inner)
}

pub fn write<T>(lock: &RwLock<T>) -> RwLockWriteGuard<'_, T> {
    lock.write().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
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
}
