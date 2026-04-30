# gwm2MQTT

A Rust service that bridges GWM Cloud vehicle telemetry to MQTT with Home Assistant auto-discovery support.

## Features

- Polls **all vehicles** on the account and publishes each to its own MQTT topic namespace
- Home Assistant auto-discovery for sensors, binary sensors, switches, numbers, and device tracker
- Remote commands: lock/unlock and climate control (requires PIN)
- Pet mode: keeps AC cycling automatically
- Persists AC timer, temperature, and pet mode state locally across restarts
- Web dashboard with **vehicle selector dropdown** for live status and remote commands (default port 8080)
- Service restart button in the dashboard — reconnects automatically after restart
- Credentials stored AES-256-GCM encrypted at rest, keyed to the host machine
- Debian package via `cargo-deb` with systemd service and auto-restart

## Installation (Debian/Ubuntu)

Download the latest `.deb` from the [Releases](../../releases) page and install:

```bash
sudo dpkg -i gwm2mqtt_*.deb
```

The service is installed to `/usr/bin/gwm2mqtt` and managed by systemd. On first install the web dashboard starts immediately at `http://<host>:8080/settings` even before credentials exist. Enter your GWM account and MQTT credentials there and save; the service retries login automatically every 30 seconds until it succeeds — no rebuild required.

## Web Dashboard

The dashboard runs on port 8080 by default. Override with `-p`:

```
gwm2mqtt -p 9090
```

Or set `WEB_PORT` at build time for a different compile-time default.

| Page | URL | Description |
|---|---|---|
| Status | `/` | Live vehicle status and command buttons (lock, AC, pet mode) |
| Settings | `/settings` | GWM account credentials, MQTT broker config, API base URL, and service restart |

The status page includes an AC settings panel (temperature and timer sliders) that can be toggled via the ⚙️ button next to the AC command button.

Settings are saved to `/opt/gwm2mqtt/credentials.json` (AES-256-GCM encrypted) and `/opt/gwm2mqtt/mqtt.json`. GWM credential changes are picked up automatically on the next login retry (within 30 s). MQTT broker changes require a service restart — use the **Restart Service** button at the bottom of the Settings page; the dashboard polls and reconnects automatically.

## Configuration

Configuration can be set at **compile time** via environment variables or at **runtime** via the web dashboard (saved to `/opt/gwm2mqtt/`). **Runtime file settings always take priority** — compile-time values are only used as a fallback when no config file exists.

### Precedence (highest → lowest)

| Source | How to set |
|---|---|
| Runtime files | Web dashboard → `/settings`, or edit files directly |
| Compile-time env vars | Set at `cargo build` time (see [Building](#building)) |
| Built-in defaults | `broker=localhost`, `port=1883`, `discoveryTopic=homeassistant`, etc. |

### Environment variables (compile-time fallback only)

| Variable | Default | Description |
|---|---|---|
| `EMAIL` | — | GWM Cloud account email |
| `PASSWORD` | — | GWM Cloud account password |
| `PIN` | — | Vehicle PIN for remote commands |
| `MQTT_BROKER` | `localhost` | MQTT broker hostname |
| `MQTT_PORT` | `1883` | MQTT broker port |
| `MQTT_USER` | — | MQTT username |
| `MQTT_PASSWORD` | — | MQTT password |
| `MQTT_DISCOVERY_TOPIC` | `homeassistant` | Home Assistant discovery prefix |
| `MQTT_TOPIC_PREFIX` | `gwm2mqtt` | MQTT topic prefix |
| `VEHICLE_VIN` | — | Restrict to a single vehicle by VIN (matched on first 3 + last 4 characters); all vehicles used when unset |
| `REFRESH_INTERVAL` | `10` | Polling interval in seconds |
| `WEB_PORT` | `8080` | Web dashboard port |

> **Note:** `EMAIL`, `PASSWORD`, `PIN`, `VEHICLE_VIN`, `REFRESH_INTERVAL`, and the GWM API base URL can also be set at runtime via the **Settings** page and are stored in `credentials.json`. Runtime values always take priority over compile-time env vars.

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
| `/opt/gwm2mqtt/credentials.json` | GWM account credentials and runtime settings (`baseUrl`, `vehicleVin`, `refreshInterval`) — AES-256-GCM encrypted, keyed to `/etc/machine-id` |
| `/opt/gwm2mqtt/mqtt.json` | MQTT broker config (written by dashboard) |
| `/opt/gwm2mqtt/configuration.json` | Cached vehicle info (array, one entry per vehicle) |
| `/opt/gwm2mqtt/state_{VIN}.json` | Persisted vehicle state per vehicle (`ac_time`, `ac_temp`, `petmode`); legacy `state.json` is read on first run for migration |
| `/opt/gwm2mqtt/logs/app.log` | Rolling log file (debug level, 10 MB × 10 files) |

## Home Assistant Entities

Once running, the following entities are auto-discovered per vehicle (`{prefix}/{brand}_{last4vin}`):

**Sensors:** Mileage, Charge (%), Range, Charge Time, Charge Status, Tire Pressure (FL/FR/RL/RR in psi), Tire Temperature (FL/FR/RL/RR)

**Binary Sensors:** Charging Port Plugged, AC Status, Air Filter Status, Lock Status, Door Open (FL/FR/RL/RR), Trunk Open, Window Open (FL/FR/RL/RR), Sunroof Open, Tire Pressure Alarms (×4), Tire Temp Alarms (×4), Headlight, Left/Right Turn Light

**Switches:** Lock Control, Climate Control, Pet Mode

**Numbers:** AC Timer (1–30 min), AC Temperature (17–31 °C)

**Device Tracker:** GPS position

## Notes

- The GWM API base URL defaults to `http://localhost:8080/` and can be changed at runtime via **Settings → GWM API Base URL**. The field auto-corrects missing `https://` prefix and trailing `/`. Region-specific headers (`country: TH`, `language: th`, etc.) are compiled in to `STD_HEADER` in `gwm.rs` and must be changed there for other regions.
- Remote commands (lock, climate) poll for confirmation up to 10 times with 5-second intervals.
- To create a release: add the `release` label to a PR before merging it to `main`. The CI workflow will build the binary and `.deb` and publish a GitHub Release automatically.
