use super::*;
use serde_json::json;

#[test]
fn cached_validator_keeps_the_original_contract_and_detects_schema_changes() {
    let schema = json!({"type":"object","properties":{"query":{"type":"string","minLength":2}},"required":["query"],"additionalProperties":false});
    let first = validator(&schema).unwrap();
    assert!(Arc::ptr_eq(&first, &validator(&schema).unwrap()));
    assert!(first.is_valid(&json!({"query":"ok"})));
    assert!(!first.is_valid(&json!({"query":"x"})));
    assert!(!first.is_valid(&json!({"query":"ok","extra":true})));
    let mut changed = schema.clone();
    changed["properties"]["query"]["minLength"] = json!(3);
    let changed_validator = validator(&changed).unwrap();
    assert!(!Arc::ptr_eq(&first, &changed_validator));
    assert!(!changed_validator.is_valid(&json!({"query":"ok"})));
    assert!(first.is_valid(&json!({"query":"ok"})));
}

#[test]
fn large_schemas_bypass_cache_without_losing_validation() {
    let schema = json!({"type":"integer","description":"x".repeat(MAX_SCHEMA_BYTES)});
    let first = validator(&schema).unwrap();
    let second = validator(&schema).unwrap();
    assert!(!Arc::ptr_eq(&first, &second));
    assert!(second.is_valid(&json!(1)));
    assert!(!second.is_valid(&json!("1")));
}

#[test]
fn concurrent_compilation_reuses_one_retained_validator() {
    let barrier = Arc::new(std::sync::Barrier::new(4));
    let handles: Vec<_> = (0..4)
        .map(|_| {
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                barrier.wait();
                validator(&json!({"type":"string","const":"synthetic-concurrent-schema"})).unwrap()
            })
        })
        .collect();
    let validators: Vec<_> = handles
        .into_iter()
        .map(|handle| handle.join().unwrap())
        .collect();
    assert!(validators
        .iter()
        .all(|value| Arc::ptr_eq(&validators[0], value)));
}

fn entry(schema_bytes: usize, now: Instant) -> Entry {
    Entry {
        validator: Arc::new(jsonschema::options().offline().build(&json!(true)).unwrap()),
        schema_bytes,
        last_used: now,
    }
}

#[test]
fn cache_expires_idle_entries_but_refreshes_active_entries() {
    let mut cache = ValidatorCache::default();
    let now = Instant::now();
    cache.insert([0; 32], entry(10, now), now);
    cache.insert([1; 32], entry(10, now), now);
    assert!(cache.get(&[0; 32], now + IDLE_TTL / 2).is_some());
    let later = now + IDLE_TTL;
    assert!(cache.get(&[1; 32], later).is_none());
    cache.insert([2; 32], entry(10, later), later);
    assert!(!cache.entries.contains_key(&[1; 32]));
    assert!(cache.get(&[0; 32], later).is_some());
    assert!(cache.get(&[0; 32], later + IDLE_TTL).is_none());
}

#[test]
fn cache_bounds_entries_and_schema_bytes_and_evicts_least_recently_used() {
    for schema_bytes in [10, MAX_SCHEMA_BYTES] {
        let mut cache = ValidatorCache::default();
        let now = Instant::now();
        let limit = MAX_ENTRIES.min(MAX_RETAINED_SCHEMA_BYTES / schema_bytes);
        for index in 0..limit {
            let key = Sha256::digest(index.to_le_bytes()).into();
            let time = now + Duration::from_millis(index as u64);
            cache.insert(key, entry(schema_bytes, time), time);
        }
        let oldest = Sha256::digest(0_usize.to_le_bytes()).into();
        let second: [u8; 32] = Sha256::digest(1_usize.to_le_bytes()).into();
        let later = now + Duration::from_secs(1);
        assert!(cache.get(&oldest, later).is_some());
        cache.insert([255; 32], entry(schema_bytes, later), later);
        assert!(cache.entries.contains_key(&oldest));
        assert!(!cache.entries.contains_key(&second));
        assert_eq!(cache.entries.len(), limit);
        assert!(
            cache
                .entries
                .values()
                .map(|entry| entry.schema_bytes)
                .sum::<usize>()
                <= MAX_RETAINED_SCHEMA_BYTES
        );
    }
}

#[test]
#[ignore = "local validator compilation benchmark; no provider requests"]
fn repeated_catalog_validation_latency() {
    let schemas: Vec<_> = (0..296)
        .map(|index| {
            json!({
                "type":"object", "title":format!("synthetic-tool-{index}"),
                "properties":{
                    "query":{"type":"string","minLength":1},
                    "limit":{"type":"integer","minimum":1,"maximum":100},
                    "options":{"type":"array","items":{"enum":["a","b","c"]}}
                }, "required":["query"], "additionalProperties":false
            })
        })
        .collect();
    let cold_start = Instant::now();
    for schema in &schemas {
        validator(schema).unwrap();
    }
    let cold = cold_start.elapsed();
    let start = Instant::now();
    for _ in 0..10 {
        for schema in &schemas {
            std::hint::black_box(jsonschema::options().offline().build(schema).unwrap());
        }
    }
    let uncached = start.elapsed();
    let start = Instant::now();
    for _ in 0..10 {
        for schema in &schemas {
            std::hint::black_box(validator(schema).unwrap());
        }
    }
    eprintln!(
        "296 schemas: first cold pass={cold:?}; 10 passes: compile={uncached:?}, cached={:?}",
        start.elapsed()
    );
}
