use anyhow::Context;
use axum::extract::{Path, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{Local, Utc};
use jwt::ToBase64;
use log::info;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::Digest;

use crate::api::auth::{Auth, AuthRestaurant, AuthUser};
use crate::api::notifications::{new_notification, Notification};
use crate::api::restaurants::{
    get_restaurant_name, get_restaurant_phonpe_details, PhonepeMerchant,
};
use crate::api::users::get_username;
use crate::api::AppContext;
use crate::api::Result;

const HOST: &str = "https://api-preprod.phonepe.com/apis/pg-sandbox";

pub(crate) fn router() -> Router<AppContext> {
    Router::new()
        .route("/api/orders", post(make_order))
        .route("/api/orders/complete/:order_id", post(complete_order))
        .route("/api/orders/:days", get(get_orders))
        .route("/api/orders/payment/:order_id", get(get_payment_session))
        .route("/api/orders/cancel/:order_id", post(cancel_order))
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
    status: String,
    created_at: chrono::DateTime<Utc>,
    order_placed_time: Option<chrono::DateTime<Utc>>,
    order_completed_time: Option<chrono::DateTime<Utc>>,
    time_taken: Option<i32>,
    avg_wait_time: Option<i32>,
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
            status: "payment_pending".into(),
            created_at: order.created_at,
            order_placed_time: None,
            order_completed_time: None,
            time_taken: None,
            avg_wait_time: None,
        },
    }))
}

fn calc_xverify(parts: &[&str], index: &str) -> String {
    let mut hasher = sha2::Sha256::new();
    for part in parts {
        hasher.update(part);
    }
    let result = hasher.finalize();
    let mut xverify = hex::encode(result);
    xverify.push_str("###");
    xverify.push_str(index);
    xverify
}

#[derive(Serialize, Deserialize)]
enum PaymentStatus {
    Paid,
    Pending,
    Failed,
}

#[derive(Serialize, Deserialize)]
struct Payment {
    status: PaymentStatus,
    url: Option<String>,
}

async fn get_payment_session(
    auth_user: AuthUser,
    Path(order_id): Path<uuid::Uuid>,
    ctx: State<AppContext>,
) -> Result<Json<Payment>> {
    let order = sqlx::query!(
        r#"select total, status, payment_url, restaurant_id from "order" where order_id = $1"#,
        order_id
    )
    .fetch_one(&ctx.db)
    .await?;

    if order.status == "payment_failed" {
        return Ok(Json(Payment {
            status: PaymentStatus::Failed,
            url: None,
        }));
    }

    if order.status != "payment_pending" {
        return Ok(Json(Payment {
            status: PaymentStatus::Paid,
            url: None,
        }));
    }

    let merchant_info = get_restaurant_phonpe_details(order.restaurant_id, &ctx).await?;

    let oid = order_id.to_string().replace("-", "");
    let user_id = auth_user.user_id.to_string().replace("-", "");

    match order.payment_url {
        Some(url) => {
            let status = verify_payment(oid, merchant_info).await?;
            match status {
                PaymentStatus::Paid => {
                    sqlx::query!(
                        r#"update "order" set status = 'paid', order_placed_time = $1 where order_id = $2"#,
                        Utc::now(),
                        order_id
                    )
                    .execute(&ctx.db)
                    .await?;
                    Ok(Json(Payment {
                        status: PaymentStatus::Paid,
                        url: None,
                    }))
                }
                PaymentStatus::Pending => Ok(Json(Payment {
                    status: PaymentStatus::Pending,
                    url: Some(url),
                })),
                PaymentStatus::Failed => {
                    sqlx::query!(
                        r#"update "order" set status = 'payment_failed' where order_id = $1"#,
                        order_id
                    )
                    .execute(&ctx.db)
                    .await?;
                    Ok(Json(Payment {
                        status: PaymentStatus::Failed,
                        url: Some(url),
                    }))
                }
            }
        }
        None => {
            let data = json!({
              "merchantId": merchant_info.id,
              "merchantTransactionId": oid,
              "merchantUserId": user_id,
              "amount": order.total.to_string()+"00",
              "redirectUrl": "https://khaogalli.me/static/payments.html",
              "redirectMode": "REDIRECT",
              "callbackUrl": "https://webhook.site/callback-url",
              "paymentInstrument": {
                "type": "PAY_PAGE"
              }
            });

            let request_data = data
                .to_base64()
                .context("failed to convert json data to base64")?;

            let request = json!({
                "request": request_data
            });

            let client = Client::new();

            let xverify = calc_xverify(
                &[&request_data, "/pg/v1/pay", &merchant_info.key],
                &merchant_info.key_id,
            );

            let response = client
                .post(format!("{}/pg/v1/pay", HOST))
                .header("Content-Type", "application/json")
                .header("X-VERIFY", xverify)
                .json(&request)
                .send()
                .await
                .context("failed to make phonepe api call")?;

            if !response.status().is_success() {
                return Err(anyhow::anyhow!(format!(
                    "got error status while calling phonepe: {}",
                    response.text().await.context("couldnte get text")?
                ))
                .into());
            }

            let response: serde_json::Value =
                response.json().await.context("failed to parse response")?;

            info!("response: {:?}", response);

            let url = response["data"]["instrumentResponse"]["redirectInfo"]["url"]
                .as_str()
                .context("weird url")?;

            sqlx::query!(
                r#"update "order" set payment_url = $1 where order_id = $2"#,
                url,
                order_id
            )
            .execute(&ctx.db)
            .await?;

            Ok(Json(Payment {
                status: PaymentStatus::Pending,
                url: Some(url.into()),
            }))
        }
    }
}

async fn verify_payment(oid: String, merchant_info: PhonepeMerchant) -> Result<PaymentStatus> {
    let client = Client::new();
    let api_url = format!("{}/pg/v1/status/{}/{}", HOST, merchant_info.id, oid);
    let xverify = calc_xverify(
        &[
            &format!("/pg/v1/status/{}/{}", merchant_info.id, oid),
            &merchant_info.key,
        ],
        &merchant_info.key_id,
    );
    let response = client
        .get(api_url)
        .header("Content-Type", "application/json")
        .header("X-VERIFY", xverify)
        .header("X-MERCHANT-ID", merchant_info.id)
        .send()
        .await
        .context("failed to api request to phonepe")?;

    if !response.status().is_success() {
        return Err(anyhow::anyhow!(format!(
            "got error status while calling phonepe: {}",
            response.text().await.context("couldn't get text")?
        ))
        .into());
    }

    let response: serde_json::Value = response.json().await.context("failed to parse response")?;

    dbg!(response.to_string());
    let code = response
        .get("code")
        .context("field code missing from phonepe response")?
        .as_str()
        .context("weird field code")?;
    match code {
        "PAYMENT_SUCCESS" => Ok(PaymentStatus::Paid),
        "PAYMENT_PENDING" => Ok(PaymentStatus::Pending),
        "PAYMENT_DECLINED" | "TIMED_OUT" => Ok(PaymentStatus::Failed),
        _ => Err(anyhow::anyhow!(format!(
            "unknown payment status code in phonepe response: {}",
            code
        ))
        .into()),
    }
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

    let x = sqlx::query!(
        r#"update "order" 
               set status = 'completed', 
                   order_completed_time = now(), 
                   time_taken = extract(epoch from now() - order_placed_time)
               where order_id = $1 
                 and restaurant_id = $2 
                 and status = 'paid' 
               returning user_id"#,
        order_id,
        auth_restaurant.restaurant_id
    )
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    new_notification(
        ctx,
        Notification {
            sender_id: Some(auth_restaurant.restaurant_id),
            recipient_id: Some(x.user_id),
            title: "Order Completed".into(),
            body: "Your order has been completed".into(),
            ttl_minutes: 24 * 60,
        },
    )
    .await?;

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
        r#"select order_id, restaurant_id, total, status, created_at, order_placed_time, order_completed_time, time_taken from "order" where user_id = $1 and created_at > now() - interval '1 day' * $2 and status in ('completed','paid', 'cancelled')"#,
        auth_user.user_id,
        days as f64
    )
    .fetch_all(&ctx.db)
    .await?;

    let mut orders = Vec::with_capacity(db_orders.len());

    for order in db_orders {
        let items = get_items(order.order_id, &ctx).await?;
        let avg_wait_time = if order.status == "paid" {
            let avg_wait_time = sqlx::query!(
                r#"select TRUNC(avg(time_taken)) as "avg_wait_time: i32" from "order" where user_id = $1 and status = 'completed'"#,
                auth_user.user_id
            )
            .fetch_one(&ctx.db)
            .await?;
            Some(avg_wait_time.avg_wait_time.unwrap_or(0))
        } else {
            None
        };

        orders.push(Order {
            id: order.order_id,
            restaurant_id: order.restaurant_id,
            restaurant_name: get_restaurant_name(order.restaurant_id, &ctx).await?,
            user_id: auth_user.user_id,
            user_name: get_username(auth_user.user_id, &ctx).await?,
            items,
            total: order.total,
            status: order.status,
            created_at: order.created_at,
            order_placed_time: order.order_placed_time,
            order_completed_time: order.order_completed_time,
            time_taken: order.time_taken,
            avg_wait_time,
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
        r#"select order_id, user_id, total, status, created_at, order_placed_time, order_completed_time, time_taken from "order" where restaurant_id = $1 and created_at > now() - interval '1 day' * $2 and status in ('completed','paid')"#,
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
            status: order.status,
            created_at: order.created_at,
            order_placed_time: order.order_placed_time,
            order_completed_time: order.order_completed_time,
            time_taken: order.time_taken,
            avg_wait_time: None,
        });
    }

    Ok(orders)
}

async fn cancel_order(
    auth: Auth,
    Path(order_id): Path<uuid::Uuid>,
    ctx: State<AppContext>,
) -> Result<Json<bool>> {
    // allow user to cancel only if order is not completed and less than 1 minute old
    // and allow restaurant to cancel any time before completing

    match auth {
        Auth::User(auth_user) => cancel_order_user(auth_user, order_id, ctx).await,
        Auth::Restaurant(auth_restaurant) => {
            cancel_order_restaurant(auth_restaurant, order_id, ctx).await
        }
    }
}

async fn cancel_order_user(
    auth_user: AuthUser,
    order_id: uuid::Uuid,
    ctx: State<AppContext>,
) -> Result<Json<bool>> {
    let order = sqlx::query!(
        r#"select status, order_placed_time from "order" where order_id = $1 and user_id = $2"#,
        order_id,
        auth_user.user_id
    )
    .fetch_one(&ctx.db)
    .await?;

    if order.status != "paid" {
        return Ok(Json(false));
    }

    let order_time = if let Some(order_time) = order.order_placed_time {
        order_time
    } else {
        return Ok(Json(false));
    };

    let now = Utc::now();
    if now.signed_duration_since(order_time).num_minutes() > 1 {
        return Ok(Json(false));
    }

    sqlx::query!(
        r#"update "order" set status = 'cancelled' where order_id = $1"#,
        order_id
    )
    .execute(&ctx.db)
    .await?;

    Ok(Json(true))
}

async fn cancel_order_restaurant(
    auth_restaurant: AuthRestaurant,
    order_id: uuid::Uuid,
    ctx: State<AppContext>,
) -> Result<Json<bool>> {
    let order = sqlx::query!(
        r#"select status from "order" where order_id = $1 and restaurant_id = $2"#,
        order_id,
        auth_restaurant.restaurant_id
    )
    .fetch_one(&ctx.db)
    .await?;

    if order.status != "paid" {
        return Ok(Json(false));
    }

    let x = sqlx::query!(
        r#"update "order" set status = 'cancelled' where order_id = $1 returning user_id"#,
        order_id
    )
    .fetch_one(&ctx.db)
    .await?;

    new_notification(
        ctx,
        Notification {
            sender_id: Some(auth_restaurant.restaurant_id),
            recipient_id: Some(x.user_id),
            title: "Order Cancelled".into(),
            body: "Your order has been cancelled by restaurant".into(),
            ttl_minutes: 24 * 60,
        },
    )
    .await?;

    Ok(Json(true))
}
