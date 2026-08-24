use crate::service::DaemonService;
use futures_util::StreamExt;
use std::sync::Arc;
use std::time::Duration;
use zbus::Connection;

const POLL_INTERVAL: Duration = Duration::from_secs(20);

pub async fn run(connection: Connection, service: Arc<DaemonService>) {
    tracing::info!("session_watch: starting ({POLL_INTERVAL:?} poll + dbus SessionRemoved fast path)");

    // Poll safety net: re-checks every interval regardless of signals
    let poll_service = service.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(POLL_INTERVAL);
        loop {
            interval.tick().await;
            tracing::debug!("session_watch: poll tick");
            poll_service.apply_pending_gpu_switch_if_ready().await;
        }
    });

    // D-Bus fast path: react to logind SessionRemoved signal directly
    let manager = match zbus::Proxy::new(
        &connection,
        "org.freedesktop.login1",
        "/org/freedesktop/login1",
        "org.freedesktop.login1.Manager",
    )
    .await
    {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!("session_watch: logind connection failed ({e}), relying on poll only");
            return;
        }
    };

    let mut stream = match manager.receive_signal("SessionRemoved").await {
        Ok(s) => {
            tracing::info!("session_watch: subscribed to SessionRemoved");
            s
        }
        Err(e) => {
            tracing::warn!("session_watch: SessionRemoved subscribe failed ({e}), relying on poll only");
            return;
        }
    };

    while stream.next().await.is_some() {
        tracing::debug!("session_watch: SessionRemoved signal");
        service.apply_pending_gpu_switch_if_ready().await;
    }

    tracing::warn!("session_watch: signal stream ended, falling back to poll only");
}
