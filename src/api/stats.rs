use anyhow::Context;
use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use chrono::{Datelike, Local, Timelike};
use chrono_tz::Asia::Kolkata;
use sqlx::{Pool, Postgres};

use crate::api::auth::AuthRestaurant;
use crate::api::{AppContext, Result};

pub(crate) fn router() -> Router<AppContext> {
    Router::new().route("/api/stats", get(get_stats))
}

#[derive(serde::Serialize)]
struct Stats {
    total_orders: i64,
    total_revenue: i64,
    average_order_value: f64,
    top_3_items: Vec<(String, i64)>,
    bottom_3_items: Vec<String>,
    orders_per_hour_by_day: [[i64; 24]; 7],
}

async fn get_stats(auth_restaurant: AuthRestaurant, ctx: State<AppContext>) -> Result<Json<Stats>> {
    let total_orders = total_orders(&ctx.db, auth_restaurant.restaurant_id).await?;
    let total_revenue = total_revenue(&ctx.db, auth_restaurant.restaurant_id).await?;
    let average_order_value = average_order_value(&ctx.db, auth_restaurant.restaurant_id).await?;
    let top_3_items = top_3_items(&ctx.db, auth_restaurant.restaurant_id).await?;
    let bottom_3_items = bottom_3_items(&ctx.db, auth_restaurant.restaurant_id).await?;
    let orders_per_hour_by_day =
        orders_per_hour_by_day(&ctx.db, auth_restaurant.restaurant_id).await?;

    Ok(Json(Stats {
        total_orders,
        total_revenue,
        average_order_value,
        top_3_items,
        bottom_3_items,
        orders_per_hour_by_day,
    }))
}

async fn total_orders(db: &Pool<Postgres>, restaurant_id: uuid::Uuid) -> Result<i64> {
    let row = sqlx::query!(
        r#"
        SELECT COUNT(*)
        FROM "order"
        WHERE restaurant_id = $1
        "#,
        restaurant_id
    )
    .fetch_one(db)
    .await
    .context("failed to get total orders")?;

    Ok(row.count.unwrap())
}

async fn total_revenue(db: &Pool<Postgres>, restaurant_id: uuid::Uuid) -> Result<i64> {
    let row = sqlx::query!(
        r#"
        SELECT COALESCE(SUM(total), 0) as total_revenue
        FROM "order"
        WHERE restaurant_id = $1
        "#,
        restaurant_id
    )
    .fetch_one(db)
    .await
    .context("failed to get total revenue")?;

    Ok(row.total_revenue.unwrap())
}

async fn average_order_value(db: &Pool<Postgres>, restaurant_id: uuid::Uuid) -> Result<f64> {
    // get sum and count from db
    let row = sqlx::query!(
        r#"
        SELECT COALESCE(SUM(total), 0) as total_revenue, COUNT(*)
        FROM "order"
        WHERE restaurant_id = $1
        "#,
        restaurant_id
    )
    .fetch_one(db)
    .await
    .context("failed to get average order value")?;

    // calculate average
    let total_revenue = row.total_revenue.unwrap();
    let count = row.count.unwrap();
    let average = if count > 0 {
        total_revenue as f64 / count as f64
    } else {
        0.0
    };
    Ok(average)
}

async fn top_3_items(db: &Pool<Postgres>, restaurant_id: uuid::Uuid) -> Result<Vec<(String, i64)>> {
    let rows = sqlx::query!(
        r#"
        SELECT item_name, sum(quantity) as total_quantity
        FROM "order" natural join "order_item"
        WHERE restaurant_id = $1
        GROUP BY item_name
        ORDER BY sum(quantity) DESC
        LIMIT 3
        "#,
        restaurant_id
    )
    .fetch_all(db)
    .await
    .context("failed to get top 3 items")?;

    Ok(rows
        .into_iter()
        .map(|row| (row.item_name, row.total_quantity.unwrap()))
        .collect())
}

async fn bottom_3_items(db: &Pool<Postgres>, restaurant_id: uuid::Uuid) -> Result<Vec<String>> {
    let rows = sqlx::query!(
        r#"
        SELECT item_name, sum(quantity) as total_quantity
        FROM "order" natural join "order_item"
        WHERE restaurant_id = $1
        GROUP BY item_name
        ORDER BY sum(quantity) ASC
        LIMIT 3
        "#,
        restaurant_id
    )
    .fetch_all(db)
    .await
    .context("failed to get bottom 3 items")?;

    Ok(rows.into_iter().map(|row| row.item_name).collect())
}

async fn orders_per_hour_by_day(
    db: &Pool<Postgres>,
    restaurant_id: uuid::Uuid,
) -> Result<[[i64; 24]; 7]> {
    let rows = sqlx::query!(
        r#"
        SELECT created_at as "created_at!: chrono::DateTime<Local>"
        FROM "order"
        WHERE restaurant_id = $1
        "#,
        restaurant_id
    )
    .fetch_all(db)
    .await
    .context("failed to get orders by hour per day")?;

    let mut orders_by_hour = [[0; 24]; 7];

    for row in rows {
        let created_at = row.created_at.with_timezone(&Kolkata);
        let day = created_at.weekday().num_days_from_monday() as usize;
        let hour = created_at.hour() as usize;
        orders_by_hour[day][hour] += 1;
    }
    todo!()
}
