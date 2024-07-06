use crate::api::auth::{Auth, AuthRestaurant, AuthUser};
use crate::api::util::{hash_password, verify_password};
use crate::api::{Error, Result, ResultExt};
use axum::extract::{Path, State};
use axum::routing::{get, post, put};
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
        .route("/api/restaurants/menu", put(update_menu))
        .route("/api/restaurants/menu/:restaurant_id", get(get_menu))
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

#[derive(serde::Deserialize)]
struct NewItem {
    name: String,
    description: String,
    price: i32,
}

async fn update_menu(
    auth_restaurant: AuthRestaurant,
    ctx: State<AppContext>,
    Json(req): Json<Menu<NewItem>>,
) -> Result<Json<Menu<Item>>> {
    let mut tx = ctx.db.begin().await?;

    sqlx::query!(
        r#"delete from item where restaurant_id = $1"#,
        auth_restaurant.restaurant_id
    )
    .execute(&mut *tx)
    .await?;

    let mut items = Vec::with_capacity(req.menu.len());

    for item in req.menu {
        let record = sqlx::query!(
            r#"insert into item (restaurant_id, name, description, price) values ($1, $2, $3, $4) returning item_id"#,
            auth_restaurant.restaurant_id,
            item.name,
            item.description,
            item.price,
        )
        .fetch_one(&mut *tx)
        .await?;

        items.push(Item {
            id: record.item_id,
            name: item.name,
            description: item.description,
            price: item.price,
        });
    }

    tx.commit().await?;

    Ok(Json(Menu { menu: items }))
}

async fn get_menu(
    _auth: Auth,
    Path(restaurant_id): Path<uuid::Uuid>,
    State(ctx): State<AppContext>,
) -> Result<Json<Menu<Item>>> {
    let items = sqlx::query_as!(
        Item,
        r#"select item_id as "id!", name, description, price from item where restaurant_id = $1"#,
        restaurant_id
    )
    .fetch_all(&ctx.db)
    .await?;

    Ok(Json(Menu { menu: items }))
}
