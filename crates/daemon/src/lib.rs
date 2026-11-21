pub mod config;
pub mod error;
pub mod relay_client;
pub mod responder;
pub mod server;
pub mod store;

use std::{collections::HashMap, fs, net::SocketAddr, sync::Arc};

use tokio::{
    net::TcpListener,
    sync::{broadcast, mpsc, watch, RwLock},
};
use tracing::info;

use crate::{
    config::AgentLinkHome,
    error::Result,
    relay_client::run_relay_loop,
    server::{build_router, AppState},
    store::Store,
};

pub struct DaemonHandle {
    pub addr: SocketAddr,
    pub token: String,
    pub shutdown: watch::Sender<bool>,
}

/// Start the local daemon on `127.0.0.1:<port>` (pass port `0` in tests to bind an ephemeral port).
pub async fn start_daemon(home: &AgentLinkHome, port_override: Option<u16>) -> Result<DaemonHandle> {
    let identity = home.load_identity()?;
    let config = home.load_config()?;
    let token = home.load_token()?;
    let store = Arc::new(Store::open(&home.db_path())?);

    let (tx_out, rx_out) = mpsc::channel(128);
    let (notify_thread, _) = broadcast::channel(64);
    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    let port = port_override.unwrap_or(config.listen_port);
    let listener = TcpListener::bind(("127.0.0.1", port)).await?;
    let addr = listener.local_addr()?;

    let _ = fs::write(home.pid_path(), std::process::id().to_string());

    let state = AppState {
        identity: identity.clone(),
        config: config.clone(),
        token: token.clone(),
        store: Arc::clone(&store),
        tx_out,
        sessions: Arc::new(RwLock::new(HashMap::new())),
        notify_thread: notify_thread.clone(),
    };

    // Spawn relay WebSocket loop
    let relay_url = config.relay_url.clone();
    let mut relay_shutdown = shutdown_rx.clone();
    let tx_out_bg = state.tx_out.clone();
    tokio::spawn(async move {
        run_relay_loop(
            relay_url,
            identity,
            config,
            store,
            tx_out_bg,
            rx_out,
            notify_thread,
            relay_shutdown.clone(),
        )
        .await;
        let _ = relay_shutdown.changed().await;
    });

    // Spawn localhost HTTP API
    let router = build_router(state);
    let mut http_shutdown = shutdown_rx.clone();
    tokio::spawn(async move {
        let _ = axum::serve(listener, router)
            .with_graceful_shutdown(async move {
                while !*http_shutdown.borrow() {
                    if http_shutdown.changed().await.is_err() {
                        break;
                    }
                }
            })
            .await;
    });

    info!("agentlink daemon listening on http://{addr}");
    Ok(DaemonHandle {
        addr,
        token,
        shutdown: shutdown_tx,
    })
}
