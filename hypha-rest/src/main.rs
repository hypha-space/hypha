use std::{error::Error, net::SocketAddr};

use axum::serve::serve;
use tokio::net::TcpListener;
use tracing::{info, level_filters::LevelFilter};

use crate::{
    service::service,
    state::{State, persistence::RocksDBPersistence},
};

mod api;
mod service;
mod state;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::builder()
                .with_default_directive(LevelFilter::INFO.into())
                .from_env_lossy(),
        )
        .init();

    let state = State::new(RocksDBPersistence::try_new("state_db")?);

    let listener = TcpListener::bind("[::1]:8080").await?;
    info!(address = %listener.local_addr()?, "serving");
    serve(
        listener,
        service(state).into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;

    Ok(())
}
