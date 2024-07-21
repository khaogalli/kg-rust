use std::io::Cursor;

use anyhow::Context;
use axum::extract::{Path, State};
use axum::http::header::CONTENT_TYPE;
use axum::http::StatusCode;
use axum::response::{AppendHeaders, IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use image::imageops::FilterType::Nearest;
use image::ImageFormat;
use serde::Deserialize;
use sqlx::query;

use crate::api::auth::AuthUser;
use crate::api::util::{hash_password, image_from_base64, verify_password};
use crate::api::Result;
use crate::api::{AppContext, Error, ResultExt};

pub(crate) fn router() -> Router<AppContext> {
    Router::new()
        .route("/api/users", post(create_user))
        .route("/api/users/login", post(login_user))
        .route("/api/users", get(get_current_user).patch(update_user))
        .route("/api/users/upload_image", post(upload_image))
        .route("/api/users/image/:id", get(get_image))
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

#[derive(Deserialize)]
struct ImageUpload {
    image: String,
}

async fn upload_image(
    auth_user: AuthUser,
    State(ctx): State<AppContext>,
    Json(req): Json<ImageUpload>,
) -> Result<()> {
    let image = image_from_base64(&req.image)?;
    let image = image.resize(160, 160, Nearest);
    let mut cursor = Cursor::new(Vec::new());
    image
        .write_to(&mut cursor, ImageFormat::Jpeg)
        .context("failed to encode image")?;
    query!(
        r#"update "user" set image = $1 where user_id = $2"#,
        cursor.into_inner(),
        auth_user.user_id
    )
    .execute(&ctx.db)
    .await
    .context("failed to upload image")?;
    Ok(())
}

async fn get_image(Path(id): Path<uuid::Uuid>, State(ctx): State<AppContext>) -> Result<Response> {
    let res = query!(r#"select image from "user" where user_id = $1"#, id)
        .fetch_one(&ctx.db)
        .await?;

    match res.image {
        Some(image) => Ok((AppendHeaders([(CONTENT_TYPE, "image/jpeg")]), image).into_response()),
        None => Ok(StatusCode::NOT_FOUND.into_response()),
    }
}
