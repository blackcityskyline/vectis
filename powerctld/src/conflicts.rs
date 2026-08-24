use zbus::Connection;

/// Returns subset of watch_services currently active (via systemd D-Bus API, no systemctl subprocess)
pub async fn active_conflicts(connection: &Connection, watch_services: &[String]) -> Vec<String> {
    let mut active = Vec::new();

    let manager = match zbus::Proxy::new(
        connection,
        "org.freedesktop.systemd1",
        "/org/freedesktop/systemd1",
        "org.freedesktop.systemd1.Manager",
    )
    .await
    {
        Ok(p) => p,
        Err(_) => return active,
    };

    for name in watch_services {
        let unit_path: zbus::zvariant::OwnedObjectPath = match manager.call("GetUnit", &(name.as_str(),)).await {
            Ok(p) => p,
            Err(_) => continue,
        };

        let unit_proxy = match zbus::Proxy::new(
            connection,
            "org.freedesktop.systemd1",
            unit_path,
            "org.freedesktop.systemd1.Unit",
        )
        .await
        {
            Ok(p) => p,
            Err(_) => continue,
        };

        if let Ok(state) = unit_proxy.get_property::<String>("ActiveState").await {
            if state == "active" {
                active.push(name.clone());
            }
        }
    }

    active
}
