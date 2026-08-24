mod backends;
mod conflicts;
mod dm;
mod polkit;
mod proc_kill;
mod procutil;
mod service;
mod session;
mod session_watch;
mod state;

use vectis_common::config::{Config, DEFAULT_CONFIG_PATH};
use vectis_common::dbus_api::{BUS_NAME, OBJECT_PATH};
use service::{DaemonService, DaemonServiceIface};
use state::{StateStore, DEFAULT_STATE_DIR};
use std::path::Path;
use std::sync::Arc;
use zbus::Connection;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    let config = match Config::load(Path::new(DEFAULT_CONFIG_PATH)) {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("failed to load config, using defaults: {e}");
            Config::default()
        }
    };

    let state_store = StateStore::new(DEFAULT_STATE_DIR);

    let connection = match Connection::system().await {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("failed to connect to system bus: {e}");
            std::process::exit(1);
        }
    };

    let service = Arc::new(DaemonService::new(config, state_store, connection.clone()));

    // Startup reconciliation runs BEFORE display-manager.service can start.
    // Hard timeout prevents blocking the entire graphical login if something hangs.
    const STARTUP_RECONCILE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
    match tokio::time::timeout(STARTUP_RECONCILE_TIMEOUT, service.reconcile_at_startup()).await {
        Ok(()) => {}
        Err(_) => {
            tracing::error!(
                "reconcile_at_startup timed out after {STARTUP_RECONCILE_TIMEOUT:?} - starting DM anyway. GPU may be in unexpected state."
            );
        }
    }

    if let Err(e) = connection
        .object_server()
        .at(OBJECT_PATH, DaemonServiceIface(service.clone()))
        .await
    {
        tracing::error!("failed to register D-Bus object: {e}");
        std::process::exit(1);
    }

    if let Err(e) = connection.request_name(BUS_NAME).await {
        tracing::error!("failed to acquire bus name {BUS_NAME}: {e}");
        std::process::exit(1);
    }

    let _ = sd_notify::notify(true, &[sd_notify::NotifyState::Ready]);
    tracing::info!("vectisd ready, serving {BUS_NAME} at {OBJECT_PATH}");

    let watch_service = service.clone();
    tokio::spawn(session_watch::run(connection, watch_service));

    std::future::pending::<()>().await;
}
