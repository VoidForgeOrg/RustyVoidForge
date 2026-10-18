-- Periodic snapshots (Architectural Design, Persistence; decision log D-034).
-- seq 0 is a snapshot taken before any log record (the P2.9 bootstrap), so there is no foreign
-- key to log_records. state_hash is BLAKE3-256 over the World's canonical bytes (D-024).
CREATE TABLE snapshots (
    seq           bigint      PRIMARY KEY CHECK (seq >= 0),
    game_time     bigint      NOT NULL,
    rules_version integer     NOT NULL CHECK (rules_version >= 0),
    engine_build  text        NOT NULL,
    state_hash    bytea       NOT NULL CHECK (octet_length(state_hash) = 32),
    payload       bytea       NOT NULL,
    created_at    timestamptz NOT NULL DEFAULT now()
);
