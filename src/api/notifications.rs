use anyhow::Context;
use axum::extract::{Path, State};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use expo_push_notification_client::{Expo, ExpoPushMessage};
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
struct NewNotification {
    title: String,
    body: String,
    ttl_minutes: i32,
}

async fn send_notification(
    auth_restaurant: AuthRestaurant,
    ctx: State<AppContext>,
    Json(req): Json<NewNotification>,
) -> Result<()> {
    let restaurant_id = auth_restaurant.restaurant_id;

    let notification = Notification {
        sender_id: Some(restaurant_id),
        recipient_id: None,
        title: req.title,
        body: req.body,
        ttl_minutes: req.ttl_minutes,
    };
    new_notification(ctx, notification).await?;

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

#[allow(unused)]
pub(crate) struct Notification {
    pub sender_id: Option<uuid::Uuid>,
    pub recipient_id: Option<uuid::Uuid>,
    pub title: String,
    pub body: String,
    pub ttl_minutes: i32,
}

pub(crate) async fn new_notification(
    ctx: State<AppContext>,
    notification: Notification,
) -> Result<()> {
    let mut tx = ctx.db.begin().await?;
    // insert notification into database
    query!(
        r#"
        insert into notification (sender_id, recipient_id, title, body, ttl_minutes)
        values ($1, $2, $3, $4, $5)
        "#,
        notification.sender_id,
        notification.recipient_id,
        notification.title,
        notification.body,
        notification.ttl_minutes
    )
    .execute(&mut *tx)
    .await?;

    send_expo_notification(ctx, notification).await?;

    tx.commit().await?;

    Ok(())
}

async fn send_expo_notification(ctx: State<AppContext>, notification: Notification) -> Result<()> {
    let expo_push_tokens = match notification.recipient_id {
        Some(recipient_id) => query!(
            r#"
            select expo_push_token as "expo_push_token!: String"
            from "user"
            where user_id = $1 and expo_push_token is not null
            "#,
            recipient_id
        )
        .fetch_all(&ctx.db)
        .await?
        .into_iter()
        .map(|row| row.expo_push_token)
        .collect::<Vec<String>>(),
        None => query!(
            r#"
            select expo_push_token as "expo_push_token!: String"
            from "user"
            where expo_push_token is not null
            "#,
        )
        .fetch_all(&ctx.db)
        .await?
        .into_iter()
        .map(|row| row.expo_push_token)
        .collect::<Vec<String>>(),
    };
    if expo_push_tokens.is_empty() {
        return Ok(());
    }

    let expo = Expo::new(Default::default());
    let message = ExpoPushMessage::builder(expo_push_tokens)
        .title(notification.title)
        .body(notification.body)
        .build();

    expo.send_push_notifications(message)
        .await
        .context("failed to send push notification")?;

    Ok(())
}
