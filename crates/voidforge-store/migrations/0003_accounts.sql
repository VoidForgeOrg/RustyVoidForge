-- Player accounts: issuer and subject map to a player UUIDv7 (Technical Plan, Decision 9;
-- decision log D-034). The server generates the UUID (P3.3). log_records.player_id has no foreign
-- key to this table, because the log must replay into a database without accounts.
-- uuid_extract_version is NULL for a UUID outside the RFC 9562 variant (the nil UUID, for one), and
-- a CHECK that is NULL passes, so NULL counts as "not version 7".
CREATE TABLE accounts (
    player_id  uuid        PRIMARY KEY CHECK (coalesce(uuid_extract_version(player_id), 0) = 7),
    issuer     text        NOT NULL CHECK (issuer <> ''),
    subject    text        NOT NULL CHECK (subject <> ''),
    created_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT accounts_issuer_subject_unique UNIQUE (issuer, subject)
);
