use crate::api::Error;
use axum::extract::{FromRef, FromRequestParts};
use axum::http::request::Parts;
use chrono::Utc;

use crate::api::AppContext;
use async_trait::async_trait;
use axum::http::header::AUTHORIZATION;
use axum::http::HeaderValue;
use hmac::{Hmac, NewMac};
use jwt::{SignWithKey, VerifyWithKey};
use sha2::Sha384;
use uuid::Uuid;

const DEFAULT_SESSION_LENGTH: chrono::Duration = chrono::Duration::weeks(1);

const SCHEME_PREFIX: &str = "Bearer ";

pub struct AuthUser {
    pub user_id: Uuid,
}
pub struct MaybeAuthUser(pub Option<AuthUser>);

#[derive(serde::Serialize, serde::Deserialize)]
struct AuthUserClaims {
    user_id: Uuid,
    /// Standard JWT `exp` claim.
    exp: i64,
}

impl AuthUser {
    pub(in crate::api) fn to_jwt(&self, ctx: &AppContext) -> String {
        let hmac = Hmac::<Sha384>::new_from_slice(ctx.config.hmac_key.as_bytes())
            .expect("HMAC-SHA-384 can accept any key length");

        AuthUserClaims {
            user_id: self.user_id,
            exp: (Utc::now() + DEFAULT_SESSION_LENGTH).timestamp(),
        }
        .sign_with_key(&hmac)
        .expect("HMAC signing should be infallible")
    }

    /// Attempt to parse `Self` from an `Authorization` header.
    fn from_authorization(ctx: &AppContext, auth_header: &HeaderValue) -> Result<Self, Error> {
        let auth_header = auth_header.to_str().map_err(|_| {
            log::debug!("Authorization header is not UTF-8");
            Error::Unauthorized
        })?;

        if !auth_header.starts_with(SCHEME_PREFIX) {
            log::debug!(
                "Authorization header is using the wrong scheme: {:?}",
                auth_header
            );
            return Err(Error::Unauthorized);
        }

        let token = &auth_header[SCHEME_PREFIX.len()..];

        let jwt = jwt::Token::<jwt::Header, AuthUserClaims, _>::parse_unverified(token)
            .map_err(|_| Error::Unauthorized)?;

        let hmac = Hmac::<Sha384>::new_from_slice(ctx.config.hmac_key.as_bytes())
            .expect("HMAC-SHA-384 can accept any key length");

        let jwt = jwt.verify_with_key(&hmac).map_err(|e| {
            log::debug!("JWT failed to verify: {}", e);
            Error::Unauthorized
        })?;

        let (_header, claims) = jwt.into();

        if claims.exp < Utc::now().timestamp() {
            log::debug!("token expired");
            return Err(Error::Unauthorized);
        }

        Ok(Self {
            user_id: claims.user_id,
        })
    }
}

impl MaybeAuthUser {
    /// If this is `Self(Some(AuthUser))`, return `AuthUser::user_id`
    pub fn restaurant_id(&self) -> Option<Uuid> {
        self.0.as_ref().map(|auth_user| auth_user.user_id)
    }
}

#[async_trait]
impl<S> FromRequestParts<S> for AuthUser
where
    S: Send + Sync,
    AppContext: FromRef<S>,
{
    type Rejection = Error;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let ctx: AppContext = AppContext::from_ref(state);

        // Get the value of the `Authorization` header, if it was sent at all.
        let auth_header = parts
            .headers
            .get(AUTHORIZATION)
            .ok_or(Error::Unauthorized)?;

        Self::from_authorization(&ctx, auth_header)
    }
}

#[async_trait]
impl<S> FromRequestParts<S> for MaybeAuthUser
where
    S: Send + Sync,
    AppContext: FromRef<S>,
{
    type Rejection = Error;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let ctx: AppContext = AppContext::from_ref(state);

        Ok(Self(
            // Get the value of the `Authorization` header, if it was sent at all.
            parts
                .headers
                .get(AUTHORIZATION)
                .map(|auth_header| AuthUser::from_authorization(&ctx, auth_header))
                .transpose()?,
        ))
    }
}

// =========

pub struct AuthRestaurant {
    pub restaurant_id: Uuid,
}

pub struct MaybeAuthRestaurant(pub Option<AuthRestaurant>);

#[derive(serde::Serialize, serde::Deserialize)]
struct AuthRestaurantClaims {
    restaurant_id: Uuid,
    exp: i64,
}

impl AuthRestaurant {
    pub(in crate::api) fn to_jwt(&self, ctx: &AppContext) -> String {
        let hmac = Hmac::<Sha384>::new_from_slice(ctx.config.hmac_key.as_bytes())
            .expect("HMAC-SHA-384 can accept any key length");

        AuthRestaurantClaims {
            restaurant_id: self.restaurant_id,
            exp: (Utc::now() + DEFAULT_SESSION_LENGTH).timestamp(),
        }
        .sign_with_key(&hmac)
        .expect("HMAC signing should be infallible")
    }

    /// Attempt to parse `Self` from an `Authorization` header.
    fn from_authorization(ctx: &AppContext, auth_header: &HeaderValue) -> Result<Self, Error> {
        let auth_header = auth_header.to_str().map_err(|_| {
            log::debug!("Authorization header is not UTF-8");
            Error::Unauthorized
        })?;

        if !auth_header.starts_with(SCHEME_PREFIX) {
            log::debug!(
                "Authorization header is using the wrong scheme: {:?}",
                auth_header
            );
            return Err(Error::Unauthorized);
        }

        let token = &auth_header[SCHEME_PREFIX.len()..];

        let jwt = jwt::Token::<jwt::Header, AuthRestaurantClaims, _>::parse_unverified(token)
            .map_err(|e| {
                log::debug!(
                    "failed to parse Authorization header {:?}: {}",
                    auth_header,
                    e
                );
                Error::Unauthorized
            })?;

        let hmac = Hmac::<Sha384>::new_from_slice(ctx.config.hmac_key.as_bytes())
            .expect("HMAC-SHA-384 can accept any key length");

        let jwt = jwt.verify_with_key(&hmac).map_err(|e| {
            log::debug!("JWT failed to verify: {}", e);
            Error::Unauthorized
        })?;

        let (_header, claims) = jwt.into();

        if claims.exp < Utc::now().timestamp() {
            log::debug!("token expired");
            return Err(Error::Unauthorized);
        }

        Ok(Self {
            restaurant_id: claims.restaurant_id,
        })
    }
}

impl MaybeAuthRestaurant {
    /// If this is `Self(Some(AuthUser))`, return `AuthUser::user_id`
    pub fn user_id(&self) -> Option<Uuid> {
        self.0
            .as_ref()
            .map(|auth_restaurant: &AuthRestaurant| auth_restaurant.restaurant_id)
    }
}

#[async_trait]
impl<S> FromRequestParts<S> for AuthRestaurant
where
    S: Send + Sync,
    AppContext: FromRef<S>,
{
    type Rejection = Error;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let ctx: AppContext = AppContext::from_ref(state);

        // Get the value of the `Authorization` header, if it was sent at all.
        let auth_header = parts
            .headers
            .get(AUTHORIZATION)
            .ok_or(Error::Unauthorized)?;

        Self::from_authorization(&ctx, auth_header)
    }
}

#[async_trait]
impl<S> FromRequestParts<S> for MaybeAuthRestaurant
where
    S: Send + Sync,
    AppContext: FromRef<S>,
{
    type Rejection = Error;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let ctx: AppContext = AppContext::from_ref(state);

        Ok(Self(
            // Get the value of the `Authorization` header, if it was sent at all.
            parts
                .headers
                .get(AUTHORIZATION)
                .map(|auth_header| AuthRestaurant::from_authorization(&ctx, auth_header))
                .transpose()?,
        ))
    }
}

pub enum Auth {
    User(AuthUser),
    Restaurant(AuthRestaurant),
}

#[async_trait]
impl<S> FromRequestParts<S> for Auth
where
    S: Send + Sync,
    AppContext: FromRef<S>,
{
    type Rejection = Error;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        if let Ok(user) = AuthUser::from_request_parts(parts, state).await {
            return Ok(Self::User(user));
        }
        if let Ok(restaurant) = AuthRestaurant::from_request_parts(parts, state).await {
            return Ok(Self::Restaurant(restaurant));
        }

        Err(Error::Unauthorized)
    }
}
