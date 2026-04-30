# gwm2MQTT

A Rust service that bridges GWM Cloud vehicle telemetry to MQTT with Home Assistant auto-discovery support.

## Features

- Polls vehicle status from the GWM Cloud API and publishes it to MQTT
- Home Assistant auto-discovery for sensors, binary sensors, switches, numbers, and device tracker
- Remote commands: lock/unlock and climate control (requires PIN)
- Pet mode: keeps AC cycling automatically
- Persists AC timer, temperature, and pet mode state locally across restarts
- Web dashboard for live status, remote commands, and runtime configuration (default port 8080)
- Debian package via `cargo-deb` with systemd service and auto-restart

## Installation (Debian/Ubuntu)

Download the latest `.deb` from the [Releases](../../releases) page and install:

```bash
sudo dpkg -i gwm2mqtt_*.deb
```

The service is installed to `/usr/bin/gwm2mqtt` and managed by systemd. On first install, use the web dashboard at `http://<host>:8080/settings` to enter your GWM account and MQTT credentials — no rebuild required.

## Web Dashboard

The dashboard runs on port 8080 by default. Override with `-p`:

```
gwm2mqtt -p 9090
```

Or set `WEB_PORT` at build time for a different compile-time default.

| Page | URL | Description |
|---|---|---|
| Status | `/` | Live vehicle status and command buttons (lock, AC, pet mode) |
| Settings | `/settings` | GWM account credentials and MQTT broker config |

Settings are saved to `/opt/gwm2mqtt/credentials.json` and `/opt/gwm2mqtt/mqtt.json` and take effect on next restart.

## Configuration

Configuration can be set at **compile time** via environment variables or at **runtime** via the web dashboard (saved to `/opt/gwm2mqtt/`). Compile-time values take priority over file-based fallbacks.

| Variable | Required | Default | Description |
|---|---|---|---|
| `EMAIL` | Yes | — | GWM Cloud account email |
| `PASSWORD` | Yes | — | GWM Cloud account password |
| `PIN` | Yes (for commands) | — | Vehicle PIN for remote commands |
| `MQTT_BROKER` | Yes | — | MQTT broker hostname |
| `MQTT_PORT` | Yes | `1883` | MQTT broker port |
| `MQTT_USER` | No | — | MQTT username |
| `MQTT_PASSWORD` | No | — | MQTT password |
| `MQTT_DISCOVERY_TOPIC` | No | `homeassistant` | Home Assistant discovery prefix |
| `MQTT_TOPIC_PREFIX` | No | `gwm2mqtt` | MQTT topic prefix |
| `VEHICLE_VIN` | No | — | Select vehicle by VIN (matched on first 3 + last 4 characters) |
| `REFRESH_INTERVAL` | No | `10` | Polling interval in seconds |
| `WEB_PORT` | No | `8080` | Web dashboard port |

## Building

```bash
# Check only (no binary)
cargo check

# Debug build
cargo build

# Release build with compile-time credentials
EMAIL=user@example.com PASSWORD=pass PIN=1234 \
MQTT_BROKER=mqtt.local MQTT_PORT=1883 \
cargo build --release

# Build Debian package (requires cargo-deb)
cargo install cargo-deb
cargo build --release
cargo deb --no-build
```

## Runtime Files

The service reads and writes files under `/opt/gwm2mqtt/`:

| Path | Description |
|---|---|
| `/opt/gwm2mqtt/credentials.json` | GWM account credentials (written by dashboard) |
| `/opt/gwm2mqtt/mqtt.json` | MQTT broker config (written by dashboard) |
| `/opt/gwm2mqtt/configuration.json` | Cached vehicle info |
| `/opt/gwm2mqtt/state.json` | Persisted vehicle state (ac_time, ac_temp, petmode) |
| `/opt/gwm2mqtt/logs/app.log` | Rolling log file (debug level, 10 MB × 10 files) |

## Home Assistant Entities

Once running, the following entities are auto-discovered per vehicle (`{prefix}/{brand}_{last4vin}`):

**Sensors:** Mileage, Charge (%), Range, Charge Time, Charge Status, Tire Pressure (FL/FR/RL/RR in psi), Tire Temperature (FL/FR/RL/RR)

**Binary Sensors:** Charging Port Plugged, AC Status, Air Filter Status, Lock Status, Door Open (FL/FR/RL/RR), Trunk Open, Window Open (FL/FR/RL/RR), Sunroof Open, Tire Pressure Alarms (×4), Tire Temp Alarms (×4), Headlight, Left/Right Turn Light

**Switches:** Lock Control, Climate Control, Pet Mode

**Numbers:** AC Timer (1–30 min), AC Temperature (17–31 °C)

**Device Tracker:** GPS position

## Notes

- The API endpoint is hardcoded to the Asia-Pacific GWM Cloud (`ap-h5-gateway.gwmcloud.com`) with Thailand region headers. Other regions are not currently supported.
- Remote commands (lock, climate) poll for confirmation up to 10 times with 5-second intervals.
- To create a release: add the `release` label to a PR before merging it to `main`. The CI workflow will build the binary and `.deb` and publish a GitHub Release automatically.
