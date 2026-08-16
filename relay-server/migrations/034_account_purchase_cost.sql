UPDATE accounts
SET data_json = json_set(
    data_json,
    '$.purchaseCostMicroUsd',
    json_extract(data_json, '$.economics.purchaseCostMicroUsd')
)
WHERE (
    json_type(data_json, '$.purchaseCostMicroUsd') IS NULL
    OR json_type(data_json, '$.purchaseCostMicroUsd') = 'null'
)
    AND json_type(data_json, '$.economics.purchaseCostMicroUsd') IS NOT NULL;

UPDATE accounts
SET data_json = json_remove(data_json, '$.economics')
WHERE json_type(data_json, '$.economics') IS NOT NULL;
