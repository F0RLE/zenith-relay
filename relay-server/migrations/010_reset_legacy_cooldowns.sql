UPDATE accounts
SET data_json = json_set(
    data_json,
    '$.cooldowns',
    json('{}'),
    '$.consecutiveFailures',
    0
)
WHERE json_type(data_json, '$.cooldowns') = 'object'
   OR json_type(data_json, '$.consecutiveFailures') IS NOT NULL;
