use axum::extract::{Path, State};
use axum::routing::{get, post};
use axum::{Json, Router};

use crate::api::auth::{Auth, AuthRestaurant, AuthUser};
use crate::api::AppContext;
use crate::api::Result;

pub(crate) fn router() -> Router<AppContext> {
    Router::new()
        .route("/api/orders", post(make_order))
        .route("/api/orders/pending", get(get_pending_orders))
        .route("/api/orders/complete/:order_id", post(complete_order))
        .route("/api/orders/:days", get(get_orders))
}

#[derive(serde::Serialize, serde::Deserialize)]
struct OrderBody<T> {
    order: T,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct Order {
    id: uuid::Uuid,
    restaurant_id: uuid::Uuid,
    user_id: uuid::Uuid,
    items: Vec<Item>,
    total: i32,
    pending: bool,
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

    let order = sqlx::query_scalar!(
        r#"insert into "order" (restaurant_id, user_id, total) values ($1, $2, $3) returning order_id"#,
        req.order.restaurant_id,
        auth_user.user_id,
        total
    ).fetch_one(&mut *tx).await?;

    for item in &items {
        sqlx::query!(
            r#"insert into order_item (order_id, item_name, item_price, quantity) values ($1, $2, $3, $4)"#,
            order,
            item.name,
            item.price,
            item.quantity
        )
        .execute(&mut *tx)
        .await?;
    }

    tx.commit().await?;

    Ok(Json(OrderBody {
        order: Order {
            id: order,
            restaurant_id: req.order.restaurant_id,
            user_id: auth_user.user_id,
            items,
            total,
            pending: true,
        },
    }))
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
        r#"select order_id, restaurant_id, total from "order" where user_id = $1 and pending = true"#,
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
            user_id: auth_user.user_id,
            items,
            total: order.total,
            pending: true,
        });
    }

    Ok(orders)
}

async fn get_pending_orders_restaurant(
    auth_restaurant: AuthRestaurant,
    ctx: State<AppContext>,
) -> Result<Vec<Order>> {
    let db_orders = sqlx::query!(
        r#"select order_id, user_id, total from "order" where restaurant_id = $1 and pending = true"#,
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
            user_id: order.user_id,
            items,
            total: order.total,
            pending: true,
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
        r#"select order_id, restaurant_id, total from "order" where user_id = $1 and pending = false and created_at > now() - interval '1 day' * $2"#,
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
            user_id: auth_user.user_id,
            items,
            total: order.total,
            pending: false,
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
        r#"select order_id, user_id, total from "order" where restaurant_id = $1 and pending = false and created_at > now() - interval '1 day' * $2"#,
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
            user_id: order.user_id,
            items,
            total: order.total,
            pending: false,
        });
    }

    Ok(orders)
}
