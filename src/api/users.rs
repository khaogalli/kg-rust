use std::io::{BufWriter, Cursor};

use anyhow::Context;
use axum::extract::{Multipart, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::decode;
use image::load_from_memory;
use serde::{Deserialize, Serialize};
use tokio::fs::File;
use tokio::io::AsyncWriteExt;
use tower_http::services::ServeDir;

use crate::api::auth::AuthUser;
use crate::api::util::{hash_password, verify_password};
use crate::api::Result;
use crate::api::{AppContext, Error, ResultExt};

pub(crate) fn router() -> Router<AppContext> {
    let serve_image_dir = ServeDir::new("images/users/");

    Router::new()
        .route("/api/users", post(create_user))
        .route("/api/users/login", post(login_user))
        .route("/api/users", get(get_current_user).patch(update_user))
        .route("/api/users/upload_image", post(upload_image))
        .nest_service("/api/users/image", serve_image_dir)
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
    id: uuid::Uuid,
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
            id: user_id,
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
            id: user.user_id,
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
    let user = sqlx::query_scalar!(
        r#"select username from "user" where user_id = $1"#,
        auth_user.user_id
    )
    .fetch_one(&ctx.db)
    .await?;

    Ok(Json(UserBody {
        user: User {
            id: auth_user.user_id,
            token: auth_user.to_jwt(&ctx),
            username: user,
        },
    }))
}

#[derive(serde::Deserialize)]
struct UpdateUser {
    username: Option<String>,
    update_pass: Option<UpdatePass>,
}

#[derive(serde::Deserialize)]
struct UpdatePass {
    old_password: String,
    new_password: String,
}

async fn update_user(
    auth_user: AuthUser,
    ctx: State<AppContext>,
    Json(req): Json<UserBody<UpdateUser>>,
) -> Result<Json<UserBody<User>>> {
    let mut tx = ctx.db.begin().await?;

    let user = sqlx::query!(
        r#"select username, password_hash from "user" where user_id = $1"#,
        auth_user.user_id
    )
    .fetch_one(&mut *tx)
    .await?;

    if let Some(update_pass) = req.user.update_pass {
        verify_password(update_pass.old_password, user.password_hash)
            .await
            .map_err(|e| match e {
                Error::Unauthorized => {
                    Error::unprocessable_entity([("old_password", "old password is incorrect")])
                }
                _ => e,
            })?;
        let new_hash = hash_password(update_pass.new_password).await?;

        sqlx::query!(
            r#"update "user" set password_hash = $1 where user_id = $2"#,
            new_hash,
            auth_user.user_id
        )
        .execute(&mut *tx)
        .await?;
    }

    if let Some(ref username) = req.user.username {
        sqlx::query!(
            r#"update "user" set username = $1 where user_id = $2"#,
            username,
            auth_user.user_id
        )
        .execute(&mut *tx)
        .await
        .on_constraint("user_username_key", |_| {
            Error::unprocessable_entity([("username", "username taken")])
        })?;
    }

    tx.commit().await?;

    Ok(Json(UserBody {
        user: User {
            id: auth_user.user_id,
            token: auth_user.to_jwt(&ctx),
            username: req.user.username.unwrap_or(user.username),
        },
    }))
}

pub(super) async fn get_username(user_id: uuid::Uuid, ctx: &State<AppContext>) -> Result<String> {
    let username =
        sqlx::query_scalar!(r#"select username from "user" where user_id = $1"#, user_id)
            .fetch_one(&ctx.db)
            .await?;

    Ok(username)
}

#[derive(Serialize, Deserialize)]
struct ImageUpload {
    image: String,
}

async fn upload_image(
    auth_user: AuthUser,
    ctx: State<AppContext>,
    mut multipart: Multipart,
) -> Result<()> {
    let field = multipart
        .next_field()
        .await
        .context("missing image field")?
        .context("missing image field")?;

    let data = field.bytes().await.context("failed to read image data")?;

    let img = load_from_memory(&data).context("failed to load image")?;

    let mut jpeg_data = Cursor::new(Vec::new());

    img.write_to(&mut jpeg_data, image::ImageFormat::Jpeg)
        .context("failed to encode image")?;

    let output_path = format!("images/users/{}.jpg", auth_user.user_id);
    let mut file = File::create(output_path)
        .await
        .context("failed to create image file")?;
    file.write_all(&jpeg_data.into_inner())
        .await
        .context("failed to write image data")?;

    Ok(())
}
