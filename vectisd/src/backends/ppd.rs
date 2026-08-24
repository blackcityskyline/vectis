use vectis_common::error::{Error, Result};
use vectis_common::types::Profile;
use zbus::Connection;

const BUS_NAME: &str = "net.hadess.PowerProfiles";
const OBJECT_PATH: &str = "/net/hadess/PowerProfiles";
const INTERFACE: &str = "net.hadess.PowerProfiles";

fn profile_to_ppd_name(profile: Profile) -> &'static str {
    match profile {
        Profile::Powersave => "power-saver",
        Profile::Balanced => "balanced",
        Profile::Performance => "performance",
    }
}

pub async fn set_profile(connection: &Connection, profile: Profile) -> Result<()> {
    let props = zbus::Proxy::new(connection, BUS_NAME, OBJECT_PATH, "org.freedesktop.DBus.Properties")
        .await
        .map_err(|e| Error::Other(format!("could not reach power-profiles-daemon: {e}")))?;

    let ppd_name = profile_to_ppd_name(profile);

    props
        .call::<_, _, ()>("Set", &(INTERFACE, "ActiveProfile", zbus::zvariant::Value::from(ppd_name)))
        .await
        .map_err(|e| Error::Other(format!("power-profiles-daemon rejected profile '{ppd_name}': {e}")))
}

pub async fn get_active_profile(connection: &Connection) -> Option<Profile> {
    let props = zbus::Proxy::new(connection, BUS_NAME, OBJECT_PATH, "org.freedesktop.DBus.Properties").await.ok()?;
    let value: zbus::zvariant::OwnedValue = props.call("Get", &(INTERFACE, "ActiveProfile")).await.ok()?;
    let s: String = value.try_into().ok()?;
    match s.as_str() {
        "power-saver" => Some(Profile::Powersave),
        "balanced" => Some(Profile::Balanced),
        "performance" => Some(Profile::Performance),
        _ => None,
    }
}
