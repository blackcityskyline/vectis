#!/usr/bin/env bash
set -euo pipefail

BIN_DIR="/usr/local/bin"
DBUS_DIR="/usr/share/dbus-1/system.d"
POLKIT_DIR="/usr/share/polkit-1/actions"
SYSTEMD_DIR="/etc/systemd/system"
CONFIG_DIR="/etc/vectis"

info()  { printf '\033[1;34m::\033[0m %s\n' "$*"; }
ok()    { printf '\033[1;32m::\033[0m %s\n' "$*"; }
warn()  { printf '\033[1;33m::\033[0m %s\n' "$*"; }

[ "$(id -u)" -eq 0 ] || { echo "Run as root (sudo)." >&2; exit 1; }

# ── Stop & disable all services ───────────────────────────
for svc in vectisd powerctld; do
    if systemctl is-active --quiet "$svc" 2>/dev/null; then
        info "Stopping $svc"
        systemctl stop "$svc"
    fi
    if systemctl is-enabled --quiet "$svc" 2>/dev/null; then
        info "Disabling $svc"
        systemctl disable "$svc"
    fi
done

# ── Remove files ──────────────────────────────────────────
for f in \
    "$BIN_DIR/vectisd" \
    "$BIN_DIR/vectis" \
    "$BIN_DIR/powerctld" \
    "$DBUS_DIR/org.vectis.Daemon1.conf" \
    "$DBUS_DIR/org.powerctl.Daemon1.conf" \
    "$POLKIT_DIR/org.vectis.policy" \
    "$SYSTEMD_DIR/vectisd.service" \
    "$SYSTEMD_DIR/powerctld.service"
do
    if [ -f "$f" ]; then
        info "Removing $f"
        rm -f "$f"
    fi
done

# Remove drop-in dirs if empty
rm -rf "$SYSTEMD_DIR/vectisd.service.d" \
       "$SYSTEMD_DIR/powerctld.service.d"

# ── Reload systemd ────────────────────────────────────────
info "Reloading systemd daemon"
systemctl daemon-reload

# ── Note about config ─────────────────────────────────────
if [ -d "$CONFIG_DIR" ]; then
    warn "Config directory $CONFIG_DIR was NOT removed (user data)."
    warn "Remove manually if no longer needed: sudo rm -rf $CONFIG_DIR"
fi

echo
ok "Uninstall complete."
