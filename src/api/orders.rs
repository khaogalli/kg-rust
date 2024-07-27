use anyhow::Context;
use axum::extract::{Path, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::Local;
use jwt::ToBase64;
use log::info;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::Digest;

use crate::api::auth::{Auth, AuthRestaurant, AuthUser};
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
        r#"insert into "order" (restaurant_id, user_id, total) values ($1, $2, $3) returning order_id, created_at as "created_at!: chrono::DateTime<Local>""#,
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
                        r#"update "order" set status = 'paid' where order_id = $1"#,
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
              "redirectUrl": "https://webhook.site/redirect-url",
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

    sqlx::query!(
        r#"update "order" set status = 'completed' where order_id = $1 and restaurant_id = $2 and status='paid'"#,
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
        r#"select order_id, restaurant_id, total, status, created_at as "created_at!: chrono::DateTime<Local>" from "order" where user_id = $1 and created_at > now() - interval '1 day' * $2 and status in ('completed','paid')"#,
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
            status: order.status,
            created_at: order.created_at,
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
        r#"select order_id, user_id, total, status, created_at as "created_at!: chrono::DateTime<Local>" from "order" where restaurant_id = $1 and created_at > now() - interval '1 day' * $2 and status in ('completed','paid')"#,
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
        });
    }

    Ok(orders)
}
