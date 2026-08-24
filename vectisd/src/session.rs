use std::path::Path;
use zbus::zvariant::OwnedObjectPath;
use zbus::Connection;

#[derive(Debug, Clone)]
pub struct SessionInfo {
    pub id: String,
    pub uid: u32,
    pub class: String,
    pub leader_pid: u32,
}

fn pid_alive(pid: u32) -> bool {
    pid != 0 && Path::new(&format!("/proc/{pid}")).exists()
}

pub async fn list_sessions(connection: &Connection) -> zbus::Result<Vec<SessionInfo>> {
    let manager = zbus::Proxy::new(
        connection,
        "org.freedesktop.login1",
        "/org/freedesktop/login1",
        "org.freedesktop.login1.Manager",
    )
    .await?;

    let raw: Vec<(String, u32, String, String, OwnedObjectPath)> =
        manager.call("ListSessions", &()).await?;

    let mut out = Vec::with_capacity(raw.len());
    for (id, uid, _user, _seat, path) in raw {
        let session_proxy =
            zbus::Proxy::new(connection, "org.freedesktop.login1", path, "org.freedesktop.login1.Session").await?;
        let class: String = session_proxy.get_property("Class").await.unwrap_or_default();
        let leader_pid: u32 = session_proxy.get_property("Leader").await.unwrap_or(0);
        out.push(SessionInfo { id, uid, class, leader_pid });
    }
    Ok(out)
}

/// True if no user session (Class=user, uid>=1000, leader alive) exists.
/// Fails closed (false) on logind errors.
pub async fn no_active_user_sessions(connection: &Connection) -> bool {
    match list_sessions(connection).await {
        Ok(sessions) => {
            let blocker = sessions.iter().find(|s| s.class == "user" && s.uid >= 1000 && pid_alive(s.leader_pid));
            match blocker {
                Some(s) => {
                    tracing::debug!(
                        "no_active_user_sessions: blocked by session id={} uid={} leader_pid={} (alive)",
                        s.id, s.uid, s.leader_pid
                    );
                    false
                }
                None => {
                    tracing::debug!("no_active_user_sessions: no live user session, {} total", sessions.len());
                    true
                }
            }
        }
        Err(e) => {
            tracing::warn!("no_active_user_sessions: logind query failed ({e}), failing closed");
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pid_alive_true_for_pid_1() {
        assert!(pid_alive(1));
    }

    #[test]
    fn pid_alive_false_for_implausible_pid() {
        assert!(!pid_alive(999_999_999));
    }

    #[test]
    fn pid_alive_false_for_zero() {
        assert!(!pid_alive(0));
    }
}
