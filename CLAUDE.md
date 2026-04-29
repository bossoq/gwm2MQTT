# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Commands

```bash
# Build
cargo build
cargo build --release

# Check (faster than build, no binary)
cargo check

# Run (requires env vars — see Configuration below)
cargo run

# Lint
cargo clippy

# Format
cargo fmt
```

There are no automated tests in this project.

## Configuration

The binary reads configuration from environment variables at **compile time** via `option_env!()`, not runtime env vars. This means configuration values must be set at build time:

```bash
EMAIL=user@example.com PASSWORD=pass PIN=1234 \
MQTT_BROKER=localhost MQTT_PORT=1883 \
cargo build --release
```

Alternatively, config files under `/opt/gwm2mqtt/` are read at runtime as fallback:
- `/opt/gwm2mqtt/mqtt.json` — MQTT broker settings
- `/opt/gwm2mqtt/configuration.json` — cached vehicle info (`VehicleInfo`)
- `/opt/gwm2mqtt/state.json` — persisted vehicle state (`VehicleStatus`)
- `/opt/gwm2mqtt/logs/` — rolling log files

Environment variables (all compile-time via `option_env!`):
- `EMAIL`, `PASSWORD`, `PIN` — GWM Cloud credentials (PIN is MD5-hashed for remote commands)
- `MQTT_BROKER`, `MQTT_PORT`, `MQTT_USER`, `MQTT_PASSWORD` — MQTT connection
- `MQTT_DISCOVERY_TOPIC` (default: `homeassistant`), `MQTT_TOPIC_PREFIX` (default: `gwm2mqtt`)
- `VEHICLE_VIN` — optional; selects vehicle by VIN prefix (first 3 chars) and suffix (last 4 chars)
- `REFRESH_INTERVAL` — polling interval in seconds (default: 10)

## Architecture

This is a Rust async bridge that polls a GWM Cloud API and publishes vehicle telemetry to an MQTT broker for Home Assistant auto-discovery.

### Data flow

1. **Startup** (`lib.rs::run`): Logs in via GWM API → validates token → resolves vehicle → publishes MQTT discovery payloads → subscribes to command topics → enters polling loop.
2. **Polling loop**: Calls `get_vehicle_status()` every `REFRESH_INTERVAL` seconds → merges local-only fields (`ac_time`, `ac_temp`, `petmode`) back onto the API response → persists to `state.json` → publishes to MQTT.
3. **Command handling** (`handle_event`): MQTT `Publish` events on `{prefix}/{entity}/set` topics are matched by regex → dispatched to `send_lock_command` or `send_climate_command` → command confirmation polled via `get_remote_cmd_status` (up to 10 × 5s retries).

### Debounce pattern

`DEBOUNCE_LOCK` and `DEBOUNCE_AC` are global `Mutex<String>` sentinels. When a command is sent, the sentinel is set to the expected next state (`"lock"`, `"unlock"`, `"on"`, `"off"`). `publish_vehicle_status` overrides the API-reported state with the debounced value until the API confirms the change, preventing UI flicker.

`ac_time`, `ac_temp`, and `petmode` are **local-only** — they are never returned by the GWM API. Every time vehicle status is fetched, these three fields must be explicitly copied from the previous `VEHICLE_STATUS` back onto the new struct (see `lib.rs:551-553`).

### Module responsibilities

- `src/gwm.rs` — GWM Cloud API client: auth (login, token refresh), vehicle list, vehicle status polling, remote commands (lock `0x05`, climate `0x04`). Request signing uses HMAC-SHA256 over sorted query params / JSON body.
- `src/mqtt.rs` — MQTT client setup, Home Assistant discovery payload construction, state publishing. Topic scheme: `{prefix}/{brand}_{last4vin}/{entity}`.
- `src/lib.rs` — Orchestration: global state (`LazyLock<Mutex<_>>`), main event loop, command dispatch, debounce logic.
- `src/logging.rs` — log4rs setup: debug-level file logging with rolling (10 MB / 10 files), info-level stderr.

### API target

The GWM Cloud base URL is hardcoded to the Asia-Pacific endpoint (`ap-h5-gateway.gwmcloud.com`) with Thailand region headers (`country: TH`, `language: th`). Changing region requires updating `STD_HEADER` constants in `gwm.rs`.
