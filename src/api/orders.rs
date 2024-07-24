use anyhow::Context;
use axum::extract::{Path, State};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::Local;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::api::auth::{Auth, AuthRestaurant, AuthUser};
use crate::api::restaurants::get_restaurant_name;
use crate::api::users::get_username;
use crate::api::AppContext;
use crate::api::Result;

pub(crate) fn router() -> Router<AppContext> {
    Router::new()
        .route("/api/orders", post(make_order))
        .route("/api/orders/pending", get(get_pending_orders))
        .route("/api/orders/complete/:order_id", post(complete_order))
        .route("/api/orders/:days", get(get_orders))
        .route("/api/orders/payment/:order_id", get(get_payment_session))
        .route("/api/orders/payment/verify/:order_id", get(verify_payment))
}

#[derive(serde::Serialize, serde::Deserialize)]
struct OrderBody<T> {
    order: T,
}

#[derive(serde::Serialize, serde::Deserialize)]
pub(super) struct Order {
    id: uuid::Uuid,
    restaurant_id: uuid::Uuid,
    restaurant_name: String,
    user_id: uuid::Uuid,
    user_name: String,
    items: Vec<Item>,
    total: i32,
    pending: bool,
    created_at: chrono::DateTime<Local>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct Item {
    name: String,
    price: i32,
    quantity: i32,
}

#[derive(serde::Deserialize)]
struct NewOrder {
    restaurant_id: uuid::Uuid,
    items: Vec<NewItem>,
}

#[derive(serde::Deserialize)]
struct NewItem {
    id: uuid::Uuid,
    quantity: i32,
}

#[derive(Deserialize, Serialize, Debug)]
struct PaymentResponse {
    cf_order_id: String,
    payment_session_id: String,
    order_status: String,
}

async fn make_order(
    auth_user: AuthUser,
    ctx: State<AppContext>,
    Json(req): Json<OrderBody<NewOrder>>,
) -> Result<Json<OrderBody<Order>>> {
    let mut total = 0;
    let mut items = Vec::new();
    let mut tx = ctx.db.begin().await?;
    for item in &req.order.items {
        // TODO: currently gives a 500 error if the item doesn't exist. fix this.
        let db_item = sqlx::query!(
            r#"select item_id, name, price from item where item_id = $1"#,
            item.id
        )
        .fetch_one(&mut *tx)
        .await?;

        total += db_item.price * item.quantity;
        items.push(Item {
            name: db_item.name,
            price: db_item.price,
            quantity: item.quantity,
        });
    }

    let order = sqlx::query!(
        r#"insert into "order" (restaurant_id, user_id, total) values ($1, $2, $3) returning order_id, created_at"#,
        req.order.restaurant_id,
        auth_user.user_id,
        total
    ).fetch_one(&mut *tx).await?;

    for item in &items {
        sqlx::query!(
            r#"insert into order_item (order_id, item_name, item_price, quantity) values ($1, $2, $3, $4)"#,
            order.order_id,
            item.name,
            item.price,
            item.quantity
        )
        .execute(&mut *tx)
        .await?;
    }

    let rk = sqlx::query!(
        r#"select cashfree_app_id, cashfree_secret_key from restaurant where restaurant_id=$1"#,
        req.order.restaurant_id
    )
    .fetch_one(&mut *tx)
    .await?;

    let client = Client::new();

    let cf_order_data = json!({
        "order_amount": total,
        "order_currency": "INR",
        "customer_details": {
            "customer_id": auth_user.user_id,
            "customer_name": &get_username(auth_user.user_id, &ctx).await?,
            "customer_phone": "+919999999999"
        }
    });

    let response = client
        .post("https://sandbox.cashfree.com/pg/orders")
        .header(
            "X-Client-Secret",
            rk.cashfree_secret_key
                .context("cashfree_secret_key is empyt")?,
        )
        .header(
            "X-Client-Id",
            rk.cashfree_app_id.context("cashfree_app_id is empty")?,
        )
        .header("x-api-version", "2023-08-01")
        .header("Content-Type", "application/json")
        .header("Accept", "application/json")
        .json(&cf_order_data)
        .send()
        .await
        .context("failed to create payment session")?;

    if !response.status().is_success() {
        return Err(anyhow::anyhow!("failed to create payment session").into());
    }

    let response: PaymentResponse = response.json().await.context("failed to parse response")?;

    sqlx::query!(
        r#"insert into "payment" (cf_order_id,order_id,payment_session_id) values ($1,$2,$3)"#,
        response.cf_order_id,
        order.order_id,
        response.payment_session_id
    )
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;

    Ok(Json(OrderBody {
        order: Order {
            id: order.order_id,
            restaurant_id: req.order.restaurant_id,
            restaurant_name: get_restaurant_name(req.order.restaurant_id, &ctx).await?,
            user_id: auth_user.user_id,
            user_name: get_username(auth_user.user_id, &ctx).await?,
            items,
            total,
            pending: true,
            created_at: order.created_at.with_timezone(&chrono::Local),
        },
    }))
}

#[derive(Deserialize)]
struct PaymentStatus {
    order_status: String,
}

async fn get_payment_session(
    Path(order_id): Path<uuid::Uuid>,
    ctx: State<AppContext>,
) -> Result<Json<String>> {
    let payment_session_id = sqlx::query!(
        r#"select payment_session_id from "payment" where order_id = $1"#,
        order_id
    )
    .fetch_one(&ctx.db)
    .await?;

    Ok(Json(payment_session_id.payment_session_id))
}

async fn verify_payment(
    Path(order_id): Path<uuid::Uuid>,
    ctx: State<AppContext>,
) -> Result<impl IntoResponse> {
    let cf_order_id = sqlx::query_scalar!(
        r#"select cf_order_id from "payment" where order_id = $1"#,
        order_id
    )
    .fetch_one(&ctx.db)
    .await?;

    let client = Client::new();

    let response = client
        .get(format!(
            "https://sandbox.cashfree.com/pg/orders/{}",
            cf_order_id
        ))
        .header("x-api-version", "2023-08-01")
        .header("Accept", "application/json")
        .send()
        .await
        .context("failed to verify payment")?;

    if !response.status().is_success() {
        return Err(anyhow::anyhow!("failed to verify payment").into());
    }

    let response: PaymentStatus = response.json().await.context("failed to parse response")?;

    if response.order_status == "PENDING" {
        return Ok("PENDING");
    }

    if response.order_status == "PAID" {
        sqlx::query!(
            r#"update "order" set status = 'paid' where order_id = $1"#,
            order_id
        )
        .execute(&ctx.db)
        .await?;

        sqlx::query!(
            r#"update "payment" set status = 'paid' where order_id = $1"#,
            order_id
        )
        .execute(&ctx.db)
        .await?;

        return Ok("PAID");
    } else {
        sqlx::query!(
            r#"update "order" set status = 'failed' where order_id = $1"#,
            order_id
        )
        .execute(&ctx.db)
        .await?;
        sqlx::query!(
            r#"update "payment" set status = 'failed' where order_id = $1"#,
            order_id
        )
        .execute(&ctx.db)
        .await?;

        return Ok("FAILED");
    }

    Ok("")
}

async fn get_pending_orders(auth: Auth, ctx: State<AppContext>) -> Result<Json<Vec<Order>>> {
    let order = match auth {
        Auth::User(auth_user) => get_pending_orders_user(auth_user, ctx).await?,
        Auth::Restaurant(auth_restaurant) => {
            get_pending_orders_restaurant(auth_restaurant, ctx).await?
        }
    };

    Ok(Json(order))
}

async fn get_pending_orders_user(
    auth_user: AuthUser,
    ctx: State<AppContext>,
) -> Result<Vec<Order>> {
    let db_orders = sqlx::query!(
        r#"select order_id, restaurant_id, total, created_at from "order" where user_id = $1 and pending = true"#,
        auth_user.user_id
    )
    .fetch_all(&ctx.db)
    .await?;

    let mut orders = Vec::with_capacity(db_orders.len());

    for order in db_orders {
        let items = get_items(order.order_id, &ctx).await?;
        orders.push(Order {
            id: order.order_id,
            restaurant_id: order.restaurant_id,
            restaurant_name: get_restaurant_name(order.restaurant_id, &ctx).await?,
            user_id: auth_user.user_id,
            user_name: get_username(auth_user.user_id, &ctx).await?,
            items,
            total: order.total,
            pending: true,
            created_at: order.created_at.with_timezone(&chrono::Local),
        });
    }

    Ok(orders)
}

async fn get_pending_orders_restaurant(
    auth_restaurant: AuthRestaurant,
    ctx: State<AppContext>,
) -> Result<Vec<Order>> {
    let db_orders = sqlx::query!(
        r#"select order_id, user_id, total, created_at from "order" where restaurant_id = $1 and pending = true"#,
        auth_restaurant.restaurant_id
    )
    .fetch_all(&ctx.db)
    .await?;

    let mut orders = Vec::with_capacity(db_orders.len());

    for order in db_orders {
        let items = get_items(order.order_id, &ctx).await?;
        orders.push(Order {
            id: order.order_id,
            restaurant_id: auth_restaurant.restaurant_id,
            restaurant_name: get_restaurant_name(auth_restaurant.restaurant_id, &ctx).await?,
            user_id: order.user_id,
            user_name: get_username(order.user_id, &ctx).await?,
            items,
            total: order.total,
            pending: true,
            created_at: order.created_at.with_timezone(&chrono::Local),
        });
    }

    Ok(orders)
}

async fn get_items(order_id: uuid::Uuid, ctx: &State<AppContext>) -> Result<Vec<Item>> {
    let items = sqlx::query!(
        r#"select item_name, item_price, quantity from order_item where order_id = $1"#,
        order_id
    )
    .fetch_all(&ctx.db)
    .await?;

    Ok(items
        .into_iter()
        .map(|item| Item {
            name: item.item_name,
            price: item.item_price,
            quantity: item.quantity,
        })
        .collect())
}

async fn complete_order(
    auth_restaurant: AuthRestaurant,
    ctx: State<AppContext>,
    Path(order_id): Path<uuid::Uuid>,
) -> Result<()> {
    let mut tx = ctx.db.begin().await?;

    sqlx::query!(
        r#"update "order" set pending = false where order_id = $1 and restaurant_id = $2"#,
        order_id,
        auth_restaurant.restaurant_id
    )
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;

    Ok(())
}

async fn get_orders(
    auth: Auth,
    Path(days): Path<i32>,
    ctx: State<AppContext>,
) -> Result<Json<Vec<Order>>> {
    let orders = match auth {
        Auth::User(auth_user) => get_orders_user(auth_user, days, ctx).await?,
        Auth::Restaurant(auth_restaurant) => {
            get_orders_restaurant(auth_restaurant, days, ctx).await?
        }
    };

    Ok(Json(orders))
}

async fn get_orders_user(
    auth_user: AuthUser,
    days: i32,
    ctx: State<AppContext>,
) -> Result<Vec<Order>> {
    let db_orders = sqlx::query!(
        r#"select order_id, restaurant_id, total, pending, created_at from "order" where user_id = $1 and created_at > now() - interval '1 day' * $2"#,
        auth_user.user_id,
        days as f64
    )
    .fetch_all(&ctx.db)
    .await?;

    let mut orders = Vec::with_capacity(db_orders.len());

    for order in db_orders {
        let items = get_items(order.order_id, &ctx).await?;
        orders.push(Order {
            id: order.order_id,
            restaurant_id: order.restaurant_id,
            restaurant_name: get_restaurant_name(order.restaurant_id, &ctx).await?,
            user_id: auth_user.user_id,
            user_name: get_username(auth_user.user_id, &ctx).await?,
            items,
            total: order.total,
            pending: order.pending,
            created_at: order.created_at.with_timezone(&chrono::Local),
        });
    }

    Ok(orders)
}

async fn get_orders_restaurant(
    auth_restaurant: AuthRestaurant,
    days: i32,
    ctx: State<AppContext>,
) -> Result<Vec<Order>> {
    let db_orders = sqlx::query!(
        r#"select order_id, user_id, total, pending, created_at from "order" where restaurant_id = $1 and created_at > now() - interval '1 day' * $2"#,
        auth_restaurant.restaurant_id,
        days as f64
    )
    .fetch_all(&ctx.db)
    .await?;

    let mut orders = Vec::with_capacity(db_orders.len());

    for order in db_orders {
        let items = get_items(order.order_id, &ctx).await?;
        orders.push(Order {
            id: order.order_id,
            restaurant_id: auth_restaurant.restaurant_id,
            restaurant_name: get_restaurant_name(auth_restaurant.restaurant_id, &ctx).await?,
            user_id: order.user_id,
            user_name: get_username(order.user_id, &ctx).await?,
            items,
            total: order.total,
            pending: order.pending,
            created_at: order.created_at.with_timezone(&chrono::Local),
        });
    }

    Ok(orders)
}
