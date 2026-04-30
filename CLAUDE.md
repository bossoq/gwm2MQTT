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

Tests exist for `web.rs` and `gwm.rs`. Run with `cargo test`.

## Configuration

The binary reads configuration from environment variables at **compile time** via `option_env!()`, not runtime env vars. This means configuration values must be set at build time:

```bash
EMAIL=user@example.com PASSWORD=pass PIN=1234 \
MQTT_BROKER=localhost MQTT_PORT=1883 \
cargo build --release
```

Alternatively, config files under `/opt/gwm2mqtt/` are read at runtime as fallback:
- `/opt/gwm2mqtt/mqtt.json` — MQTT broker settings
- `/opt/gwm2mqtt/credentials.json` — encrypted GWM credentials (AES-256-GCM): `email`, `password`, `pin`, `vehicleVin`, `refreshInterval`
- `/opt/gwm2mqtt/baseurl.json` — **plain JSON** `{"baseUrl": "https://..."}` — not encrypted; required field
- `/opt/gwm2mqtt/configuration.json` — cached vehicle info (`VehicleInfo`)
- `/opt/gwm2mqtt/state_{VIN}.json` — persisted vehicle state (`VehicleStatus`), one file per vehicle
- `/opt/gwm2mqtt/logs/` — rolling log files

Runtime-configurable settings (saved via web dashboard):
- `baseUrl` — GWM API base URL (**required**; stored in plain `baseurl.json`, not encrypted); default: `https://example.api.com/`; auto-normalised (adds `https://` prefix and trailing `/` if missing); cached in memory after first read, updated immediately when saved
- `vehicleVin` — VIN filter (optional; stored in `credentials.json`)
- `refreshInterval` — polling interval in seconds (stored in `credentials.json`)

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
- `src/web.rs` — Axum web dashboard (`/` and `/settings`); REST API for status, config read/write, and direct vehicle commands.
- `src/crypto.rs` — AES-256-GCM encryption for `credentials.json`; machine-id derived key with `/opt/gwm2mqtt/.secret` fallback.
- `src/logging.rs` — log4rs setup: debug-level file logging with rolling (10 MB / 10 files), info-level stderr.

### Web dashboard

Served on the port passed to `main` (default `3000`). Static files are embedded at compile time via `include_str!`:
- `static/index.html` — live vehicle dashboard; polls `/api/status` every 5 s; AC settings panel toggle via ⚙️ button next to the AC command button.
- `static/settings.html` — configuration page for GWM credentials, MQTT broker, and API base URL.
- `static/style.css`, `static/app.js` — shared styles and utilities.

### API target

The GWM API base URL is runtime-configurable via the settings page. It is stored as plain JSON in `/opt/gwm2mqtt/baseurl.json` (not encrypted). `get_base_url()` in `gwm.rs` reads from an in-memory `RwLock` cache on every call; the cache is populated on first access from the file and updated via `set_base_url_cache()` whenever the settings page saves a new value. `DEFAULT_BASEURL` is `"https://example.api.com/"`. The field is **required** — the server validates scheme (`http`/`https`) and presence of a host before accepting a save. If the file is missing or the URL is invalid, API calls return `Err` (logged) rather than panicking, so the web service stays available for reconfiguration. Region-specific headers (`country: TH`, `language: th`, etc.) are in the `STD_HEADER` static in `gwm.rs` and must be updated there for other regions.
