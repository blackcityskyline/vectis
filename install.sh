#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")" && pwd)"
BIN_DIR="/usr/local/bin"
DBUS_DIR="/usr/share/dbus-1/system.d"
POLKIT_DIR="/usr/share/polkit-1/actions"
SYSTEMD_DIR="/etc/systemd/system"

info()  { printf '\033[1;34m::\033[0m %s\n' "$*"; }
ok()    { printf '\033[1;32m::\033[0m %s\n' "$*"; }
error() { printf '\033[1;31m::\033[0m %s\n' "$*" >&2; exit 1; }

[ "$(id -u)" -eq 0 ] || error "Run as root (sudo)."

# ── Binaries ──────────────────────────────────────────────
for bin in vectisd vectis; do
    src="$REPO_ROOT/target/release/$bin"
    [ -f "$src" ] || error "Binary not found: $src\n       Run: cargo build --release --workspace"
    info "Installing $bin → $BIN_DIR/$bin"
    install -Dm755 "$src" "$BIN_DIR/$bin"
done

# ── D-Bus policy ──────────────────────────────────────────
info "Installing D-Bus policy → $DBUS_DIR/org.vectis.Daemon1.conf"
install -Dm644 "$REPO_ROOT/systemd/org.vectis.Daemon1.conf" \
    "$DBUS_DIR/org.vectis.Daemon1.conf"

# ── PolicyKit actions ─────────────────────────────────────
info "Installing PolicyKit policy → $POLKIT_DIR/org.vectis.policy"
install -Dm644 "$REPO_ROOT/policy/org.vectis.policy" \
    "$POLKIT_DIR/org.vectis.policy"

# ── systemd unit ──────────────────────────────────────────
info "Installing systemd unit → $SYSTEMD_DIR/vectisd.service"
install -Dm644 "$REPO_ROOT/systemd/vectisd.service" \
    "$SYSTEMD_DIR/vectisd.service"

# ── Migrate from powerctld (old name) ─────────────────────
for old_name in powerctld; do
    if systemctl is-active --quiet "$old_name" 2>/dev/null || \
       [ -f "$SYSTEMD_DIR/$old_name.service" ]; then
        info "Stopping old service: $old_name"
        systemctl stop "$old_name" 2>/dev/null || true
        systemctl disable "$old_name" 2>/dev/null || true
        rm -f "$SYSTEMD_DIR/$old_name.service" \
              "$SYSTEMD_DIR/$old_name.service.d/override.conf" \
              "/usr/share/dbus-1/system.d/org.powerctl.Daemon1.conf" \
              "/usr/local/bin/$old_name"
        info "Removed $old_name"
    fi
done

# ── Activate ──────────────────────────────────────────────
info "Reloading systemd daemon"
systemctl daemon-reload

info "Enabling and starting vectisd"
systemctl enable --now vectisd

# ── Verify ────────────────────────────────────────────────
if systemctl is-active --quiet vectisd; then
    ok "vectisd is running"
else
    error "vectisd failed to start. Check: journalctl -u vectisd -e"
fi

echo
ok "Installation complete. Try: vectis status"
