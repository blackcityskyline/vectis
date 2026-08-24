# vectis

D-Bus daemon + CLI for Linux laptop power management: power profiles (TLP/ppd), hard TDP caps (RAPL), and GPU switching (nvidia/AMD Optimus).

## Build

```bash
cargo build --release --workspace
```

## Test

```bash
cargo test --workspace
```

39 tests covering GPU mode-switching (bbswitch settling, runtime PM fallback, nvidia/amd topologies), RAPL powercap, TLP profile mapping, state persistence, and gpu_lock concurrency.

## Install

```bash
# Binaries
sudo install -Dm755 target/release/vectisd /usr/local/bin/vectisd
sudo install -Dm755 target/release/vectis  /usr/local/bin/vectis

# D-Bus policy (required — daemon won't start without it)
sudo install -Dm644 systemd/org.vectis.Daemon1.conf \
  /usr/share/dbus-1/system.d/org.vectis.Daemon1.conf

# PolicyKit actions (passwordless from active graphical session)
sudo install -Dm644 policy/org.vectis.policy \
  /usr/share/polkit-1/actions/org.vectis.policy

# systemd unit
sudo install -Dm644 systemd/vectisd.service \
  /etc/systemd/system/vectisd.service
sudo systemctl daemon-reload
sudo systemctl enable --now vectisd

# Verify
vectis status
```

If `vectis status` fails with D-Bus error — check `journalctl -u vectisd -e`, usually the D-Bus policy file (step 2) was skipped.

## Usage

```bash
# Power profiles
vectis profile balanced
vectis profile powersave
vectis profile performance

# TDP (RAPL)
vectis tdp --pl1 15 --pl2 25
vectis tdp --pl1 15 --hard-lock    # PL2 = PL1, kills boost
vectis tdp-clear

# GPU switching
vectis gpu hybrid                  # queued (applies on logout/boot)
vectis gpu hybrid --force          # immediate (kills session)
vectis gpu integrated              # power off discrete GPU
vectis gpu hybrid -n               # force nouveau

vectis gpu-list                    # PCI display devices
vectis status                      # full status
vectis status --json               # machine-readable
```

## Config

`/etc/vectis/config.toml` (all optional):

```toml
[power_backend]
# "auto" | "tlp" | "ppd" — default: auto

[tdp]
enabled = false
# pl1_watts, pl2_watts, tau_us, hard_lock — applied once at startup

[gpu]
backend = "auto"  # "auto" | "bbswitch" | "acpi_call" | "runtime_pm"
acpi_call_off = "\\_SB.PCI0.PEG0.PEGP._OFF"
acpi_call_on  = "\\_SB.PCI0.PEG0.PEGP._ON"

[conflicts]
watch_services = ["supergfxd.service", "optimus-manager.service"]
```

## Components

```
vectis-common/    Shared types, D-Bus contract, config
vectisd/          Daemon (backends: gpu, rapl, tlp, ppd)
vectis-cli/       CLI client
systemd/          Service unit + D-Bus policy
policy/           PolicyKit actions
```

## How it works

- **Profiles** — delegates to TLP (`tlp <profile>`) or power-profiles-daemon (ActiveProfile D-Bus property). No config rewriting.
- **TDP** — writes directly to `/sys/class/powercap/intel-rapl:0`. Independent of TLP/ppd.
- **GPU** — priority fallback: runtime PM (RTD3) → bbswitch → acpi_call. Queued switches apply on `SessionRemoved` signal or next boot. `gpu_lock` serializes foreground/background GPU paths to prevent race conditions.
