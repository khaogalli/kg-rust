use anyhow::Context;
use argon2::password_hash::SaltString;
use argon2::{Argon2, PasswordHash};
use axum::extract::State;
use axum::routing::{get, post};
use axum::{Json, Router};

use crate::api::auth::AuthUser;
use crate::api::Result;
use crate::api::{AppContext, Error, ResultExt};

pub(crate) fn router() -> Router<AppContext> {
    Router::new()
        .route("/api/users", post(create_user))
        .route("/api/users/login", post(login_user))
        .route("/api/users", get(get_current_user))
}

#[derive(serde::Serialize, serde::Deserialize)]
struct UserBody<T> {
    user: T,
}

#[derive(serde::Deserialize)]
struct NewUser {
    username: String,
    password: String,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct User {
    token: String,
    username: String,
}

async fn create_user(
    ctx: State<AppContext>,
    Json(req): Json<UserBody<NewUser>>,
) -> Result<Json<UserBody<User>>> {
    let hash = hash_password(req.user.password).await?;
    let user_id = sqlx::query_scalar!(
        r#"insert into "user" (username, password_hash) values ($1, $2) returning user_id"#,
        req.user.username,
        hash
    )
    .fetch_one(&ctx.db)
    .await
    .on_constraint("user_username_key", |_| {
        Error::unprocessable_entity([("username", "username taken")])
    })?;

    Ok(Json(UserBody {
        user: User {
            token: AuthUser { user_id }.to_jwt(&ctx),
            username: req.user.username,
        },
    }))
}

#[derive(serde::Deserialize)]
struct LoginUser {
    username: String,
    password: String,
}

async fn login_user(
    ctx: State<AppContext>,
    Json(req): Json<UserBody<LoginUser>>,
) -> Result<Json<UserBody<User>>> {
    let user = sqlx::query!(
        r#"
            select user_id, username, password_hash 
            from "user" where username = $1
        "#,
        req.user.username,
    )
    .fetch_optional(&ctx.db)
    .await?
    .ok_or_else(|| Error::unprocessable_entity([("username", "does not exist")]))?;

    verify_password(req.user.password, user.password_hash).await?;

    Ok(Json(UserBody {
        user: User {
            token: AuthUser {
                user_id: user.user_id,
            }
            .to_jwt(&ctx),
            username: user.username,
        },
    }))
}

async fn get_current_user(
    auth_user: AuthUser,
    ctx: State<AppContext>,
) -> Result<Json<UserBody<User>>> {
    let user = sqlx::query!(
        r#"select username from "user" where user_id = $1"#,
        auth_user.user_id
    )
    .fetch_one(&ctx.db)
    .await?;

    Ok(Json(UserBody {
        user: User {
            token: auth_user.to_jwt(&ctx),
            username: user.username,
        },
    }))
}

async fn hash_password(password: String) -> Result<String> {
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

async fn verify_password(password: String, password_hash: String) -> Result<()> {
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
