#[test]
fn usage_total_columns_keep_the_shared_activity_prefix_and_local_speed_guard() {
    let server =
        crate::usage_total_columns_sql!("MAX(COALESCE(output_tokens, 0), 0) <= latency_ms");
    let desktop = crate::usage_total_columns_sql!("COALESCE(output_tokens, 0) <= latency_ms");
    let marker = "COALESCE(SUM(total_tokens), 0)";
    let server_at = server.find(marker).unwrap() + marker.len();
    let desktop_at = desktop.find(marker).unwrap() + marker.len();
    assert_eq!(&server[..server_at], &desktop[..desktop_at]);
    assert_eq!(
        &server[server_at..],
        ", COALESCE(SUM(CASE WHEN success != 0 AND COALESCE(output_tokens, 0) > 0 AND latency_ms > 0 AND MAX(COALESCE(output_tokens, 0), 0) <= latency_ms THEN MAX(COALESCE(output_tokens, 0), 0) ELSE 0 END), 0), COALESCE(SUM(CASE WHEN success != 0 AND COALESCE(output_tokens, 0) > 0 AND latency_ms > 0 AND MAX(COALESCE(output_tokens, 0), 0) <= latency_ms THEN latency_ms ELSE 0 END), 0)"
    );
    assert_eq!(
        &desktop[desktop_at..],
        ", COALESCE(SUM(CASE WHEN success != 0 AND COALESCE(output_tokens, 0) > 0 AND latency_ms > 0 AND COALESCE(output_tokens, 0) <= latency_ms THEN MAX(COALESCE(output_tokens, 0), 0) ELSE 0 END), 0), COALESCE(SUM(CASE WHEN success != 0 AND COALESCE(output_tokens, 0) > 0 AND latency_ms > 0 AND COALESCE(output_tokens, 0) <= latency_ms THEN latency_ms ELSE 0 END), 0)"
    );
}

#[test]
fn priced_aggregate_reader_keeps_nulls_and_skips_the_combined_write_column() {
    let sums = super::ObservedUsageSums::from_priced_aggregate(
        |offset| {
            assert_ne!(offset, 2, "combined cache-write column is not priced");
            Ok::<_, ()>(match offset {
                0 => Some(10),
                1 => Some(-1),
                3 => Some(5),
                4 => None,
                5 => Some(1),
                6 => Some(7),
                7 => Some(8),
                _ => panic!("unexpected token offset {offset}"),
            })
        },
        |offset| {
            Ok(match offset {
                8 => 4,
                9 => 4,
                10 => 2,
                11 => -3,
                12 => 1,
                _ => panic!("unexpected sample offset {offset}"),
            })
        },
    )
    .unwrap();
    assert_eq!(sums.input_tokens, Some(10));
    assert_eq!(sums.cached_input_tokens, None);
    assert_eq!(sums.cache_write_5m_tokens, Some(5));
    assert_eq!(sums.cache_write_1h_tokens, None);
    assert_eq!(sums.unknown_cache_write_tokens, Some(1));
    assert_eq!(sums.output_tokens, Some(7));
    assert_eq!(sums.total_tokens, Some(8));
    assert_eq!(sums.input_samples, 4);
    assert_eq!(sums.cached_samples, 4);
    assert_eq!(sums.cache_write_samples, 2);
    assert_eq!(sums.output_samples, 0);
    assert_eq!(sums.total_samples, 1);
    assert!(sums.gate_measured_buckets);
}

#[test]
fn rollup_reader_does_not_require_output_or_total_sample_counts() {
    let sums = super::ObservedUsageSums::from_rollup_aggregate(
        |offset| Ok::<_, ()>(Some(i64::try_from(offset).unwrap())),
        |offset| {
            assert!(offset < 11, "rollup rows have no output or total samples");
            Ok(3)
        },
    )
    .unwrap();
    assert_eq!(sums.output_samples, 0);
    assert_eq!(sums.total_samples, 0);
    assert!(!sums.gate_measured_buckets);
    assert_eq!(sums.input_samples, 3);
}
