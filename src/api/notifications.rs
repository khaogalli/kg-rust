use axum::extract::{Path, State};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use sqlx::query;

use crate::api::auth::{AuthRestaurant, AuthUser};
use crate::api::{AppContext, Result};

pub(crate) fn router() -> Router<AppContext> {
    Router::new()
        .route(
            "/api/notification/restaurant",
            get(get_restaurant_notifications),
        )
        .route("/api/notification/user", get(get_user_notifications))
        .route("/api/notification", post(send_notification))
        .route("/api/notification/delete/:id", delete(delete_notification))
}

#[derive(Serialize)]
pub struct NotificationRestaurant {
    id: uuid::Uuid,
    title: String,
    body: String,
    ttl_minutes: i32,
    created_at: chrono::DateTime<chrono::Utc>,
}

async fn get_restaurant_notifications(
    auth_restaurant: AuthRestaurant,
    ctx: State<AppContext>,
) -> Result<Json<Vec<NotificationRestaurant>>> {
    let restaurant_id = auth_restaurant.restaurant_id;
    Ok(Json(
        query!(
            r#"
        select * from notification
        where sender_id = $1
        and recipient_id is null
        "#,
            restaurant_id
        )
        .fetch_all(&ctx.db)
        .await?
        .into_iter()
        .map(|row| NotificationRestaurant {
            id: row.notification_id,
            title: row.title.clone(),
            body: row.body.clone(),
            ttl_minutes: row.ttl_minutes,
            created_at: row.created_at,
        })
        .collect(),
    ))
}

#[derive(Serialize)]
pub struct NotificationUser {
    id: uuid::Uuid,
    title: String,
    body: String,
    restaurant_name: String,
    restaurant_id: uuid::Uuid,
    ttl_minutes: i32,
    created_at: chrono::DateTime<chrono::Utc>,
}

async fn get_user_notifications(
    auth_user: AuthUser,
    ctx: State<AppContext>,
) -> Result<Json<Vec<NotificationUser>>> {
    let user_id = auth_user.user_id;
    // fetch notification from database
    Ok(Json(
        query!(
            r#"
        select n.notification_id, n.title, n.body, n.ttl_minutes, n.created_at,
        r.restaurant_id, r.name as restaurant_name
        from notification n
        join restaurant r on n.sender_id = r.restaurant_id
        where (recipient_id = $1 or recipient_id is null) and sender_id is not null
        "#,
            user_id
        )
        .fetch_all(&ctx.db)
        .await?
        .into_iter()
        .map(|row| NotificationUser {
            id: row.notification_id,
            title: row.title.clone(),
            body: row.body.clone(),
            restaurant_name: row.restaurant_name.clone(),
            restaurant_id: row.restaurant_id,
            ttl_minutes: row.ttl_minutes,
            created_at: row.created_at,
        })
        .collect(),
    ))
}

#[derive(Deserialize)]
pub struct NewNotification {
    pub title: String,
    pub body: String,
    pub ttl_minutes: i32,
}

async fn send_notification(
    auth_restaurant: AuthRestaurant,
    ctx: State<AppContext>,
    Json(req): Json<NewNotification>,
) -> Result<()> {
    let restaurant_id = auth_restaurant.restaurant_id;
    // insert notification into database
    query!(
        r#"
        insert into notification (sender_id, title, body, ttl_minutes)
        values ($1, $2, $3, $4)
        "#,
        restaurant_id,
        req.title,
        req.body,
        req.ttl_minutes
    )
    .execute(&ctx.db)
    .await?;
    Ok(())
}

async fn delete_notification(
    auth_restaurant: AuthRestaurant,
    ctx: State<AppContext>,
    Path(notification_id): Path<uuid::Uuid>,
) -> Result<()> {
    let restaurant_id = auth_restaurant.restaurant_id;
    // delete notification from database
    query!(
        r#"
        delete from notification
        where sender_id = $1 and recipient_id is null and notification_id = $2
        "#,
        restaurant_id,
        notification_id
    )
    .execute(&ctx.db)
    .await?;
    Ok(())
}
