use std::convert::Infallible;
use std::net::SocketAddr;
use std::path::PathBuf;

use rand::distributions::DistString;

#[derive(clap::Parser)]
pub struct Options {
    /// Address to listen on for WebSocket server.
    #[clap(long, value_parser, default_value = "127.0.0.1:8080")]
    pub addr: SocketAddr,

    /// Serve the WebSocket server over TLS (WSS instead of WS).
    #[clap(long, action)]
    pub tls:   bool,
    /// Path to TLS certificate file. Only used if `--tls` is enabled.
    #[clap(long, value_parser)]
    pub certs: Vec<PathBuf>,
    /// Path to TLS key file. Only used if `--tls` is enabled.
    #[clap(long, value_parser)]
    pub key:   Option<PathBuf>,

    /// Require an auth token from clients.
    ///
    /// The auth token is a randomly generated 16-character alphanumeric string
    /// if no value is provided.
    #[clap(long, value_parser = fill_with_random)]
    pub auth_token: String,
}

fn fill_with_random(input: &str) -> Result<String, Infallible> {
    if input.is_empty() {
        Ok(rand::distributions::Alphanumeric.sample_string(&mut rand::thread_rng(), 16))
    } else {
        Ok(String::from(input))
    }
}
