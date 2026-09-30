-- pool rotation reads only pool_routing and max_retry_candidates. The old
-- scalar strategy/cooldown hints were never part of the current admission policy.
DELETE FROM metadata
WHERE key IN (
    'routing_strategy',
    'subscription_plan_order',
    'cooldown_after_failures',
    'keep_last_candidate_available'
);
