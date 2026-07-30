CREATE TABLE response_affinity (
    response_key TEXT PRIMARY KEY,
    candidate_id TEXT NOT NULL,
    expires_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL
);

CREATE INDEX idx_response_affinity_expires ON response_affinity(expires_at_ms);
