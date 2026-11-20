use vectis_common::dbus_api::{PPD_LEGACY_BUS_NAME as BUS_NAME, PPD_LEGACY_PATH as OBJECT_PATH};
use vectis_common::error::{Error, Result};
use vectis_common::types::Profile;
use zbus::Connection;

pub async fn set_profile(connection: &Connection, profile: Profile) -> Result<()> {
    let props = zbus::Proxy::new(connection, BUS_NAME, OBJECT_PATH, "org.freedesktop.DBus.Properties")
        .await
        .map_err(|e| Error::Other(format!("could not reach power-profiles-daemon: {e}")))?;

    let ppd_name = profile.ppd_name();

    props
        .call::<_, _, ()>("Set", &(BUS_NAME, "ActiveProfile", zbus::zvariant::Value::from(ppd_name)))
        .await
        .map_err(|e| Error::Other(format!("power-profiles-daemon rejected profile '{ppd_name}': {e}")))
}

pub async fn get_active_profile(connection: &Connection) -> Option<Profile> {
    let props = zbus::Proxy::new(connection, BUS_NAME, OBJECT_PATH, "org.freedesktop.DBus.Properties").await.ok()?;
    let value: zbus::zvariant::OwnedValue = props.call("Get", &(BUS_NAME, "ActiveProfile")).await.ok()?;
    let s: String = value.try_into().ok()?;
    Profile::from_ppd_name(&s)
}
