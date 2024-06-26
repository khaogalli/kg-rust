/// The config for our application
///
/// They can either be passed from the command line or read from the environment
/// The app also loads a `.env` file if it exists
#[derive(clap::Parser)]
pub struct Config {
    /// The connection URL for the postgres server this application should use
    #[clap(long, env)]
    pub database_url: String,

    /// The HMAC secret key used to sign JWT tokens
    #[clap(long, env)]
    pub hmac_key: String,
}
