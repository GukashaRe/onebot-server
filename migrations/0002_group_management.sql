-- Group management: per-group card, persistent bans, and operator audit trail.
--
-- Ban model choice: a dedicated group_bans table (rather than a column on
-- group_members) keeps active bans independent from membership rows and makes
-- expiry queries/cleanup straightforward. Active means banned_until > now();
-- expired rows are harmless and may be removed by a periodic task later.

ALTER TABLE group_members
    ADD COLUMN card TEXT NOT NULL DEFAULT '';

CREATE TABLE group_bans (
    group_id     BIGINT      NOT NULL REFERENCES groups(id) ON DELETE CASCADE,
    account_id   BIGINT      NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    banned_until TIMESTAMPTZ NOT NULL,
    operator_id  BIGINT      NOT NULL REFERENCES accounts(id) ON DELETE RESTRICT,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (group_id, account_id)
);

CREATE INDEX group_bans_expiry
    ON group_bans (banned_until);

CREATE TABLE audit_logs (
    id          BIGSERIAL   PRIMARY KEY,
    actor_id    BIGINT      NOT NULL REFERENCES accounts(id) ON DELETE RESTRICT,
    action      TEXT        NOT NULL,
    target_type TEXT        NOT NULL DEFAULT 'account',
    target_id   BIGINT,
    group_id    BIGINT,
    detail      JSONB       NOT NULL DEFAULT '{}'::jsonb,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX audit_logs_group_created
    ON audit_logs (group_id, created_at DESC);
CREATE INDEX audit_logs_actor_created
    ON audit_logs (actor_id, created_at DESC);
CREATE INDEX audit_logs_action_created
    ON audit_logs (action, created_at DESC);
