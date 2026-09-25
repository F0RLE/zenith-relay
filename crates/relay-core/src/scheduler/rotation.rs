//! pool rotation: one deterministic selection and attempt lifecycle.
//! Transport drivers provide execution evidence; this module never sends HTTP.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

pub const DEFAULT_MAX_DISPATCHES: u8 = 3;
static NEXT_REQUEST_ID: AtomicU64 = AtomicU64::new(1);
const CIRCUIT_FAILURE_THRESHOLD: u32 = 3;
const FIRST_TRANSIENT_PACING_MS: u64 = 250;
const SECOND_TRANSIENT_PACING_MS: u64 = 500;
const OPEN_BACKOFF_MS: u64 = 2_000;
const MAX_OPEN_BACKOFF_MS: u64 = 60_000;
const FAILURE_WINDOW_MS: u64 = 60_000;

mod budget;
mod engine;
mod types;

pub use budget::*;
pub use engine::RotationEngine;
pub use types::*;
