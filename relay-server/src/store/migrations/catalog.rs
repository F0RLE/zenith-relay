pub(super) const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        name: "001_init",
        sql: include_str!("../../../migrations/001_init.sql"),
    },
    Migration {
        version: 2,
        name: "002_migration_ledger",
        sql: include_str!("../../../migrations/002_migration_ledger.sql"),
    },
    Migration {
        version: 3,
        name: "003_usage_query_indexes",
        sql: include_str!("../../../migrations/003_usage_query_indexes.sql"),
    },
    Migration {
        version: 4,
        name: "004_account_proxies",
        sql: include_str!("../../../migrations/004_account_proxies.sql"),
    },
    Migration {
        version: 5,
        name: "005_pool_membership",
        sql: include_str!("../../../migrations/005_pool_membership.sql"),
    },
    Migration {
        version: 6,
        name: "006_model_rules",
        sql: include_str!("../../../migrations/006_model_rules.sql"),
    },
    Migration {
        version: 7,
        name: "007_cached_input_tokens",
        sql: include_str!("../../../migrations/007_cached_input_tokens.sql"),
    },
    Migration {
        version: 8,
        name: "008_reasoning_tokens",
        sql: include_str!("../../../migrations/008_reasoning_tokens.sql"),
    },
    Migration {
        version: 9,
        name: "009_ttft_ms",
        sql: include_str!("../../../migrations/009_ttft_ms.sql"),
    },
    Migration {
        version: 10,
        name: "010_reset_legacy_cooldowns",
        sql: include_str!("../../../migrations/010_reset_legacy_cooldowns.sql"),
    },
    Migration {
        version: 11,
        name: "011_request_rotation_default",
        sql: include_str!("../../../migrations/011_request_rotation_default.sql"),
    },
    Migration {
        version: 12,
        name: "012_routing_diagnostics",
        sql: include_str!("../../../migrations/012_routing_diagnostics.sql"),
    },
    Migration {
        version: 13,
        name: "013_routing_strategy",
        sql: include_str!("../../../migrations/013_routing_strategy.sql"),
    },
    Migration {
        version: 14,
        name: "014_default_service_tier",
        sql: include_str!("../../../migrations/014_default_service_tier.sql"),
    },
    Migration {
        version: 15,
        name: "015_cache_write_input_tokens",
        sql: include_str!("../../../migrations/015_cache_write_input_tokens.sql"),
    },
    Migration {
        version: 16,
        name: "016_response_affinity",
        sql: include_str!("../../../migrations/016_response_affinity.sql"),
    },
    Migration {
        version: 17,
        name: "017_generation_ms",
        sql: include_str!("../../../migrations/017_generation_ms.sql"),
    },
    Migration {
        version: 18,
        name: "018_image_base_model",
        sql: include_str!("../../../migrations/018_image_base_model.sql"),
    },
    Migration {
        version: 19,
        name: "019_remove_cache_write_input_tokens",
        sql: include_str!("../../../migrations/019_remove_cache_write_input_tokens.sql"),
    },
    Migration {
        version: 20,
        name: "020_cache_write_input_tokens",
        sql: include_str!("../../../migrations/020_cache_write_input_tokens.sql"),
    },
    Migration {
        version: 21,
        name: "021_terminal_usage_per_request",
        sql: include_str!("../../../migrations/021_terminal_usage_per_request.sql"),
    },
    Migration {
        version: 22,
        name: "022_usage_retention_rollups",
        sql: include_str!("../../../migrations/022_usage_retention_rollups.sql"),
    },
    Migration {
        version: 23,
        name: "023_server_proxy_objects",
        sql: include_str!("../../../migrations/023_server_proxy_objects.sql"),
    },
    Migration {
        version: 24,
        name: "024_remove_free_account_policy",
        sql: include_str!("../../../migrations/024_remove_free_account_policy.sql"),
    },
    Migration {
        version: 25,
        name: "025_usage_effective_credits",
        sql: include_str!("../../../migrations/025_usage_effective_credits.sql"),
    },
    Migration {
        version: 26,
        name: "026_remove_effective_credits",
        sql: include_str!("../../../migrations/026_remove_effective_credits.sql"),
    },
    Migration {
        version: 27,
        name: "027_remove_quota_refresh_interval",
        sql: include_str!("../../../migrations/027_remove_quota_refresh_interval.sql"),
    },
    Migration {
        version: 28,
        name: "028_applied_service_tier",
        sql: include_str!("../../../migrations/028_applied_service_tier.sql"),
    },
    Migration {
        version: 29,
        name: "029_candidate_usage_rollups",
        sql: include_str!("../../../migrations/029_candidate_usage_rollups.sql"),
    },
    Migration {
        version: 30,
        name: "030_source_priced_key_rollups",
        sql: include_str!("../../../migrations/030_source_priced_key_rollups.sql"),
    },
    Migration {
        version: 31,
        name: "031_tool_use_diagnostics",
        sql: include_str!("../../../migrations/031_tool_use_diagnostics.sql"),
    },
    Migration {
        version: 32,
        name: "032_error_origin",
        sql: include_str!("../../../migrations/032_error_origin.sql"),
    },
    Migration {
        version: 33,
        name: "033_reasoning_effort",
        sql: include_str!("../../../migrations/033_reasoning_effort.sql"),
    },
    Migration {
        version: 34,
        name: "034_account_purchase_cost",
        sql: include_str!("../../../migrations/034_account_purchase_cost.sql"),
    },
    Migration {
        version: 35,
        name: "035_cache_write_ttl",
        sql: include_str!("../../../migrations/035_cache_write_ttl.sql"),
    },
    Migration {
        version: 36,
        name: "036_upstream_error_details",
        sql: include_str!("../../../migrations/036_upstream_error_details.sql"),
    },
    Migration {
        version: 37,
        name: "037_account_refresh_revisions",
        sql: include_str!("../../../migrations/037_account_refresh_revisions.sql"),
    },
    Migration {
        version: 38,
        name: "038_source_refresh_revisions",
        sql: include_str!("../../../migrations/038_source_refresh_revisions.sql"),
    },
    Migration {
        version: 39,
        name: "039_remove_v1_routing_options",
        sql: include_str!("../../../migrations/039_remove_v1_routing_options.sql"),
    },
];

pub(super) struct Migration {
    pub(super) version: u32,
    pub(super) name: &'static str,
    pub(super) sql: &'static str,
}
