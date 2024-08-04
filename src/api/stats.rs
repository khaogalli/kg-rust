use std::collections::HashMap;

use ::chrono::{Datelike, Timelike, Utc};
use anyhow::Context;
use axum::extract::{Path, State};
use axum::routing::get;
use axum::{Json, Router};
use chrono::Local;
use chrono_tz::Asia::Kolkata;
use sqlx::types::chrono;
use sqlx::{Pool, Postgres};

use crate::api::auth::AuthRestaurant;
use crate::api::{AppContext, Result};

pub(crate) fn router() -> Router<AppContext> {
    Router::new()
        .route("/api/stats/restaurant", get(get_restaurant_stats))
        .route("/api/stats/user", get(get_user_stats))
        .route(
            "/api/stats/restaurant/custom/days/:start/:end",
            get(get_custom_restaurants_stats),
        )
}

#[derive(serde::Serialize)]
struct RestaurantStats {
    total_orders: i64,
    total_revenue: i64,
    item_frequency: Vec<(String, i64)>,
    orders_per_hour_by_day: [[i64; 24]; 7],
}

async fn get_restaurant_stats(
    auth_restaurant: AuthRestaurant,
    ctx: State<AppContext>,
) -> Result<Json<RestaurantStats>> {
    let total_orders = total_orders_restaurant(&ctx.db, auth_restaurant.restaurant_id).await?;
    let total_revenue = total_revenue_restaurant(&ctx.db, auth_restaurant.restaurant_id).await?;
    let item_frequency = item_frequency_restaurant(&ctx.db, auth_restaurant.restaurant_id).await?;
    let orders_per_hour_by_day =
        orders_per_hour_by_day_restaurant(&ctx.db, auth_restaurant.restaurant_id).await?;

    Ok(Json(RestaurantStats {
        total_orders,
        total_revenue,
        item_frequency,
        orders_per_hour_by_day,
    }))
}

async fn total_orders_restaurant(db: &Pool<Postgres>, restaurant_id: uuid::Uuid) -> Result<i64> {
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

async fn total_revenue_restaurant(db: &Pool<Postgres>, restaurant_id: uuid::Uuid) -> Result<i64> {
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

async fn item_frequency_restaurant(
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

async fn orders_per_hour_by_day_restaurant(
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

#[derive(serde::Serialize)]
struct UserStats {
    total_orders: i64,
    total_spent: i64,
    orders_per_hour_by_day: [[i64; 24]; 7],
    orders_per_day: HashMap<String, i64>,
}

async fn get_user_stats(
    auth_user: crate::api::auth::AuthUser,
    ctx: State<AppContext>,
) -> Result<Json<UserStats>> {
    let total_orders = total_orders_user(&ctx.db, auth_user.user_id).await?;
    let total_spent = total_spent_user(&ctx.db, auth_user.user_id).await?;
    let orders_per_hour_by_day = orders_per_hour_by_day_user(&ctx.db, auth_user.user_id).await?;
    let orders_per_day = orders_per_day_user(&ctx.db, auth_user.user_id).await?;

    Ok(Json(UserStats {
        total_orders,
        total_spent,
        orders_per_hour_by_day,
        orders_per_day,
    }))
}

async fn total_orders_user(db: &Pool<Postgres>, user_id: uuid::Uuid) -> Result<i64> {
    let row = sqlx::query!(
        r#"
        SELECT COUNT(*)
        FROM "order"
        WHERE user_id = $1
        "#,
        user_id
    )
    .fetch_one(db)
    .await
    .context("failed to get total orders")?;

    Ok(row.count.unwrap())
}

async fn total_spent_user(db: &Pool<Postgres>, user_id: uuid::Uuid) -> Result<i64> {
    let row = sqlx::query!(
        r#"
        SELECT COALESCE(SUM(total), 0) as total_spent
        FROM "order"
        WHERE user_id = $1
        "#,
        user_id
    )
    .fetch_one(db)
    .await
    .context("failed to get total spent")?;

    Ok(row.total_spent.unwrap())
}

async fn orders_per_hour_by_day_user(
    db: &Pool<Postgres>,
    user_id: uuid::Uuid,
) -> Result<[[i64; 24]; 7]> {
    let rows = sqlx::query!(
        r#"
        SELECT created_at as "created_at!: chrono::DateTime<Local>"
        FROM "order"
        WHERE user_id = $1
        "#,
        user_id
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

async fn orders_per_day_user(
    db: &Pool<Postgres>,
    user_id: uuid::Uuid,
) -> Result<HashMap<String, i64>> {
    let rows = sqlx::query!(
        r#"
        SELECT created_at
        FROM "order"
        WHERE user_id = $1
        "#,
        user_id
    )
    .fetch_all(db)
    .await
    .context("failed to get orders by day")?;

    let mut orders_by_day = HashMap::new();

    for row in rows {
        let created_at = row.created_at.with_timezone(&Kolkata);
        let day = created_at.format("%Y-%m-%d").to_string();
        *orders_by_day.entry(day).or_insert(0) += 1;
    }
    Ok(orders_by_day)
}

#[derive(serde::Serialize)]
struct RestaurantStatsCustom {
    total_orders: i64,
    total_revenue: i64,
    item_frequency: Vec<(String, i64)>,
    orders_by_day: HashMap<String, i64>,
}

async fn get_custom_restaurants_stats(
    Path(start): Path<String>,
    Path(end): Path<String>,
    auth_restaurant: AuthRestaurant,
    ctx: State<AppContext>,
) -> Result<Json<RestaurantStatsCustom>> {
    let start = chrono::DateTime::parse_from_rfc3339(start.as_str())
        .context("invalid start date")?
        .with_timezone(&Kolkata)
        .with_time(chrono::NaiveTime::from_hms(0, 0, 0))
        .unwrap()
        .to_utc();

    let end = chrono::DateTime::parse_from_rfc3339(end.as_str())
        .context("invalid end date")?
        .with_timezone(&Kolkata)
        .with_time(chrono::NaiveTime::from_hms(23, 59, 59))
        .unwrap()
        .to_utc();

    let total_orders =
        total_orders_custom(&ctx.db, auth_restaurant.restaurant_id, start, end).await?;
    let total_revenue =
        total_revenue_custom(&ctx.db, auth_restaurant.restaurant_id, start, end).await?;
    let item_frequency =
        item_frequency_custom(&ctx.db, auth_restaurant.restaurant_id, start, end).await?;
    let orders_by_day =
        orders_by_day_custom(&ctx.db, auth_restaurant.restaurant_id, start, end).await?;

    Ok(Json(RestaurantStatsCustom {
        total_orders,
        total_revenue,
        item_frequency,
        orders_by_day,
    }))
}

async fn total_orders_custom(
    db: &Pool<Postgres>,
    restaurant_id: uuid::Uuid,
    start: chrono::DateTime<Utc>,
    end: chrono::DateTime<Utc>,
) -> Result<i64> {
    let row = sqlx::query!(
        r#"
        SELECT COUNT(*)
        FROM "order"
        WHERE restaurant_id = $1 AND created_at >= $2 AND created_at <= $3
        "#,
        restaurant_id,
        start,
        end
    )
    .fetch_one(db)
    .await
    .context("failed to get total orders")?;

    Ok(row.count.unwrap())
}

async fn total_revenue_custom(
    db: &Pool<Postgres>,
    restaurant_id: uuid::Uuid,
    start: chrono::DateTime<Utc>,
    end: chrono::DateTime<Utc>,
) -> Result<i64> {
    let row = sqlx::query!(
        r#"
        SELECT COALESCE(SUM(total), 0) as total_revenue
        FROM "order"
        WHERE restaurant_id = $1 AND created_at >= $2 AND created_at <= $3
        "#,
        restaurant_id,
        start,
        end
    )
    .fetch_one(db)
    .await
    .context("failed to get total revenue")?;

    Ok(row.total_revenue.unwrap())
}

async fn item_frequency_custom(
    db: &Pool<Postgres>,
    restaurant_id: uuid::Uuid,
    start: chrono::DateTime<Utc>,
    end: chrono::DateTime<Utc>,
) -> Result<Vec<(String, i64)>> {
    let rows = sqlx::query!(
        r#"
        SELECT item_name, sum(quantity) as total_quantity
        FROM "order" natural join "order_item"
        WHERE restaurant_id = $1 AND created_at >= $2 AND created_at <= $3
        GROUP BY item_name
        ORDER BY sum(quantity) DESC
        "#,
        restaurant_id,
        start,
        end
    )
    .fetch_all(db)
    .await
    .context("failed to get item frequency")?;

    Ok(rows
        .into_iter()
        .map(|row| (row.item_name, row.total_quantity.unwrap()))
        .collect())
}

async fn orders_by_day_custom(
    db: &Pool<Postgres>,
    restaurant_id: uuid::Uuid,
    start: chrono::DateTime<Utc>,
    end: chrono::DateTime<Utc>,
) -> Result<HashMap<String, i64>> {
    let rows = sqlx::query!(
        r#"
        SELECT created_at
        FROM "order"
        WHERE restaurant_id = $1 AND created_at >= $2 AND created_at <= $3
        "#,
        restaurant_id,
        start,
        end
    )
    .fetch_all(db)
    .await
    .context("failed to get orders by day")?;

    let mut orders_by_day = HashMap::new();

    for row in rows {
        let created_at = row.created_at.with_timezone(&Kolkata);
        let day = created_at.format("%Y-%m-%d").to_string();
        *orders_by_day.entry(day).or_insert(0) += 1;
    }
    Ok(orders_by_day)
}
