# gwm2MQTT

A Rust service that bridges GWM Cloud vehicle telemetry to MQTT with Home Assistant auto-discovery support.

## Features

- Polls vehicle status from the GWM Cloud API and publishes it to MQTT
- Home Assistant auto-discovery for sensors, binary sensors, switches, numbers, and device tracker
- Remote commands: lock/unlock and climate control (requires PIN)
- Pet mode: keeps AC cycling automatically
- Persists AC timer, temperature, and pet mode state locally across restarts

## Configuration

Configuration is baked in at **compile time** via environment variables using `option_env!`. Set these before building:

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

If `MQTT_BROKER` and `MQTT_PORT` are not set at build time, the service falls back to reading `/opt/gwm2mqtt/mqtt.json` at runtime.

## Building

```bash
EMAIL=user@example.com PASSWORD=pass PIN=1234 \
MQTT_BROKER=mqtt.local MQTT_PORT=1883 \
cargo build --release
```

## Runtime Files

The service reads and writes files under `/opt/gwm2mqtt/`:

| Path | Description |
|---|---|
| `/opt/gwm2mqtt/mqtt.json` | MQTT config (written on startup if env vars are set) |
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
