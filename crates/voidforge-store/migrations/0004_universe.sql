-- The universe's epoch: the wall-clock Unix ms of game time 0 (decision log D-044, D-048, D-050).
-- One row, written by seed in the same transaction as the seq-0 snapshot, so a universe never
-- exists without it; serve reads it and never invents one. The CHECK repeats the code's window,
-- [1970-01-01, 2100-01-01), so the database holds nothing the code would refuse.
CREATE TABLE universe (
    id            boolean     PRIMARY KEY DEFAULT true CHECK (id),
    epoch_unix_ms bigint      NOT NULL
                  CONSTRAINT universe_epoch_in_window
                  CHECK (epoch_unix_ms >= 0 AND epoch_unix_ms < 4102444800000),
    created_at    timestamptz NOT NULL DEFAULT now()
);
