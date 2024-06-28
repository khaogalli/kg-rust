use anyhow::Context;
use argon2::password_hash::SaltString;
use argon2::{Argon2, PasswordHash};

use crate::api::Error;
use crate::api::Result;

pub(crate) async fn hash_password(password: String) -> Result<String> {
    tokio::task::spawn_blocking(move || -> Result<String> {
        let salt = SaltString::generate(rand::thread_rng());
        Ok(
            PasswordHash::generate(Argon2::default(), password, salt.as_salt())
                .map_err(|e| anyhow::anyhow!("failed to generate password hash: {}", e))?
                .to_string(),
        )
    })
    .await
    .context("panic in generating password hash")?
}

pub(crate) async fn verify_password(password: String, password_hash: String) -> Result<()> {
    tokio::task::spawn_blocking(move || -> Result<()> {
        let hash = PasswordHash::new(&password_hash)
            .map_err(|e| anyhow::anyhow!("invalid password hash: {}", e))?;

        hash.verify_password(&[&Argon2::default()], password)
            .map_err(|e| match e {
                argon2::password_hash::Error::Password => Error::Unauthorized,
                _ => anyhow::anyhow!("failed to verify password hash: {}", e).into(),
            })
    })
    .await
    .context("panic in verifying password hash")?
}
