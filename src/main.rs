use anyhow::Context;
use clap::Parser;
use kg_rust::api;
use kg_rust::config::Config;
use sqlx::postgres::{PgPool, PgPoolOptions};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // dotenv returns an error if the .env file is not found
    // but the dotenv file is optional, so we ignore the error
    dotenvy::dotenv().ok();

    // Initialize the logger
    env_logger::init();

    // Parse config from command line arguments and environment
    let config = Config::parse();

    let db = PgPoolOptions::new()
        .min_connections(config.db_min_connections)
        .max_connections(config.db_max_connections)
        .test_before_acquire(true)
        .connect(&config.database_url)
        .await
        .context("could not connect to database")?;

    sqlx::migrate!()
        .run(&db)
        .await
        .context("could not run migrations")?;

    api::serve(config, db).await
}
