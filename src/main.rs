use anyhow::Context;
use clap::Parser;
use kg_rust::api;
use kg_rust::config::Config;
use sqlx::postgres::PgPool;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // dotenv returns an error if the .env file is not found
    // but the dotenv file is optional, so we ignore the error
    dotenv::dotenv().ok();

    // Initialize the logger
    env_logger::init();

    // Parse config from command line arguments and environment
    let config = Config::parse();

    let db = PgPool::connect(&config.database_url)
        .await
        .context("could not connect to database_url")?;

    api::serve(config, db).await
}
