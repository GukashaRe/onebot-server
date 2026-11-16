use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};
use sqlx::{FromRow, PgPool};
use subtle::ConstantTimeEq;

use crate::{error::AuthError, model::Account};

#[derive(Clone, Debug)]
pub struct AuthenticatedAccount {
    pub account: Account,
    pub token_hash: String,
}

#[derive(Debug, FromRow)]
struct AccountAuthRow {
    id: i64,
    username: String,
    labels: Vec<String>,
    rate_limit: i32,
    created_at: DateTime<Utc>,
    disabled: bool,
    token_hash: String,
}

impl AccountAuthRow {
    fn into_parts(self) -> (Account, String) {
        (
            Account {
                id: self.id,
                username: self.username,
                labels: self.labels,
                rate_limit: self.rate_limit,
                created_at: self.created_at,
                disabled: self.disabled,
            },
            self.token_hash,
        )
    }
}

/// 32 CSPRNG bytes, base64url without padding. Only the SHA-256 hex digest is
/// persisted.
pub fn generate_token() -> Result<String, getrandom::Error> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes)?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

pub fn hash_token(token: &str) -> String {
    let digest = Sha256::digest(token.as_bytes());
    let mut out = String::with_capacity(64);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(out, "{byte:02x}");
    }
    out
}

pub fn token_fingerprint(token_hash: &str) -> String {
    token_hash.chars().take(12).collect()
}

pub fn constant_time_eq(a: &str, b: &str) -> bool {
    a.as_bytes().ct_eq(b.as_bytes()).into()
}

/// The one token validation path shared by HTTP middleware and the WS handshake.
pub async fn authenticate_token(
    pool: &PgPool,
    token: &str,
) -> Result<AuthenticatedAccount, AuthError> {
    if token.is_empty() || token.len() > 512 {
        return Err(AuthError("invalid bearer token".to_owned()));
    }
    let token_hash = hash_token(token);
    let row = sqlx::query_as::<_, AccountAuthRow>(
        r#"
        SELECT id, username, labels, rate_limit, created_at, disabled, token_hash
        FROM accounts
        WHERE token_hash = $1
        "#,
    )
    .bind(&token_hash)
    .fetch_optional(pool)
    .await
    .map_err(|err| {
        tracing::error!(error = %err, "token lookup failed");
        AuthError("authentication backend unavailable".to_owned())
    })?
    .ok_or_else(|| AuthError("invalid token".to_owned()))?;

    let (account, token_hash) = row.into_parts();
    if account.disabled {
        return Err(AuthError("account is disabled".to_owned()));
    }
    Ok(AuthenticatedAccount {
        account,
        token_hash,
    })
}

/// Cheap periodic WS revalidation fallback. Returns false when the account was
/// disabled or its stored hash no longer matches the hash captured at handshake.
pub async fn token_still_valid(
    pool: &PgPool,
    account_id: i64,
    expected_token_hash: &str,
) -> Result<bool, sqlx::Error> {
    let row = sqlx::query_as::<_, (String, bool)>(
        "SELECT token_hash, disabled FROM accounts WHERE id = $1",
    )
    .bind(account_id)
    .fetch_optional(pool)
    .await?;
    match row {
        Some((hash, disabled)) => Ok(!disabled && constant_time_eq(&hash, expected_token_hash)),
        None => Ok(false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_is_urlsafe_and_hashes_to_64_hex() {
        let token = generate_token().expect("CSPRNG should be available in tests");
        assert_eq!(token.len(), 43);
        assert!(
            token
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        );
        let hash = hash_token(&token);
        assert_eq!(hash.len(), 64);
        assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(token_fingerprint(&hash), hash[..12]);
    }

    #[test]
    fn tokens_are_unique_and_constant_time_comparison_works() {
        let first = generate_token().unwrap();
        let second = generate_token().unwrap();
        assert_ne!(first, second);
        assert!(constant_time_eq("secret", "secret"));
        assert!(!constant_time_eq("secret", "secreu"));
        assert!(!constant_time_eq("secret", "secret2"));
    }
}
