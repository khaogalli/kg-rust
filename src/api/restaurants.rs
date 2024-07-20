use axum::http::header::CONTENT_TYPE;
use axum::http::StatusCode;
use axum::response::{AppendHeaders, IntoResponse, Response};
use base64::prelude::BASE64_STANDARD;
use base64::Engine;
use serde::Deserialize;
use sqlx::query;

use crate::api::auth::{Auth, AuthRestaurant, AuthUser};
use crate::api::util::{hash_password, verify_password};
use crate::api::{Error, Result, ResultExt};
use anyhow::Context;
use axum::extract::{Path, State};
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};

use crate::api::AppContext;

pub(crate) fn router() -> Router<AppContext> {
    Router::new()
        .route("/api/restaurants/list", get(get_restaurants))
        .route("/api/restaurants/login", post(login_restaurant))
        .route(
            "/api/restaurants",
            get(get_current_restaurant).patch(update_restaurant),
        )
        .route("/api/restaurants/menu/:restaurant_id", get(get_menu))
        .route("/api/restaurants/upload_image", post(upload_image))
        .route("/api/restaurants/image/:id", get(get_image))
        .route(
            "/api/restaurants/menu/item",
            post(add_item).patch(update_item),
        )
        .route("/api/restaurants/menu/item/:id", delete(delete_item))
        .route("/api/restaurants/menu/item/image/:id", get(get_item_image))
}

#[derive(serde::Serialize, serde::Deserialize)]
struct RestaurantBody<T> {
    restaurant: T,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct Restaurant {
    id: uuid::Uuid,
    username: String,
    name: String,
    token: String,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct Menu<T> {
    menu: Vec<T>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct Item {
    id: uuid::Uuid,
    name: String,
    description: String,
    price: i32,
    available: bool,
}

#[derive(serde::Serialize)]
struct Restaurants {
    restaurants: Vec<RestaurantInfo>,
}

#[derive(serde::Serialize)]
struct RestaurantInfo {
    id: uuid::Uuid,
    name: String,
}

async fn get_restaurants(_user: AuthUser, ctx: State<AppContext>) -> Result<Json<Restaurants>> {
    let restaurants = sqlx::query_as!(
        RestaurantInfo,
        r#"select restaurant_id as "id!", name from restaurant"#
    )
    .fetch_all(&ctx.db)
    .await?;

    Ok(Json(Restaurants { restaurants }))
}

#[derive(serde::Deserialize)]
struct LoginRestaurant {
    username: String,
    password: String,
}

async fn login_restaurant(
    ctx: State<AppContext>,
    Json(req): Json<RestaurantBody<LoginRestaurant>>,
) -> Result<Json<RestaurantBody<Restaurant>>> {
    let restaurant = sqlx::query!(
        r#"
            select restaurant_id, username, name, password_hash
            from "restaurant" where username = $1
        "#,
        req.restaurant.username,
    )
    .fetch_optional(&ctx.db)
    .await?
    .ok_or_else(|| Error::unprocessable_entity([("username", "does not exist")]))?;

    verify_password(req.restaurant.password, restaurant.password_hash).await?;

    Ok(Json(RestaurantBody {
        restaurant: Restaurant {
            id: restaurant.restaurant_id,
            token: AuthRestaurant {
                restaurant_id: restaurant.restaurant_id,
            }
            .to_jwt(&ctx),
            username: restaurant.username,
            name: restaurant.name,
        },
    }))
}

async fn get_current_restaurant(
    auth_restaurant: AuthRestaurant,
    ctx: State<AppContext>,
) -> Result<Json<RestaurantBody<Restaurant>>> {
    let restaurant = sqlx::query!(
        r#"select username, name from "restaurant" where restaurant_id = $1"#,
        auth_restaurant.restaurant_id
    )
    .fetch_one(&ctx.db)
    .await?;

    Ok(Json(RestaurantBody {
        restaurant: Restaurant {
            id: auth_restaurant.restaurant_id,
            token: auth_restaurant.to_jwt(&ctx),
            username: restaurant.username,
            name: restaurant.name,
        },
    }))
}

#[derive(serde::Deserialize)]
struct UpdateRestaurant {
    username: Option<String>,
    update_pass: Option<UpdatePass>,
    name: Option<String>,
}

#[derive(serde::Deserialize)]
struct UpdatePass {
    old_password: String,
    new_password: String,
}

async fn update_restaurant(
    auth_restaurant: AuthRestaurant,
    ctx: State<AppContext>,
    Json(req): Json<RestaurantBody<UpdateRestaurant>>,
) -> Result<Json<RestaurantBody<Restaurant>>> {
    let mut tx = ctx.db.begin().await?;

    let restaurant = sqlx::query!(
        r#"select username, name, password_hash from "restaurant" where restaurant_id = $1"#,
        auth_restaurant.restaurant_id
    )
    .fetch_one(&mut *tx)
    .await?;

    if let Some(update_pass) = req.restaurant.update_pass {
        verify_password(update_pass.old_password, restaurant.password_hash)
            .await
            .map_err(|e| match e {
                Error::Unauthorized => {
                    Error::unprocessable_entity([("old_password", "old password is incorrect")])
                }
                _ => e,
            })?;
        let new_hash = hash_password(update_pass.new_password).await?;

        sqlx::query!(
            r#"update "restaurant" set password_hash = $1 where restaurant_id = $2"#,
            new_hash,
            auth_restaurant.restaurant_id
        )
        .execute(&mut *tx)
        .await?;
    }

    if let Some(ref username) = req.restaurant.username {
        sqlx::query!(
            r#"update "user" set username = $1 where user_id = $2"#,
            username,
            auth_restaurant.restaurant_id
        )
        .execute(&mut *tx)
        .await
        .on_constraint("user_username_key", |_| {
            Error::unprocessable_entity([("username", "username taken")])
        })?;
    }

    if let Some(ref name) = req.restaurant.name {
        sqlx::query!(
            r#"update "restaurant" set name = $1 where restaurant_id = $2"#,
            name,
            auth_restaurant.restaurant_id
        )
        .execute(&mut *tx)
        .await?;
    }

    tx.commit().await?;

    Ok(Json(RestaurantBody {
        restaurant: Restaurant {
            id: auth_restaurant.restaurant_id,
            token: auth_restaurant.to_jwt(&ctx),
            username: req.restaurant.username.unwrap_or(restaurant.username),
            name: req.restaurant.name.unwrap_or(restaurant.name),
        },
    }))
}

async fn get_menu(
    _auth: Auth,
    Path(restaurant_id): Path<uuid::Uuid>,
    State(ctx): State<AppContext>,
) -> Result<Json<Menu<Item>>> {
    let items = sqlx::query_as!(
        Item,
        r#"select item_id as "id!", name, description, price, available from item where restaurant_id = $1 ORDER BY created_at"#,
        restaurant_id
    )
    .fetch_all(&ctx.db)
    .await?;

    Ok(Json(Menu { menu: items }))
}

pub(super) async fn get_restaurant_name(
    restaurant_id: uuid::Uuid,
    ctx: &State<AppContext>,
) -> Result<String> {
    let name = sqlx::query_scalar!(
        r#"select name from "restaurant" where restaurant_id = $1"#,
        restaurant_id
    )
    .fetch_one(&ctx.db)
    .await?;

    Ok(name)
}

#[derive(Deserialize)]
struct ImageUpload {
    image: String,
}

async fn upload_image(
    auth_restaurant: AuthRestaurant,
    State(ctx): State<AppContext>,
    Json(req): Json<ImageUpload>,
) -> Result<()> {
    let b64_data = req.image;
    let data = BASE64_STANDARD
        .decode(b64_data)
        .context("failed to read image data")?;

    query!(
        "update restaurant set image = $1 where restaurant_id = $2",
        data,
        auth_restaurant.restaurant_id
    )
    .execute(&ctx.db)
    .await
    .context("failed to upload image")?;
    Ok(())
}

async fn get_image(Path(id): Path<uuid::Uuid>, State(ctx): State<AppContext>) -> Result<Response> {
    let res = query!("select image from restaurant where restaurant_id = $1", id)
        .fetch_one(&ctx.db)
        .await?;

    match res.image {
        Some(image) => Ok((AppendHeaders([(CONTENT_TYPE, "image/jpeg")]), image).into_response()),
        None => Ok(StatusCode::NOT_FOUND.into_response()),
    }
}

async fn get_item_image(
    Path(id): Path<uuid::Uuid>,
    State(ctx): State<AppContext>,
) -> Result<Response> {
    let res = query!("select image from item where item_id = $1", id)
        .fetch_one(&ctx.db)
        .await?;

    match res.image {
        Some(image) => Ok((AppendHeaders([(CONTENT_TYPE, "image/jpeg")]), image).into_response()),
        None => Ok(StatusCode::NOT_FOUND.into_response()),
    }
}

#[derive(Deserialize)]
struct ItemBody<T> {
    item: T,
}

#[derive(Deserialize)]

struct UpdatedItem {
    id: uuid::Uuid,
    image: Option<String>,
    name: Option<String>,
    price: Option<i32>,
    description: Option<String>,
    available: Option<bool>,
}

async fn update_item(
    auth_restaurant: AuthRestaurant,
    State(ctx): State<AppContext>,
    Json(req): Json<ItemBody<UpdatedItem>>,
) -> Result<()> {
    let mut tx = ctx.db.begin().await?;

    if let Some(image) = req.item.image {
        let b64_data = image;
        let data = BASE64_STANDARD
            .decode(b64_data)
            .context("failed to read image data")?;
        query!(
            r#"update item set image = $1 where item_id = $2 AND restaurant_id = $3 "#,
            data,
            req.item.id,
            auth_restaurant.restaurant_id
        )
        .execute(&mut *tx)
        .await?;
    };

    if let Some(name) = req.item.name {
        query!(
            r#"update item set name = $1 where item_id = $2 AND restaurant_id = $3 "#,
            name,
            req.item.id,
            auth_restaurant.restaurant_id
        )
        .execute(&mut *tx)
        .await?;
    }

    if let Some(price) = req.item.price {
        query!(
            r#"update item set price = $1 where item_id = $2 AND restaurant_id = $3 "#,
            price,
            req.item.id,
            auth_restaurant.restaurant_id
        )
        .execute(&mut *tx)
        .await?;
    }

    if let Some(description) = req.item.description {
        query!(
            r#"update item set description = $1 where item_id = $2 AND restaurant_id = $3 "#,
            description,
            req.item.id,
            auth_restaurant.restaurant_id
        )
        .execute(&mut *tx)
        .await?;
    }

    if let Some(available) = req.item.available {
        query!(
            r#"update item set available = $1 where item_id = $2 AND restaurant_id = $3 "#,
            available,
            req.item.id,
            auth_restaurant.restaurant_id
        )
        .execute(&mut *tx)
        .await?;
    }

    tx.commit().await?;
    Ok(())
}

async fn delete_item(
    auth_restaurant: AuthRestaurant,
    Path(id): Path<uuid::Uuid>,
    State(ctx): State<AppContext>,
) -> Result<()> {
    query!(
        r#"delete from item where item_id = $1 AND restaurant_id = $2"#,
        id,
        auth_restaurant.restaurant_id
    )
    .execute(&ctx.db)
    .await?;
    Ok(())
}

#[derive(Deserialize)]
pub struct AddItem {
    name: String,
    description: String,
    price: i32,
    image: Option<String>,
}

async fn add_item(
    auth_restaurant: AuthRestaurant,
    State(ctx): State<AppContext>,
    Json(req): Json<ItemBody<AddItem>>,
) -> Result<()> {
    let mut tx = ctx.db.begin().await?;

    let record = query!(
        r#"insert into item (restaurant_id, name, description, price) values ($1, $2, $3, $4) returning item_id"#,
        auth_restaurant.restaurant_id,
        req.item.name,
        req.item.description,
        req.item.price,
    )
    .fetch_one(&mut *tx)
    .await?;

    if let Some(image) = req.item.image {
        let b64_data = image;
        let data = BASE64_STANDARD
            .decode(b64_data)
            .context("failed to read image data")?;
        query!(
            r#"update item set image = $1 where item_id = $2 AND restaurant_id = $3 "#,
            data,
            record.item_id,
            auth_restaurant.restaurant_id
        )
        .execute(&mut *tx)
        .await?;
    };

    tx.commit().await?;
    Ok(())
}
