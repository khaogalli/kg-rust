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
    item_frequency: Vec<(String, i64)>,
    orders_per_hour_by_day: [[i64; 24]; 7],
}

async fn get_stats(auth_restaurant: AuthRestaurant, ctx: State<AppContext>) -> Result<Json<Stats>> {
    let total_orders = total_orders(&ctx.db, auth_restaurant.restaurant_id).await?;
    let total_revenue = total_revenue(&ctx.db, auth_restaurant.restaurant_id).await?;
    let item_frequency = item_frequency(&ctx.db, auth_restaurant.restaurant_id).await?;
    let orders_per_hour_by_day =
        orders_per_hour_by_day(&ctx.db, auth_restaurant.restaurant_id).await?;

    Ok(Json(Stats {
        total_orders,
        total_revenue,
        item_frequency,
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

async fn item_frequency(
    db: &Pool<Postgres>,
    restaurant_id: uuid::Uuid,
) -> Result<Vec<(String, i64)>> {
    let rows = sqlx::query!(
        r#"
        SELECT item_name, sum(quantity) as total_quantity
        FROM "order" natural join "order_item"
        WHERE restaurant_id = $1
        GROUP BY item_name
        ORDER BY sum(quantity) DESC
        "#,
        restaurant_id
    )
    .fetch_all(db)
    .await
    .context("failed to get item frequency")?;

    Ok(rows
        .into_iter()
        .map(|row| (row.item_name, row.total_quantity.unwrap()))
        .collect())
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
    Ok(orders_by_hour)
}
