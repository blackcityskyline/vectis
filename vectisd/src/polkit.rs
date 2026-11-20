use zbus::Connection;

/// Extracts the caller from a message header and checks `action_id` via polkit.
pub async fn authorize(
    connection: &Connection,
    hdr: &zbus::MessageHeader<'_>,
    action_id: &str,
) -> zbus::fdo::Result<()> {
    let sender = hdr
        .sender()
        .ok()
        .flatten()
        .map(|s| s.to_string())
        .ok_or_else(|| zbus::fdo::Error::AccessDenied("no caller identity on message".into()))?;

    if check_authorization(connection, &sender, action_id).await {
        Ok(())
    } else {
        Err(zbus::fdo::Error::AccessDenied(format!("not authorized for {action_id}")))
    }
}

/// Checks sender authorization for action_id via polkit.
/// Fails closed: errors = not authorized.
pub async fn check_authorization(connection: &Connection, sender: &str, action_id: &str) -> bool {
    let authority = match zbus::Proxy::new(
        connection,
        "org.freedesktop.PolicyKit1",
        "/org/freedesktop/PolicyKit1/Authority",
        "org.freedesktop.PolicyKit1.Authority",
    )
    .await
    {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!("polkit: failed to build Authority proxy: {e}");
            return false;
        }
    };

    let mut details = std::collections::HashMap::new();
    details.insert("name", zbus::zvariant::Value::from(sender));
    let subject = ("system-bus-name", details);

    let empty_details: std::collections::HashMap<&str, &str> = std::collections::HashMap::new();
    let flags: u32 = 0;
    let cancellation_id = "";

    tracing::debug!("polkit: checking action_id={action_id} sender={sender}");

    let result: zbus::Result<(bool, bool, std::collections::HashMap<String, String>)> = authority
        .call(
            "CheckAuthorization",
            &(subject, action_id, empty_details, flags, cancellation_id),
        )
        .await;

    match result {
        Ok((is_authorized, _is_challenge, _details)) => is_authorized,
        Err(e) => {
            tracing::warn!("polkit: CheckAuthorization failed: {e}");
            false
        }
    }
}
