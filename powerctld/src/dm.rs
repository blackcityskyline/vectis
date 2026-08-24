use zbus::Connection;

const DM_CANDIDATES: &[&str] = &["greetd", "sddm", "lightdm", "gdm", "gdm3", "lxdm", "xdm"];

/// Finds active DM via display-manager.service alias, falls back to candidate list
pub async fn detect(connection: &Connection) -> Option<String> {
    let manager = zbus::Proxy::new(
        connection,
        "org.freedesktop.systemd1",
        "/org/freedesktop/systemd1",
        "org.freedesktop.systemd1.Manager",
    )
    .await
    .ok()?;

    if let Ok(unit_path) = manager.call::<_, _, zbus::zvariant::OwnedObjectPath>("GetUnit", &("display-manager.service",)).await {
        if let Ok(unit_proxy) = zbus::Proxy::new(
            connection,
            "org.freedesktop.systemd1",
            unit_path,
            "org.freedesktop.systemd1.Unit",
        )
        .await
        {
            if let Ok(id) = unit_proxy.get_property::<String>("Id").await {
                if id != "display-manager.service" {
                    return Some(id);
                }
            }
        }
    }

    for name in DM_CANDIDATES {
        let unit_name = format!("{name}.service");
        if let Ok(unit_path) = manager.call::<_, _, zbus::zvariant::OwnedObjectPath>("GetUnit", &(unit_name.as_str(),)).await {
            if let Ok(unit_proxy) = zbus::Proxy::new(
                connection,
                "org.freedesktop.systemd1",
                unit_path,
                "org.freedesktop.systemd1.Unit",
            )
            .await
            {
                if let Ok(state) = unit_proxy.get_property::<String>("ActiveState").await {
                    if state == "active" {
                        return Some(unit_name);
                    }
                }
            }
        }
    }

    None
}

pub async fn stop(connection: &Connection, unit: &str) -> zbus::Result<()> {
    let manager = zbus::Proxy::new(
        connection,
        "org.freedesktop.systemd1",
        "/org/freedesktop/systemd1",
        "org.freedesktop.systemd1.Manager",
    )
    .await?;
    let _job: zbus::zvariant::OwnedObjectPath = manager.call("StopUnit", &(unit, "replace")).await?;
    Ok(())
}

pub async fn start(connection: &Connection, unit: &str) -> zbus::Result<()> {
    let manager = zbus::Proxy::new(
        connection,
        "org.freedesktop.systemd1",
        "/org/freedesktop/systemd1",
        "org.freedesktop.systemd1.Manager",
    )
    .await?;
    let _job: zbus::zvariant::OwnedObjectPath = manager.call("StartUnit", &(unit, "replace")).await?;
    Ok(())
}
