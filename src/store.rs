use chrono::{DateTime, Utc};
use sqlx::{FromRow, PgPool};
use thiserror::Error;

use crate::{
    auth::{generate_token, hash_token},
    model::{
        Account, ActionableGroupJoinRequest, FriendRequestView, FriendView, GroupJoinRequestView,
        GroupMemberView, GroupView, IncomingFriendRequest, MessageSegment, SessionType,
        StoredMessage, private_session_id,
    },
};

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("username is already taken")]
    UsernameTaken,
    #[error("not found: {0}")]
    NotFound(String),
    #[error("conflict: {0}")]
    Conflict(String),
    #[error("forbidden: {0}")]
    Forbidden(String),
    #[error("internal storage error: {0}")]
    Internal(String),
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

#[derive(Clone, Debug)]
pub struct Store {
    pool: PgPool,
}

#[derive(Debug, FromRow)]
struct AccountRow {
    id: i64,
    username: String,
    labels: Vec<String>,
    rate_limit: i32,
    created_at: DateTime<Utc>,
    disabled: bool,
}

impl AccountRow {
    fn into_account(self) -> Account {
        Account {
            id: self.id,
            username: self.username,
            labels: self.labels,
            rate_limit: self.rate_limit,
            created_at: self.created_at,
            disabled: self.disabled,
        }
    }
}

#[derive(Clone, Debug)]
pub struct GroupRecord {
    pub id: i64,
    pub name: String,
    pub owner_id: i64,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, FromRow)]
struct GroupRecordRow {
    id: i64,
    name: String,
    owner_id: i64,
    created_at: DateTime<Utc>,
}

impl From<GroupRecordRow> for GroupRecord {
    fn from(row: GroupRecordRow) -> Self {
        Self {
            id: row.id,
            name: row.name,
            owner_id: row.owner_id,
            created_at: row.created_at,
        }
    }
}

#[derive(Debug, FromRow)]
struct FriendRequestRow {
    id: i64,
    from_id: i64,
    to_id: i64,
    status: String,
    created_at: DateTime<Utc>,
}

impl From<FriendRequestRow> for FriendRequestView {
    fn from(row: FriendRequestRow) -> Self {
        Self {
            id: row.id,
            from_id: row.from_id,
            to_id: row.to_id,
            status: row.status,
            created_at: row.created_at,
        }
    }
}

#[derive(Debug, FromRow)]
struct GroupJoinRequestRow {
    id: i64,
    group_id: i64,
    account_id: i64,
    status: String,
    created_at: DateTime<Utc>,
}

impl From<GroupJoinRequestRow> for GroupJoinRequestView {
    fn from(row: GroupJoinRequestRow) -> Self {
        Self {
            id: row.id,
            group_id: row.group_id,
            account_id: row.account_id,
            status: row.status,
            created_at: row.created_at,
        }
    }
}

#[derive(Debug, FromRow)]
struct MemberRow {
    group_id: i64,
    account_id: i64,
    username: String,
    role: String,
    joined_at: DateTime<Utc>,
}

impl From<MemberRow> for GroupMemberView {
    fn from(row: MemberRow) -> Self {
        Self {
            group_id: row.group_id,
            account_id: row.account_id,
            username: row.username,
            role: row.role,
            joined_at: row.joined_at,
        }
    }
}

#[derive(Debug, FromRow)]
struct GroupViewRow {
    id: i64,
    name: String,
    owner_id: i64,
    created_at: DateTime<Utc>,
    member_count: i64,
    self_role: String,
}

impl From<GroupViewRow> for GroupView {
    fn from(row: GroupViewRow) -> Self {
        Self {
            id: row.id,
            name: row.name,
            owner_id: row.owner_id,
            created_at: row.created_at,
            member_count: row.member_count,
            self_role: row.self_role,
        }
    }
}

#[derive(Debug, FromRow)]
struct FriendRow {
    account_id: i64,
    username: String,
    created_at: DateTime<Utc>,
}

#[derive(Debug, FromRow)]
struct IncomingFriendRequestRow {
    request_id: i64,
    from_id: i64,
    from_username: String,
    created_at: DateTime<Utc>,
}

#[derive(Debug, FromRow)]
struct ActionableGroupJoinRequestRow {
    request_id: i64,
    group_id: i64,
    group_name: String,
    account_id: i64,
    account_username: String,
    created_at: DateTime<Utc>,
}

impl From<FriendRow> for FriendView {
    fn from(row: FriendRow) -> Self {
        Self {
            account_id: row.account_id,
            username: row.username,
            created_at: row.created_at,
        }
    }
}

#[derive(Debug, FromRow)]
struct MessageRow {
    id: i64,
    session_type: String,
    session_id: String,
    sender_id: i64,
    message_seq: i64,
    content: sqlx::types::Json<Vec<MessageSegment>>,
    created_at: DateTime<Utc>,
    recalled: bool,
}

impl TryFrom<MessageRow> for StoredMessage {
    type Error = StoreError;

    fn try_from(row: MessageRow) -> Result<Self, Self::Error> {
        let session_type = match row.session_type.as_str() {
            "group" => SessionType::Group,
            "private" => SessionType::Private,
            other => {
                return Err(StoreError::Internal(format!(
                    "invalid session_type {other}"
                )));
            }
        };
        Ok(Self {
            id: row.id,
            session_type,
            session_id: row.session_id,
            sender_id: row.sender_id,
            message_seq: row.message_seq,
            content: row.content.0,
            created_at: row.created_at,
            recalled: row.recalled,
        })
    }
}

impl Store {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    // ---------------------------------------------------------------- accounts

    pub async fn create_account(
        &self,
        username: &str,
        labels: &[String],
        rate_limit: i32,
    ) -> Result<(Account, String), StoreError> {
        for _ in 0..8 {
            let token = generate_token()
                .map_err(|err| StoreError::Internal(format!("CSPRNG unavailable: {err}")))?;
            let token_hash = hash_token(&token);
            let inserted = sqlx::query_as::<_, AccountRow>(
                r#"
                INSERT INTO accounts (username, token_hash, labels, rate_limit)
                VALUES ($1, $2, $3, $4)
                ON CONFLICT DO NOTHING
                RETURNING id, username, labels, rate_limit, created_at, disabled
                "#,
            )
            .bind(username)
            .bind(&token_hash)
            .bind(labels)
            .bind(rate_limit)
            .fetch_optional(&self.pool)
            .await?;

            if let Some(row) = inserted {
                return Ok((row.into_account(), token));
            }

            let username_exists: bool =
                sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM accounts WHERE username = $1)")
                    .bind(username)
                    .fetch_one(&self.pool)
                    .await?;
            if username_exists {
                return Err(StoreError::UsernameTaken);
            }
            // Otherwise the token hash collided; generate another token.
        }
        Err(StoreError::Internal(
            "could not allocate a unique token after 8 attempts".to_owned(),
        ))
    }

    pub async fn get_account(&self, account_id: i64) -> Result<Account, StoreError> {
        let row = sqlx::query_as::<_, AccountRow>(
            "SELECT id, username, labels, rate_limit, created_at, disabled FROM accounts WHERE id = $1",
        )
        .bind(account_id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| StoreError::NotFound("account".to_owned()))?;
        Ok(row.into_account())
    }

    pub async fn get_accounts_by_ids(
        &self,
        account_ids: &[i64],
    ) -> Result<Vec<Account>, StoreError> {
        if account_ids.is_empty() {
            return Ok(Vec::new());
        }
        let rows = sqlx::query_as::<_, AccountRow>(
            r#"
            SELECT id, username, labels, rate_limit, created_at, disabled
            FROM accounts
            WHERE id = ANY($1)
            "#,
        )
        .bind(account_ids)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(AccountRow::into_account).collect())
    }

    /// Rotates the token and writes the required operator audit row in one
    /// transaction. Returns the new plaintext token exactly once.
    pub async fn rotate_token(
        &self,
        account_id: i64,
        actor: &str,
        reason: Option<&str>,
    ) -> Result<(Account, String), StoreError> {
        for _ in 0..8 {
            let token = generate_token()
                .map_err(|err| StoreError::Internal(format!("CSPRNG unavailable: {err}")))?;
            let token_hash = hash_token(&token);
            let mut tx = self.pool.begin().await?;
            let updated = sqlx::query_as::<_, AccountRow>(
                r#"
                UPDATE accounts
                SET token_hash = $2
                WHERE id = $1
                RETURNING id, username, labels, rate_limit, created_at, disabled
                "#,
            )
            .bind(account_id)
            .bind(&token_hash)
            .fetch_optional(&mut *tx)
            .await;

            let row = match updated {
                Ok(row) => row,
                Err(err) if is_unique_violation(&err, "accounts_token_hash_key") => {
                    tx.rollback().await?;
                    continue;
                }
                Err(err) => return Err(err.into()),
            };

            let row = row.ok_or_else(|| StoreError::NotFound("account".to_owned()))?;
            sqlx::query(
                r#"
                INSERT INTO token_reset_audit (account_id, actor, action, reason)
                VALUES ($1, $2, 'token_reset', $3)
                "#,
            )
            .bind(account_id)
            .bind(actor)
            .bind(reason)
            .execute(&mut *tx)
            .await?;
            tx.commit().await?;
            return Ok((row.into_account(), token));
        }
        Err(StoreError::Internal(
            "could not allocate a unique token after 8 attempts".to_owned(),
        ))
    }

    // ----------------------------------------------------------------- friends

    pub async fn create_friend_request(
        &self,
        from_id: i64,
        to_id: i64,
    ) -> Result<FriendRequestView, StoreError> {
        if from_id == to_id {
            return Err(StoreError::Conflict(
                "cannot add yourself as a friend".to_owned(),
            ));
        }
        let target_exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM accounts WHERE id = $1 AND NOT disabled)",
        )
        .bind(to_id)
        .fetch_one(&self.pool)
        .await?;
        if !target_exists {
            return Err(StoreError::NotFound("target account".to_owned()));
        }
        let already_friends: bool = sqlx::query_scalar(
            r#"
            SELECT EXISTS(
                SELECT 1 FROM friends
                WHERE account_a = LEAST($1::bigint, $2::bigint)
                  AND account_b = GREATEST($1::bigint, $2::bigint)
            )
            "#,
        )
        .bind(from_id)
        .bind(to_id)
        .fetch_one(&self.pool)
        .await?;
        if already_friends {
            return Err(StoreError::Conflict("already friends".to_owned()));
        }

        let row = sqlx::query_as::<_, FriendRequestRow>(
            r#"
            INSERT INTO friend_requests (from_id, to_id)
            VALUES ($1, $2)
            ON CONFLICT DO NOTHING
            RETURNING id, from_id, to_id, status, created_at
            "#,
        )
        .bind(from_id)
        .bind(to_id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| {
            StoreError::Conflict("a pending friend request already exists".to_owned())
        })?;
        Ok(row.into())
    }

    pub async fn get_friend_request(
        &self,
        request_id: i64,
    ) -> Result<FriendRequestView, StoreError> {
        let row = sqlx::query_as::<_, FriendRequestRow>(
            "SELECT id, from_id, to_id, status, created_at FROM friend_requests WHERE id = $1",
        )
        .bind(request_id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| StoreError::NotFound("friend request".to_owned()))?;
        Ok(row.into())
    }

    pub async fn resolve_friend_request(
        &self,
        request_id: i64,
        approver_id: i64,
        approve: bool,
    ) -> Result<FriendRequestView, StoreError> {
        let mut tx = self.pool.begin().await?;
        let row = sqlx::query_as::<_, FriendRequestRow>(
            r#"
            SELECT id, from_id, to_id, status, created_at
            FROM friend_requests
            WHERE id = $1
            FOR UPDATE
            "#,
        )
        .bind(request_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| StoreError::NotFound("friend request".to_owned()))?;

        if row.to_id != approver_id {
            return Err(StoreError::Forbidden(
                "only the request target may approve or reject it".to_owned(),
            ));
        }
        if row.status != "pending" {
            return Err(StoreError::Conflict(format!(
                "friend request is already {}",
                row.status
            )));
        }

        let status = if approve { "approved" } else { "rejected" };
        sqlx::query("UPDATE friend_requests SET status = $2 WHERE id = $1")
            .bind(request_id)
            .bind(status)
            .execute(&mut *tx)
            .await?;

        if approve {
            let (lo, hi) = ordered_pair(row.from_id, row.to_id);
            sqlx::query(
                r#"
                INSERT INTO friends (account_a, account_b)
                VALUES ($1, $2)
                ON CONFLICT DO NOTHING
                "#,
            )
            .bind(lo)
            .bind(hi)
            .execute(&mut *tx)
            .await?;
        }

        let updated = FriendRequestRow {
            status: status.to_owned(),
            ..row
        };
        tx.commit().await?;
        Ok(updated.into())
    }

    pub async fn list_friends(&self, account_id: i64) -> Result<Vec<FriendView>, StoreError> {
        let rows = sqlx::query_as::<_, FriendRow>(
            r#"
            SELECT a.id AS account_id, a.username, f.created_at
            FROM friends f
            JOIN accounts a
              ON a.id = CASE WHEN f.account_a = $1 THEN f.account_b ELSE f.account_a END
            WHERE f.account_a = $1 OR f.account_b = $1
            ORDER BY a.username ASC, a.id ASC
            "#,
        )
        .bind(account_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(Into::into).collect())
    }

    pub async fn list_incoming_friend_requests(
        &self,
        account_id: i64,
    ) -> Result<Vec<IncomingFriendRequest>, StoreError> {
        let rows = sqlx::query_as::<_, IncomingFriendRequestRow>(
            r#"
            SELECT fr.id AS request_id, fr.from_id, a.username AS from_username, fr.created_at
            FROM friend_requests fr
            JOIN accounts a ON a.id = fr.from_id
            WHERE fr.to_id = $1 AND fr.status = 'pending'
            ORDER BY fr.created_at ASC, fr.id ASC
            "#,
        )
        .bind(account_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|row| IncomingFriendRequest {
                request_id: row.request_id,
                from_id: row.from_id,
                from_username: row.from_username,
                created_at: row.created_at,
            })
            .collect())
    }

    // ------------------------------------------------------------------ groups

    pub async fn create_group(&self, name: &str, owner_id: i64) -> Result<GroupView, StoreError> {
        let mut tx = self.pool.begin().await?;
        let group = sqlx::query_as::<_, GroupRecordRow>(
            r#"
            INSERT INTO groups (name, owner_id)
            VALUES ($1, $2)
            RETURNING id, name, owner_id, created_at
            "#,
        )
        .bind(name)
        .bind(owner_id)
        .fetch_one(&mut *tx)
        .await?;
        sqlx::query(
            r#"
            INSERT INTO group_members (group_id, account_id, role)
            VALUES ($1, $2, 'owner')
            "#,
        )
        .bind(group.id)
        .bind(owner_id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(GroupView {
            id: group.id,
            name: group.name,
            owner_id: group.owner_id,
            created_at: group.created_at,
            member_count: 1,
            self_role: "owner".to_owned(),
        })
    }

    pub async fn get_group(&self, group_id: i64) -> Result<GroupRecord, StoreError> {
        let row = sqlx::query_as::<_, GroupRecordRow>(
            "SELECT id, name, owner_id, created_at FROM groups WHERE id = $1",
        )
        .bind(group_id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| StoreError::NotFound("group".to_owned()))?;
        Ok(row.into())
    }

    pub async fn list_groups_for(&self, account_id: i64) -> Result<Vec<GroupView>, StoreError> {
        let rows = sqlx::query_as::<_, GroupViewRow>(
            r#"
            SELECT g.id, g.name, g.owner_id, g.created_at,
                   COUNT(all_members.account_id)::bigint AS member_count,
                   self_member.role AS self_role
            FROM groups g
            JOIN group_members self_member
              ON self_member.group_id = g.id AND self_member.account_id = $1
            JOIN group_members all_members ON all_members.group_id = g.id
            GROUP BY g.id, g.name, g.owner_id, g.created_at, self_member.role
            ORDER BY g.created_at ASC, g.id ASC
            "#,
        )
        .bind(account_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(Into::into).collect())
    }

    pub async fn get_member(
        &self,
        group_id: i64,
        account_id: i64,
    ) -> Result<GroupMemberView, StoreError> {
        let row = sqlx::query_as::<_, MemberRow>(
            r#"
            SELECT gm.group_id, gm.account_id, a.username, gm.role, gm.joined_at
            FROM group_members gm
            JOIN accounts a ON a.id = gm.account_id
            WHERE gm.group_id = $1 AND gm.account_id = $2
            "#,
        )
        .bind(group_id)
        .bind(account_id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| StoreError::NotFound("group member".to_owned()))?;
        Ok(row.into())
    }

    pub async fn list_members(&self, group_id: i64) -> Result<Vec<GroupMemberView>, StoreError> {
        let rows = sqlx::query_as::<_, MemberRow>(
            r#"
            SELECT gm.group_id, gm.account_id, a.username, gm.role, gm.joined_at
            FROM group_members gm
            JOIN accounts a ON a.id = gm.account_id
            WHERE gm.group_id = $1
            ORDER BY gm.joined_at ASC, gm.account_id ASC
            "#,
        )
        .bind(group_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(Into::into).collect())
    }

    pub async fn list_group_admins(
        &self,
        group_id: i64,
    ) -> Result<Vec<GroupMemberView>, StoreError> {
        let rows = sqlx::query_as::<_, MemberRow>(
            r#"
            SELECT gm.group_id, gm.account_id, a.username, gm.role, gm.joined_at
            FROM group_members gm
            JOIN accounts a ON a.id = gm.account_id
            WHERE gm.group_id = $1 AND gm.role IN ('owner', 'admin')
            ORDER BY gm.joined_at ASC, gm.account_id ASC
            "#,
        )
        .bind(group_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(Into::into).collect())
    }

    pub async fn create_group_join_request(
        &self,
        group_id: i64,
        account_id: i64,
    ) -> Result<GroupJoinRequestView, StoreError> {
        let exists =
            sqlx::query_scalar::<_, bool>("SELECT EXISTS(SELECT 1 FROM groups WHERE id = $1)")
                .bind(group_id)
                .fetch_one(&self.pool)
                .await?;
        if !exists {
            return Err(StoreError::NotFound("group".to_owned()));
        }
        let is_member = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM group_members WHERE group_id = $1 AND account_id = $2)",
        )
        .bind(group_id)
        .bind(account_id)
        .fetch_one(&self.pool)
        .await?;
        if is_member {
            return Err(StoreError::Conflict("already a group member".to_owned()));
        }
        let row = sqlx::query_as::<_, GroupJoinRequestRow>(
            r#"
            INSERT INTO group_join_requests (group_id, account_id)
            VALUES ($1, $2)
            ON CONFLICT DO NOTHING
            RETURNING id, group_id, account_id, status, created_at
            "#,
        )
        .bind(group_id)
        .bind(account_id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| StoreError::Conflict("a pending join request already exists".to_owned()))?;
        Ok(row.into())
    }

    pub async fn list_actionable_group_join_requests(
        &self,
        approver_id: i64,
    ) -> Result<Vec<ActionableGroupJoinRequest>, StoreError> {
        let rows = sqlx::query_as::<_, ActionableGroupJoinRequestRow>(
            r#"
            SELECT gjr.id AS request_id,
                   gjr.group_id,
                   g.name AS group_name,
                   gjr.account_id,
                   a.username AS account_username,
                   gjr.created_at
            FROM group_join_requests gjr
            JOIN groups g ON g.id = gjr.group_id
            JOIN accounts a ON a.id = gjr.account_id
            JOIN group_members gm
              ON gm.group_id = gjr.group_id AND gm.account_id = $1
            WHERE gjr.status = 'pending' AND gm.role IN ('owner', 'admin')
            ORDER BY gjr.created_at ASC, gjr.id ASC
            "#,
        )
        .bind(approver_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|row| ActionableGroupJoinRequest {
                request_id: row.request_id,
                group_id: row.group_id,
                group_name: row.group_name,
                account_id: row.account_id,
                account_username: row.account_username,
                created_at: row.created_at,
            })
            .collect())
    }

    pub async fn get_group_join_request(
        &self,
        group_id: i64,
        request_id: i64,
    ) -> Result<GroupJoinRequestView, StoreError> {
        let row = sqlx::query_as::<_, GroupJoinRequestRow>(
            r#"
            SELECT id, group_id, account_id, status, created_at
            FROM group_join_requests
            WHERE id = $1 AND group_id = $2
            "#,
        )
        .bind(request_id)
        .bind(group_id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| StoreError::NotFound("group join request".to_owned()))?;
        Ok(row.into())
    }

    pub async fn resolve_group_join_request(
        &self,
        group_id: i64,
        request_id: i64,
        approver_id: i64,
        approve: bool,
    ) -> Result<GroupJoinRequestView, StoreError> {
        let mut tx = self.pool.begin().await?;
        let approver_role = sqlx::query_scalar::<_, String>(
            r#"
            SELECT role FROM group_members
            WHERE group_id = $1 AND account_id = $2
            FOR UPDATE
            "#,
        )
        .bind(group_id)
        .bind(approver_id)
        .fetch_optional(&mut *tx)
        .await?;
        let Some(role) = approver_role else {
            return Err(StoreError::Forbidden(
                "only a group owner or admin may approve join requests".to_owned(),
            ));
        };
        if role != "owner" && role != "admin" {
            return Err(StoreError::Forbidden(
                "only a group owner or admin may approve join requests".to_owned(),
            ));
        }

        let row = sqlx::query_as::<_, GroupJoinRequestRow>(
            r#"
            SELECT id, group_id, account_id, status, created_at
            FROM group_join_requests
            WHERE id = $1 AND group_id = $2
            FOR UPDATE
            "#,
        )
        .bind(request_id)
        .bind(group_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| StoreError::NotFound("group join request".to_owned()))?;

        if row.status != "pending" {
            return Err(StoreError::Conflict(format!(
                "group join request is already {}",
                row.status
            )));
        }

        let status = if approve { "approved" } else { "rejected" };
        if approve {
            sqlx::query(
                r#"
                INSERT INTO group_members (group_id, account_id, role)
                VALUES ($1, $2, 'member')
                ON CONFLICT (group_id, account_id) DO NOTHING
                "#,
            )
            .bind(group_id)
            .bind(row.account_id)
            .execute(&mut *tx)
            .await?;
        }
        sqlx::query("UPDATE group_join_requests SET status = $2 WHERE id = $1")
            .bind(request_id)
            .bind(status)
            .execute(&mut *tx)
            .await?;

        let updated = GroupJoinRequestRow {
            status: status.to_owned(),
            ..row
        };
        tx.commit().await?;
        Ok(updated.into())
    }

    pub async fn leave_group(
        &self,
        group_id: i64,
        account_id: i64,
    ) -> Result<GroupRecord, StoreError> {
        let mut tx = self.pool.begin().await?;
        let group = sqlx::query_as::<_, GroupRecordRow>(
            "SELECT id, name, owner_id, created_at FROM groups WHERE id = $1 FOR UPDATE",
        )
        .bind(group_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| StoreError::NotFound("group".to_owned()))?;
        if group.owner_id == account_id {
            return Err(StoreError::Forbidden(
                "the group owner cannot leave in this prototype".to_owned(),
            ));
        }
        let deleted =
            sqlx::query("DELETE FROM group_members WHERE group_id = $1 AND account_id = $2")
                .bind(group_id)
                .bind(account_id)
                .execute(&mut *tx)
                .await?;
        if deleted.rows_affected() == 0 {
            return Err(StoreError::NotFound("group member".to_owned()));
        }
        tx.commit().await?;
        Ok(group.into())
    }

    // ---------------------------------------------------------------- messages

    pub async fn create_message(
        &self,
        session_type: SessionType,
        session_id: &str,
        sender_id: i64,
        content: Vec<MessageSegment>,
    ) -> Result<StoredMessage, StoreError> {
        let mut tx = self.pool.begin().await?;
        let message_seq: i64 = sqlx::query_scalar(
            r#"
            INSERT INTO message_sequences (session_type, session_id, next_seq)
            VALUES ($1, $2, 1)
            ON CONFLICT (session_type, session_id)
            DO UPDATE SET next_seq = message_sequences.next_seq + 1
            RETURNING next_seq
            "#,
        )
        .bind(session_type.as_str())
        .bind(session_id)
        .fetch_one(&mut *tx)
        .await?;

        let row = sqlx::query_as::<_, (i64, DateTime<Utc>)>(
            r#"
            INSERT INTO messages (session_type, session_id, sender_id, message_seq, content)
            VALUES ($1, $2, $3, $4, $5)
            RETURNING id, created_at
            "#,
        )
        .bind(session_type.as_str())
        .bind(session_id)
        .bind(sender_id)
        .bind(message_seq)
        .bind(sqlx::types::Json(&content))
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;

        Ok(StoredMessage {
            id: row.0,
            session_type,
            session_id: session_id.to_owned(),
            sender_id,
            message_seq,
            content,
            created_at: row.1,
            recalled: false,
        })
    }

    /// Cursor pagination is by message_seq only. Results are returned in
    /// ascending sequence order regardless of which bound was supplied.
    pub async fn list_messages(
        &self,
        session_type: SessionType,
        session_id: &str,
        before_seq: Option<i64>,
        after_seq: Option<i64>,
        limit: i64,
    ) -> Result<Vec<StoredMessage>, StoreError> {
        let rows = sqlx::query_as::<_, MessageRow>(
            r#"
            SELECT id, session_type, session_id, sender_id, message_seq,
                   content, created_at, recalled
            FROM messages
            WHERE session_type = $1
              AND session_id = $2
              AND ($3::bigint IS NULL OR message_seq < $3)
              AND ($4::bigint IS NULL OR message_seq > $4)
            ORDER BY message_seq DESC
            LIMIT $5
            "#,
        )
        .bind(session_type.as_str())
        .bind(session_id)
        .bind(before_seq)
        .bind(after_seq)
        .bind(limit.clamp(1, 500))
        .fetch_all(&self.pool)
        .await?;
        let mut messages = rows
            .into_iter()
            .map(StoredMessage::try_from)
            .collect::<Result<Vec<_>, _>>()?;
        messages.reverse();
        Ok(messages)
    }

    pub async fn private_messages(
        &self,
        requester: i64,
        peer: i64,
        before_seq: Option<i64>,
        after_seq: Option<i64>,
        limit: i64,
    ) -> Result<Vec<StoredMessage>, StoreError> {
        self.list_messages(
            SessionType::Private,
            &private_session_id(requester, peer),
            before_seq,
            after_seq,
            limit,
        )
        .await
    }
}

fn ordered_pair(a: i64, b: i64) -> (i64, i64) {
    if a < b { (a, b) } else { (b, a) }
}

fn is_unique_violation(err: &sqlx::Error, constraint: &str) -> bool {
    err.as_database_error()
        .and_then(|db| db.constraint())
        .map(|name| name == constraint)
        .unwrap_or(false)
}
