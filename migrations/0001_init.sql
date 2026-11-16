-- Protocol-neutral storage schema. Protocol-specific field names (post_type,
-- self_id, message_type, etc.) do not appear in the messages table or in cursor
-- keys. Relational columns such as group_id are domain identifiers, not wire
-- protocol fields. Protocol differences are handled by src/protocol/*.

CREATE TABLE accounts (
    id           BIGSERIAL PRIMARY KEY,
    username     TEXT        NOT NULL UNIQUE,
    token_hash   TEXT        NOT NULL UNIQUE,
    labels       TEXT[]      NOT NULL DEFAULT '{}',
    rate_limit   INTEGER     NOT NULL CHECK (rate_limit > 0),
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    disabled     BOOLEAN     NOT NULL DEFAULT FALSE,
    CONSTRAINT accounts_username_lowercase CHECK (username = lower(username)),
    CONSTRAINT accounts_username_length CHECK (char_length(username) BETWEEN 1 AND 64),
    CONSTRAINT accounts_token_hash_shape CHECK (token_hash ~ '^[0-9a-f]{64}$')
);

CREATE TABLE friends (
    account_a  BIGINT      NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    account_b  BIGINT      NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (account_a, account_b),
    CONSTRAINT friends_ordered_pair CHECK (account_a < account_b)
);

CREATE TABLE friend_requests (
    id         BIGSERIAL   PRIMARY KEY,
    from_id    BIGINT      NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    to_id      BIGINT      NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    status     TEXT        NOT NULL DEFAULT 'pending'
                           CHECK (status IN ('pending', 'approved', 'rejected')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT friend_requests_not_self CHECK (from_id <> to_id)
);

CREATE UNIQUE INDEX friend_requests_one_pending
    ON friend_requests (from_id, to_id)
    WHERE status = 'pending';
CREATE INDEX friend_requests_inbox
    ON friend_requests (to_id, status, created_at DESC);

CREATE TABLE groups (
    id         BIGSERIAL   PRIMARY KEY,
    name       TEXT        NOT NULL CHECK (char_length(name) BETWEEN 1 AND 128),
    owner_id   BIGINT      NOT NULL REFERENCES accounts(id) ON DELETE RESTRICT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE group_members (
    group_id   BIGINT      NOT NULL REFERENCES groups(id) ON DELETE CASCADE,
    account_id BIGINT      NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    role       TEXT        NOT NULL CHECK (role IN ('owner', 'admin', 'member')),
    joined_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (group_id, account_id)
);

CREATE UNIQUE INDEX group_members_single_owner
    ON group_members (group_id)
    WHERE role = 'owner';
CREATE INDEX group_members_by_account
    ON group_members (account_id);

CREATE TABLE group_join_requests (
    id         BIGSERIAL   PRIMARY KEY,
    group_id   BIGINT      NOT NULL REFERENCES groups(id) ON DELETE CASCADE,
    account_id BIGINT      NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    status     TEXT        NOT NULL DEFAULT 'pending'
                           CHECK (status IN ('pending', 'approved', 'rejected')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE UNIQUE INDEX group_join_requests_one_pending
    ON group_join_requests (group_id, account_id)
    WHERE status = 'pending';
CREATE INDEX group_join_requests_inbox
    ON group_join_requests (group_id, status, created_at DESC);

CREATE TABLE messages (
    id          BIGSERIAL   PRIMARY KEY,
    session_type TEXT       NOT NULL CHECK (session_type IN ('group', 'private')),
    session_id  TEXT        NOT NULL,
    sender_id   BIGINT      NOT NULL REFERENCES accounts(id) ON DELETE RESTRICT,
    message_seq BIGINT      NOT NULL CHECK (message_seq > 0),
    content     JSONB       NOT NULL CHECK (jsonb_typeof(content) = 'array'),
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    recalled    BOOLEAN     NOT NULL DEFAULT FALSE,
    UNIQUE (session_type, session_id, message_seq)
);

-- Cursors use (session_type, session_id, message_seq), never timestamps or
-- global message ids.
CREATE INDEX messages_session_seq
    ON messages (session_type, session_id, message_seq);

-- Atomic, per-session monotonic allocation. Row is locked by the upsert for the
-- duration of the message-insert transaction, so concurrent sends serialize only
-- within the same conversation.
CREATE TABLE message_sequences (
    session_type TEXT   NOT NULL CHECK (session_type IN ('group', 'private')),
    session_id   TEXT   NOT NULL,
    next_seq     BIGINT NOT NULL DEFAULT 1 CHECK (next_seq > 0),
    PRIMARY KEY (session_type, session_id)
);

-- Operator audit trail for token rotation. "actor" is a username for self-reset
-- or a server-operator label supplied through X-Admin-Actor for admin reset.
CREATE TABLE token_reset_audit (
    id         BIGSERIAL   PRIMARY KEY,
    account_id BIGINT      NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    actor      TEXT        NOT NULL,
    action     TEXT        NOT NULL DEFAULT 'token_reset',
    reason     TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX token_reset_audit_account
    ON token_reset_audit (account_id, created_at DESC);
