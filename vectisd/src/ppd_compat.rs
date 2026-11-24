//! power-profiles-daemon compatibility.
//!
//! Desktop shells talk to PPD, not to TLP. When no real PPD/tuned-ppd is
//! running, vectisd claims PPD's D-Bus identity (both the current
//! `org.freedesktop.UPower.PowerProfiles` and the pre-0.20 legacy
//! `net.hadess.PowerProfiles`) and routes `ActiveProfile` through the normal
//! profile backend. A real PPD always wins: if one is running, or is merely
//! enabled and due to start later in boot, nothing is registered and the
//! existing PPD-delegating backend keeps working.

use crate::service::DaemonService;
use std::collections::HashMap;
use std::sync::Arc;
use vectis_common::dbus_api::{
    PPD_LEGACY_BUS_NAME, PPD_LEGACY_PATH, PPD_MODERN_BUS_NAME, PPD_MODERN_PATH,
};
use vectis_common::types::Profile;
use zbus::fdo::{RequestNameFlags, RequestNameReply};
use zbus::zvariant::{OwnedValue, Value};
use zbus::Connection;

pub struct PpdModern(pub Arc<DaemonService>);
pub struct PpdLegacy(pub Arc<DaemonService>);

/// Known PPD API implementations. If one of these is installed in a state
/// that means it will serve the bus, vectis must not race it at boot.
const PPD_UNITS: [&str; 3] = [
    "power-profiles-daemon.service",
    "tuned-ppd.service",
    "tlp-pd.service",
];

/// Whether a unit in this state is a contender for the bus: it is serving now,
/// or it is enabled and will start. A disabled, masked or failed unit is not:
/// then nobody else serves the API and vectis takes over.
fn is_contender(file_state: &str, active_state: &str) -> bool {
    let served_now = matches!(active_state, "active" | "activating" | "reloading");
    let will_start = matches!(file_state, "enabled" | "enabled-runtime") && active_state != "failed";
    served_now || will_start
}

/// Another PPD implementation that will serve the bus, if any. Checks units
/// rather than the bus name only, because at boot vectisd (multi-user) starts
/// before tlp-pd (graphical) and would otherwise win the name by pure timing.
async fn contender(connection: &Connection) -> Option<&'static str> {
    let manager = zbus::Proxy::new(
        connection,
        "org.freedesktop.systemd1",
        "/org/freedesktop/systemd1",
        "org.freedesktop.systemd1.Manager",
    )
    .await
    .ok()?;

    for unit in PPD_UNITS {
        // GetUnitFileState fails for units that are not installed at all.
        let Ok(file_state) = manager.call::<_, _, String>("GetUnitFileState", &(unit,)).await else {
            continue;
        };
        let mut runtime_state = String::new();
        if let Ok(path) = manager
            .call::<_, _, zbus::zvariant::OwnedObjectPath>("GetUnit", &(unit,))
            .await
        {
            if let Ok(proxy) = zbus::Proxy::new(
                connection,
                "org.freedesktop.systemd1",
                path,
                "org.freedesktop.systemd1.Unit",
            )
            .await
            {
                runtime_state = proxy.get_property::<String>("ActiveState").await.unwrap_or_default();
            }
        }

        if is_contender(&file_state, &runtime_state) {
            return Some(unit);
        }
    }

    None
}

/// Claims PPD's names if they are free. Returns whether the modern identity
/// (the one current shells use) is now served by vectis.
pub async fn register(service: Arc<DaemonService>, connection: &Connection) -> bool {
    if !service.ppd_compat_enabled() {
        tracing::info!("ppd-compat: disabled by config");
        return false;
    }

    if let Some(unit) = contender(connection).await {
        tracing::info!("ppd-compat: {unit} is the bus contender, delegating instead");
        return false;
    }

    // ponytail: the contender check runs once, at startup. Whoever claims the
    // name keeps it, so a PPD unit started later by hand fails on the bus name
    // until vectisd restarts. Add a runtime handoff if that bites.

    if let Err(e) = connection
        .object_server()
        .at(PPD_MODERN_PATH, PpdModern(service.clone()))
        .await
    {
        tracing::warn!("ppd-compat: failed to register {PPD_MODERN_PATH}: {e}");
        return false;
    }
    if !own(connection, PPD_MODERN_BUS_NAME).await {
        return false;
    }

    service.mark_ppd_compat_active();

    // Legacy alias is optional: old clients use it, nothing depends on it.
    match connection
        .object_server()
        .at(PPD_LEGACY_PATH, PpdLegacy(service))
        .await
    {
        Ok(_) => {
            if own(connection, PPD_LEGACY_BUS_NAME).await {
                tracing::info!("ppd-compat: serving {PPD_MODERN_BUS_NAME} and {PPD_LEGACY_BUS_NAME}");
            } else {
                tracing::info!("ppd-compat: serving {PPD_MODERN_BUS_NAME} (legacy name occupied)");
            }
        }
        Err(e) => tracing::warn!("ppd-compat: failed to register {PPD_LEGACY_PATH}: {e}"),
    }

    true
}

async fn own(connection: &Connection, name: &str) -> bool {
    match connection
        .request_name_with_flags(name, RequestNameFlags::DoNotQueue.into())
        .await
    {
        Ok(RequestNameReply::PrimaryOwner) => true,
        Ok(reply) => {
            tracing::info!("ppd-compat: {name} not acquired ({reply:?})");
            false
        }
        Err(e) => {
            tracing::info!("ppd-compat: {name} already owned, delegating instead ({e})");
            false
        }
    }
}

pub async fn emit_profile_changed(connection: &Connection, profile: Profile) {
    for (path, iface) in [
        (PPD_MODERN_PATH, PPD_MODERN_BUS_NAME),
        (PPD_LEGACY_PATH, PPD_LEGACY_BUS_NAME),
    ] {
        let body = (
            iface,
            HashMap::from([("ActiveProfile", OwnedValue::from(Value::from(profile.ppd_name())))]),
            Vec::<&str>::new(),
        );
        if let Err(e) = connection
            .emit_signal(
                None::<&str>,
                path,
                "org.freedesktop.DBus.Properties",
                "PropertiesChanged",
                &body,
            )
            .await
        {
            tracing::warn!("ppd-compat: PropertiesChanged emission failed: {e}");
        }
    }
}

fn profiles_variant() -> Vec<HashMap<String, OwnedValue>> {
    [Profile::Powersave, Profile::Balanced, Profile::Performance]
        .into_iter()
        .map(|p| {
            HashMap::from([
                ("Profile".to_string(), OwnedValue::from(Value::from(p.ppd_name()))),
                ("Driver".to_string(), OwnedValue::from(Value::from("vectis"))),
            ])
        })
        .collect()
}

async fn apply_profile_value(service: &DaemonService, profile: &str) -> zbus::fdo::Result<()> {
    let parsed = Profile::from_ppd_name(profile)
        .ok_or_else(|| zbus::fdo::Error::InvalidArgs(format!("invalid profile '{profile}'")))?;
    service.apply_profile(&parsed.to_string()).await
}

macro_rules! ppd_interface {
    ($ty:ident, $iface:literal) => {
        #[zbus::dbus_interface(name = $iface)]
        impl $ty {
            #[dbus_interface(property)]
            async fn active_profile(&self) -> String {
                self.0.last_profile().unwrap_or(Profile::Balanced).ppd_name().to_string()
            }

            // zbus 3 property setters receive no message header, so the sender
            // is not available for polkit here. Profile switching is not a
            // privilege boundary (value-only, same reach as PPD's allow_active
            // policy); the vectis CLI path stays authenticated.
            #[dbus_interface(property)]
            async fn set_active_profile(&self, profile: String) -> zbus::Result<()> {
                apply_profile_value(&self.0, &profile).await.map_err(Into::into)
            }

            #[dbus_interface(property)]
            async fn profiles(&self) -> Vec<HashMap<String, OwnedValue>> {
                profiles_variant()
            }

            #[dbus_interface(property)]
            async fn performance_degraded(&self) -> String {
                String::new()
            }

            /// Deprecated since PPD 0.9, kept so GetAll stays complete.
            #[dbus_interface(property)]
            async fn performance_inhibited(&self) -> String {
                String::new()
            }

            #[dbus_interface(property)]
            async fn active_profile_holds(&self) -> Vec<HashMap<String, OwnedValue>> {
                Vec::new()
            }

            #[dbus_interface(property)]
            async fn actions(&self) -> Vec<String> {
                Vec::new()
            }

            #[dbus_interface(property)]
            async fn actions_info(&self) -> Vec<HashMap<String, OwnedValue>> {
                Vec::new()
            }

            #[dbus_interface(property)]
            async fn version(&self) -> String {
                env!("CARGO_PKG_VERSION").to_string()
            }

            #[dbus_interface(property)]
            async fn battery_aware(&self) -> bool {
                true
            }

            // ponytail: profile holds are read-only here - shells only set
            // ActiveProfile; add a hold table when `powerprofilesctl launch`
            // needs the real revert-on-exit semantics.
            async fn hold_profile(
                &self,
                profile: &str,
                reason: &str,
                application_id: &str,
            ) -> zbus::fdo::Result<u32> {
                let _ = (profile, reason, application_id);
                Err(zbus::fdo::Error::NotSupported(
                    "profile holds are not implemented by vectis".into(),
                ))
            }

            async fn release_profile(&self, cookie: u32) -> zbus::fdo::Result<()> {
                let _ = cookie;
                Err(zbus::fdo::Error::NotSupported(
                    "profile holds are not implemented by vectis".into(),
                ))
            }

            async fn set_action_enabled(&self, name: &str, enabled: bool) -> zbus::fdo::Result<()> {
                let _ = (name, enabled);
                Err(zbus::fdo::Error::NotSupported(
                    "PPD actions are not implemented by vectis".into(),
                ))
            }
        }
    };
}

// Attribute macros need string literals; keep these in sync with
// PPD_MODERN_BUS_NAME / PPD_LEGACY_BUS_NAME in vectis-common::dbus_api.
ppd_interface!(PpdModern, "org.freedesktop.UPower.PowerProfiles");
ppd_interface!(PpdLegacy, "net.hadess.PowerProfiles");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contender_rules() {
        // Serving now, or enabled and due to start later in boot: defer.
        assert!(is_contender("enabled", "active"));
        assert!(is_contender("enabled", "activating"));
        assert!(is_contender("enabled", "inactive"));
        // Started without being enabled (D-Bus activation): still serving.
        assert!(is_contender("disabled", "active"));
        // Gave up, or deliberately turned off: the bus is ours.
        assert!(!is_contender("enabled", "failed"));
        assert!(!is_contender("disabled", "inactive"));
        assert!(!is_contender("masked", "inactive"));
    }

    fn profile_name(entry: &HashMap<String, OwnedValue>) -> String {
        String::try_from(entry.get("Profile").expect("Profile key").clone()).expect("string value")
    }

    #[test]
    fn profiles_are_listed_in_ppd_order_with_a_driver() {
        let profiles = profiles_variant();
        let names: Vec<String> = profiles.iter().map(profile_name).collect();
        assert_eq!(names, ["power-saver", "balanced", "performance"]);
        assert!(
            profiles.iter().all(|p| p.contains_key("Driver")),
            "every entry needs Driver for old clients"
        );
    }

    #[test]
    fn ppd_slugs_roundtrip_and_reject_unknown_names() {
        for profile in [Profile::Powersave, Profile::Balanced, Profile::Performance] {
            assert_eq!(Profile::from_ppd_name(profile.ppd_name()), Some(profile));
        }
        assert_eq!(Profile::from_ppd_name("powersave"), None);
        assert_eq!(Profile::from_ppd_name(""), None);
    }
}
