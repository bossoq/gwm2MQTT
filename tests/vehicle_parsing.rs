use gwm2mqtt_lib::gwm::{parse_vehicle_status, VehicleStatus};
use serde_json::json;

fn sample_data() -> serde_json::Value {
    json!({
        "items": [
            {"code": "2103010", "value": 12345},   // mileage
            {"code": "2013021", "value": 80},       // soc
            {"code": "2011007", "value": 320},      // range
            {"code": "2013022", "value": "45"},     // charge_time (string)
            {"code": "2041142", "value": "1"},      // charging_status: charging
            {"code": "2042082", "value": "1"},      // charging_port_plugged
            {"code": "2202001", "value": "1"},      // ac_status: on
            {"code": "2078020", "value": "0"},      // air_filter_status: off
            {"code": "2208001", "value": "1"},      // unlock_status: unlocked
            {"code": "2206004", "value": "0"},      // fl_door: closed
            {"code": "2206002", "value": "1"},      // fr_door: open
            {"code": "2206005", "value": "0"},      // rl_door: closed
            {"code": "2206003", "value": "0"},      // rr_door: closed
            {"code": "2206001", "value": "0"},      // trunk: closed
            {"code": "2210002", "value": "0"},      // fl_window: open  (0 = open)
            {"code": "2210001", "value": "1"},      // fr_window: closed (1 = closed)
            {"code": "2210004", "value": "1"},      // rl_window: closed
            {"code": "2210003", "value": "1"},      // rr_window: closed
            {"code": "2210005", "value": "6"},      // sunroof: open (6 = open)
            {"code": "2101001", "value": 220.0},    // fl_tire_pressure kPa
            {"code": "2101002", "value": 220.0},    // fr_tire_pressure kPa
            {"code": "2101003", "value": 215.0},    // rl_tire_pressure kPa
            {"code": "2101004", "value": 218.0},    // rr_tire_pressure kPa
            {"code": "2101005", "value": "35"},     // fl_tire_temp
            {"code": "2101006", "value": "36"},     // fr_tire_temp
            {"code": "2101007", "value": "34"},     // rl_tire_temp
            {"code": "2101008", "value": "35"},     // rr_tire_temp
            {"code": "2102001", "value": "0"},      // fl_tire_pressure_alarm: clear
            {"code": "2102002", "value": "0"},
            {"code": "2102003", "value": "1"},      // rl_tire_pressure_alarm: alarm
            {"code": "2102004", "value": "0"},
            {"code": "2102007", "value": "0"},      // fl_tire_temp_alarm
            {"code": "2102008", "value": "0"},
            {"code": "2102009", "value": "0"},
            {"code": "2102010", "value": "0"},
            {"code": "2204007", "value": "1"},      // head_light: on
            {"code": "2204009", "value": "0"},      // left_turn_light: off
            {"code": "2204010", "value": "0"},      // right_turn_light: off
        ],
        "longitude": 100.523,
        "latitude": 13.756,
        "updateTime": 1700000000000i64,
    })
}

#[test]
fn parses_vin() {
    let status = parse_vehicle_status("TESTVIN123456789", &sample_data());
    assert_eq!(status.vin, "TESTVIN123456789");
}

#[test]
fn parses_numeric_fields() {
    let status = parse_vehicle_status("VIN", &sample_data());
    assert_eq!(status.mileage, 12345);
    assert_eq!(status.soc, 80);
    assert_eq!(status.range, 320);
    assert_eq!(status.charge_time, 45);
}

#[test]
fn parses_boolean_fields() {
    let status = parse_vehicle_status("VIN", &sample_data());
    assert!(status.charging_status);
    assert!(status.charging_port_plugged);
    assert!(status.ac_status);
    assert!(!status.air_filter_status);
    assert!(status.unlock_status);
}

#[test]
fn parses_charging_status_description() {
    let status = parse_vehicle_status("VIN", &sample_data());
    assert_eq!(status.charging_status_desc, "Charging");
}

#[test]
fn charging_status_descriptions_all_values() {
    let cases = [
        ("0", "Not Charging"),
        ("1", "Charging"),
        ("2", "Awaiting Charge"),
        ("3", "Finish Charge"),
        ("6", "Error"),
    ];
    for (code, expected) in cases {
        let data = json!({
            "items": [{"code": "2041142", "value": code}],
            "longitude": 0.0, "latitude": 0.0, "updateTime": 0i64
        });
        let status = parse_vehicle_status("VIN", &data);
        assert_eq!(status.charging_status_desc, expected, "code={code}");
    }
}

#[test]
fn parses_door_states() {
    let status = parse_vehicle_status("VIN", &sample_data());
    assert!(!status.fl_door_open);
    assert!(status.fr_door_open);
    assert!(!status.rl_door_open);
    assert!(!status.rr_door_open);
    assert!(!status.trunk_open);
}

#[test]
fn window_open_logic_is_inverted() {
    // GWM API: value "0" = open, "1" = closed
    let status = parse_vehicle_status("VIN", &sample_data());
    assert!(status.fl_window_open); // value "0" → open
    assert!(!status.fr_window_open); // value "1" → closed
}

#[test]
fn sunroof_open_logic() {
    // GWM API: value "6" = open, "3" = closed
    let status = parse_vehicle_status("VIN", &sample_data());
    assert!(status.sunroof_open); // value "6" → open

    let closed_data = json!({
        "items": [{"code": "2210005", "value": "3"}],
        "longitude": 0.0, "latitude": 0.0, "updateTime": 0i64
    });
    let closed = parse_vehicle_status("VIN", &closed_data);
    assert!(!closed.sunroof_open);
}

#[test]
fn tire_pressure_converted_from_kpa_to_psi() {
    let status = parse_vehicle_status("VIN", &sample_data());
    // 220 kPa * 14.503773... = 3190.83... → rounded to nearest int → / 100 = 31.91
    assert_eq!(status.fl_tire_pressure, 31.91);
    assert_eq!(status.fr_tire_pressure, 31.91);
    // 215 kPa → 3118.31... → 31.18
    assert_eq!(status.rl_tire_pressure, 31.18);
    // 218 kPa → 3161.82... → 31.62
    assert_eq!(status.rr_tire_pressure, 31.62);
}

#[test]
fn tire_temperatures_parsed() {
    let status = parse_vehicle_status("VIN", &sample_data());
    assert_eq!(status.fl_tire_temp, 35);
    assert_eq!(status.fr_tire_temp, 36);
    assert_eq!(status.rl_tire_temp, 34);
    assert_eq!(status.rr_tire_temp, 35);
}

#[test]
fn tire_alarms_parsed() {
    let status = parse_vehicle_status("VIN", &sample_data());
    assert!(!status.fl_tire_pressure_alarm);
    assert!(!status.fr_tire_pressure_alarm);
    assert!(status.rl_tire_pressure_alarm);
    assert!(!status.rr_tire_pressure_alarm);
}

#[test]
fn lights_parsed() {
    let status = parse_vehicle_status("VIN", &sample_data());
    assert!(status.head_light);
    assert!(!status.left_turn_light);
    assert!(!status.right_turn_light);
}

#[test]
fn gps_coordinates_parsed() {
    let status = parse_vehicle_status("VIN", &sample_data());
    assert_eq!(status.longitude, 100.523);
    assert_eq!(status.latitude, 13.756);
}

#[test]
fn local_only_fields_use_defaults() {
    // ac_time, ac_temp, petmode come from local state, not the API
    let status = parse_vehicle_status("VIN", &sample_data());
    assert_eq!(status.ac_time, 15);
    assert_eq!(status.ac_temp, 26);
    assert!(!status.petmode);
}

#[test]
fn missing_items_use_defaults() {
    let data = json!({
        "items": [],
        "longitude": 0.0,
        "latitude": 0.0,
        "updateTime": 0i64
    });
    let status = parse_vehicle_status("VIN", &data);
    assert_eq!(status.mileage, 0);
    assert_eq!(status.soc, 0);
    assert!(!status.charging_status);
    assert!(!status.unlock_status);
    assert_eq!(status.fl_tire_pressure, 0.0);
}

#[test]
fn vehicle_status_default_values() {
    let s = VehicleStatus::default();
    assert_eq!(s.vin, "");
    assert_eq!(s.ac_time, 15);
    assert_eq!(s.ac_temp, 26);
    assert!(!s.petmode);
    assert_eq!(s.mileage, 0);
    assert_eq!(s.soc, 0);
}

#[test]
fn updated_at_parsed_from_update_time() {
    let data = json!({
        "items": [],
        "longitude": 0.0,
        "latitude": 0.0,
        "updateTime": 1700000000000i64  // 2023-11-14T22:13:20Z
    });
    let status = parse_vehicle_status("VIN", &data);
    assert_eq!(status.updated_at.timestamp(), 1700000000);
}

#[test]
fn unknown_charging_code_maps_to_unknown() {
    let data = json!({
        "items": [{"code": "2041142", "value": "99"}],
        "longitude": 0.0, "latitude": 0.0, "updateTime": 0i64
    });
    let status = parse_vehicle_status("VIN", &data);
    assert_eq!(status.charging_status_desc, "Unknown Mapping");
    assert!(!status.charging_status); // "99" != "1"
}

#[test]
fn all_windows_open_when_value_is_zero() {
    let data = json!({
        "items": [
            {"code": "2210002", "value": "0"},  // fl open
            {"code": "2210001", "value": "0"},  // fr open
            {"code": "2210004", "value": "0"},  // rl open
            {"code": "2210003", "value": "0"},  // rr open
            {"code": "2210005", "value": "6"},  // sunroof open
        ],
        "longitude": 0.0, "latitude": 0.0, "updateTime": 0i64
    });
    let s = parse_vehicle_status("VIN", &data);
    assert!(s.fl_window_open);
    assert!(s.fr_window_open);
    assert!(s.rl_window_open);
    assert!(s.rr_window_open);
    assert!(s.sunroof_open);
}

#[test]
fn all_windows_closed_when_value_is_one() {
    let data = json!({
        "items": [
            {"code": "2210002", "value": "1"},  // fl closed
            {"code": "2210001", "value": "1"},  // fr closed
            {"code": "2210004", "value": "1"},  // rl closed
            {"code": "2210003", "value": "1"},  // rr closed
            {"code": "2210005", "value": "3"},  // sunroof closed
        ],
        "longitude": 0.0, "latitude": 0.0, "updateTime": 0i64
    });
    let s = parse_vehicle_status("VIN", &data);
    assert!(!s.fl_window_open);
    assert!(!s.fr_window_open);
    assert!(!s.rl_window_open);
    assert!(!s.rr_window_open);
    assert!(!s.sunroof_open);
}

#[test]
fn tire_temp_alarms_all_four_positions() {
    let data = json!({
        "items": [
            {"code": "2102007", "value": "1"},  // fl
            {"code": "2102008", "value": "0"},  // fr
            {"code": "2102009", "value": "1"},  // rl
            {"code": "2102010", "value": "0"},  // rr
        ],
        "longitude": 0.0, "latitude": 0.0, "updateTime": 0i64
    });
    let s = parse_vehicle_status("VIN", &data);
    assert!(s.fl_tire_temp_alarm);
    assert!(!s.fr_tire_temp_alarm);
    assert!(s.rl_tire_temp_alarm);
    assert!(!s.rr_tire_temp_alarm);
}

#[test]
fn zero_kpa_tire_pressure_converts_to_zero_psi() {
    let data = json!({
        "items": [
            {"code": "2101001", "value": 0.0},
            {"code": "2101002", "value": 0.0},
            {"code": "2101003", "value": 0.0},
            {"code": "2101004", "value": 0.0},
        ],
        "longitude": 0.0, "latitude": 0.0, "updateTime": 0i64
    });
    let s = parse_vehicle_status("VIN", &data);
    assert_eq!(s.fl_tire_pressure, 0.0);
    assert_eq!(s.rr_tire_pressure, 0.0);
}

#[test]
fn vehicle_status_serializes_to_camel_case() {
    let s = VehicleStatus::default();
    let json = serde_json::to_value(&s).expect("serialize");
    // Verify the camelCase keys the frontend JavaScript expects
    assert!(json.get("unlockStatus").is_some(), "unlockStatus");
    assert!(json.get("flDoorOpen").is_some(), "flDoorOpen");
    assert!(json.get("acStatus").is_some(), "acStatus");
    assert!(json.get("chargingStatus").is_some(), "chargingStatus");
    assert!(json.get("acTemp").is_some(), "acTemp");
    assert!(json.get("acTime").is_some(), "acTime");
    assert!(json.get("updatedAt").is_some(), "updatedAt");
    // Snake-case keys must NOT appear
    assert!(json.get("unlock_status").is_none(), "no snake_case");
    assert!(json.get("fl_door_open").is_none(), "no snake_case");
}

#[test]
fn vehicle_status_json_round_trip() {
    let original = parse_vehicle_status("ROUNDTRIP123456789", &sample_data());
    let json_str = serde_json::to_string(&original).expect("serialize");
    let restored: VehicleStatus = serde_json::from_str(&json_str).expect("deserialize");
    assert_eq!(restored.vin, original.vin);
    assert_eq!(restored.mileage, original.mileage);
    assert_eq!(restored.soc, original.soc);
    assert_eq!(restored.range, original.range);
    assert_eq!(restored.ac_temp, original.ac_temp);
    assert_eq!(restored.ac_time, original.ac_time);
    assert_eq!(restored.petmode, original.petmode);
    assert_eq!(restored.fl_tire_pressure, original.fl_tire_pressure);
    assert_eq!(restored.unlock_status, original.unlock_status);
    assert_eq!(restored.longitude, original.longitude);
    assert_eq!(restored.latitude, original.latitude);
}
