-- The append-only log (Architectural Design, Persistence; decision log D-034).
-- Only player_id and idempotency_key are nullable; an advance record writes JSON values, never
-- SQL NULL. seq is gap-free by the single writer (P2.3), not by the database.
CREATE TABLE log_records (
    seq             bigint      PRIMARY KEY CHECK (seq >= 1),
    kind            text        NOT NULL
                    CHECK (kind IN ('command', 'advance', 'rules_change', 'system')),
    game_time       bigint      NOT NULL,
    wall_time       timestamptz NOT NULL DEFAULT now(),
    player_id       uuid,
    idempotency_key text,
    payload         jsonb       NOT NULL,
    outcome         jsonb       NOT NULL,
    events          jsonb       NOT NULL CHECK (jsonb_typeof(events) = 'array'),
    rules_version   integer     NOT NULL CHECK (rules_version >= 0),
    engine_build    text        NOT NULL,
    -- A client key is unique per player. NULLs stay distinct, so keyless records never collide.
    CONSTRAINT log_records_player_key_unique UNIQUE (player_id, idempotency_key),
    -- A key belongs to a player, so a key without one has no meaning.
    CONSTRAINT log_records_key_needs_player
        CHECK (idempotency_key IS NULL OR player_id IS NOT NULL)
);
