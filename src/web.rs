use axum::{
    http::header,
    response::{Html, IntoResponse},
    routing::{get, post},
    Json, Router,
};
use log::{error, info};
use serde::Deserialize;
use serde_json::{json, Value};
use std::fs;
use std::path::Path;
use tokio::time::{self, Duration};
use url::Url;

use crate::crypto;
use crate::gwm::{send_climate_command, send_lock_command};
use crate::mqtt::MQTT_CONFIG_FILE;
use crate::{state_file_path, VehicleState, VEHICLES};

static INDEX_HTML: &str = include_str!("../static/index.html");
static SETTINGS_HTML: &str = include_str!("../static/settings.html");
static STYLE_CSS: &str = include_str!("../static/style.css");
static APP_JS: &str = include_str!("../static/app.js");

pub async fn start(port: u16) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let addr = format!("0.0.0.0:{port}");
    info!("Web dashboard listening on http://{addr}");
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, router()).await?;
    Ok(())
}

fn router() -> Router {
    Router::new()
        .route("/", get(index))
        .route("/settings", get(settings_page))
        .route("/style.css", get(style))
        .route("/app.js", get(script))
        .route("/api/status", get(api_status))
        .route("/api/config", get(api_config))
        .route("/api/config/mqtt", post(api_config_mqtt))
        .route("/api/config/credentials", post(api_config_credentials))
        .route("/api/command/lock", post(api_command_lock))
        .route("/api/command/ac", post(api_command_ac))
        .route("/api/command/petmode", post(api_command_petmode))
        .route("/api/command/ac_time", post(api_command_ac_time))
        .route("/api/command/ac_temp", post(api_command_ac_temp))
        .route("/api/restart", post(api_restart))
}

// ── Validation helpers ────────────────────────────────────────────────────────

fn validate_base_url(raw: &str) -> Result<String, String> {
    if raw.is_empty() {
        return Err("Base URL is required".to_string());
    }
    let parsed = Url::parse(raw).map_err(|_| format!("Invalid URL: {raw}"))?;
    match parsed.scheme() {
        "http" | "https" => {}
        s => return Err(format!("URL scheme must be http or https, got: {s}")),
    }
    if parsed.host_str().map(|h| h.is_empty()).unwrap_or(true) {
        return Err("URL must include a host".to_string());
    }
    // Ensure trailing slash so Url::join appends paths correctly
    let normalized = if raw.ends_with('/') {
        raw.to_string()
    } else {
        format!("{raw}/")
    };
    Ok(normalized)
}

// ── Static file handlers ──────────────────────────────────────────────────────

async fn index() -> Html<&'static str> {
    Html(INDEX_HTML)
}

async fn settings_page() -> Html<&'static str> {
    Html(SETTINGS_HTML)
}

async fn style() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/css; charset=utf-8")],
        STYLE_CSS,
    )
}

async fn script() -> impl IntoResponse {
    (
        [(
            header::CONTENT_TYPE,
            "application/javascript; charset=utf-8",
        )],
        APP_JS,
    )
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn vehicle_state_json(vs: &VehicleState) -> Value {
    json!({
        "vin": vs.info.showed_vin,
        "info": serde_json::to_value(&vs.info).unwrap_or(Value::Null),
        "status": serde_json::to_value(&vs.status).unwrap_or(Value::Null),
        "debounce": { "lock": vs.debounce_lock, "ac": vs.debounce_ac }
    })
}

/// Find vehicle by showed_vin (or the first vehicle if vin is None/empty).
fn find_vehicle_mut<'a>(
    vehicles: &'a mut [VehicleState],
    vin: Option<&str>,
) -> Option<&'a mut VehicleState> {
    match vin {
        Some(v) if !v.is_empty() => vehicles.iter_mut().find(|vs| vs.info.showed_vin == v),
        _ => vehicles.first_mut(),
    }
}

// ── API: status ───────────────────────────────────────────────────────────────

async fn api_status() -> impl IntoResponse {
    let vehicles = VEHICLES.lock().await;
    let result: Vec<Value> = vehicles.iter().map(vehicle_state_json).collect();
    Json(json!({ "vehicles": result }))
}

// ── API: config read/write ────────────────────────────────────────────────────

async fn api_config() -> impl IntoResponse {
    let mut mqtt_val = fs::read_to_string(MQTT_CONFIG_FILE)
        .ok()
        .and_then(|c| serde_json::from_str::<Value>(&c).ok())
        .unwrap_or_else(|| {
            json!({
                "broker": "localhost",
                "port": 1883,
                "user": "",
                "password": "",
                "discoveryTopic": "homeassistant",
                "topicPrefix": "gwm2mqtt"
            })
        });
    if let Some(obj) = mqtt_val.as_object_mut() {
        obj.insert("password".to_string(), Value::String(String::new()));
    }

    let creds = crypto::read_cred_json().unwrap_or(json!({}));
    let email = creds["email"].as_str().unwrap_or("").to_string();
    let vehicle_vin = creds["vehicleVin"].as_str().unwrap_or("").to_string();
    let refresh_interval = creds["refreshInterval"].as_i64().unwrap_or(10);
    let has_password = creds["password"].as_str().is_some_and(|s| !s.is_empty());
    let has_pin = creds["pin"].as_str().is_some_and(|s| !s.is_empty());
    let base_url = crate::gwm::get_base_url();

    Json(json!({
        "mqtt": mqtt_val,
        "credentials": {
            "email": email,
            "hasPassword": has_password,
            "hasPin": has_pin,
            "vehicleVin": vehicle_vin,
            "refreshInterval": refresh_interval,
            "baseUrl": base_url,
        }
    }))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MqttConfigRequest {
    broker: String,
    port: u16,
    user: String,
    password: String,
    discovery_topic: String,
    topic_prefix: String,
}

async fn api_config_mqtt(Json(req): Json<MqttConfigRequest>) -> impl IntoResponse {
    let password = if req.password.is_empty() {
        fs::read_to_string(MQTT_CONFIG_FILE)
            .ok()
            .and_then(|c| serde_json::from_str::<Value>(&c).ok())
            .and_then(|v| v["password"].as_str().map(String::from))
            .unwrap_or_default()
    } else {
        req.password
    };

    let config = json!({
        "broker": req.broker,
        "port": req.port,
        "user": req.user,
        "password": password,
        "discoveryTopic": req.discovery_topic,
        "topicPrefix": req.topic_prefix,
    });

    match serde_json::to_string_pretty(&config) {
        Ok(content) => match fs::write(MQTT_CONFIG_FILE, content) {
            Ok(_) => {
                info!("MQTT config updated via dashboard");
                Json(json!({"success": true, "message": "MQTT settings saved. Restart to apply."}))
            }
            Err(e) => {
                error!("Failed to write MQTT config: {e}");
                Json(json!({"success": false, "message": format!("Write failed: {e}")}))
            }
        },
        Err(e) => Json(json!({"success": false, "message": format!("Encode error: {e}")})),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CredentialsRequest {
    email: String,
    password: String,
    pin: String,
    vehicle_vin: String,
    refresh_interval: i64,
    base_url: String,
}

async fn api_config_credentials(Json(req): Json<CredentialsRequest>) -> impl IntoResponse {
    let base_url = match validate_base_url(&req.base_url) {
        Ok(u) => u,
        Err(e) => return Json(json!({"success": false, "message": e})),
    };

    let base_url_config = json!({"baseUrl": base_url});
    match serde_json::to_string_pretty(&base_url_config) {
        Ok(content) => {
            // Ensure parent directory exists before writing
            let target = Path::new(crate::gwm::BASE_URL_FILE);
            if let Some(dir) = target.parent() {
                if let Err(e) = fs::create_dir_all(dir) {
                    error!("Failed to create config directory: {e}");
                    return Json(
                        json!({"success": false, "message": format!("Directory error: {e}")}),
                    );
                }
            }
            // Atomic write: write to a temp file then rename
            let tmp_path = format!("{}.tmp", crate::gwm::BASE_URL_FILE);
            if let Err(e) = fs::write(&tmp_path, &content) {
                error!("Failed to write base URL config (tmp): {e}");
                return Json(
                    json!({"success": false, "message": format!("Write failed: {e}")}),
                );
            }
            if let Err(e) = fs::rename(&tmp_path, target) {
                let _ = fs::remove_file(&tmp_path);
                error!("Failed to rename base URL config: {e}");
                return Json(
                    json!({"success": false, "message": format!("Write failed: {e}")}),
                );
            }
        }
        Err(e) => return Json(json!({"success": false, "message": format!("Encode error: {e}")})),
    }
    crate::gwm::set_base_url_cache(base_url);

    let existing = crypto::read_cred_json().unwrap_or(json!({}));

    let email = if req.email.is_empty() {
        existing["email"].as_str().unwrap_or("").to_string()
    } else {
        req.email
    };
    let password = if req.password.is_empty() {
        existing["password"].as_str().unwrap_or("").to_string()
    } else {
        req.password
    };
    let pin = if req.pin.is_empty() {
        existing["pin"].as_str().unwrap_or("").to_string()
    } else {
        req.pin
    };

    let config = json!({
        "email": email,
        "password": password,
        "pin": pin,
        "vehicleVin": req.vehicle_vin,
        "refreshInterval": req.refresh_interval,
    });

    match crypto::write_cred_json(&config) {
        Ok(_) => {
            info!("Credentials updated via dashboard");
            Json(json!({"success": true, "message": "Account settings saved. Restart to apply."}))
        }
        Err(e) => {
            error!("Failed to write credentials: {e}");
            Json(json!({"success": false, "message": format!("Write failed: {e}")}))
        }
    }
}

// ── API: commands ─────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct LockCommand {
    action: String,
    vin: Option<String>,
}

async fn api_command_lock(Json(req): Json<LockCommand>) -> impl IntoResponse {
    let command = match req.action.as_str() {
        "lock" => "0",
        "unlock" => "1",
        _ => {
            return Json(
                json!({"success": false, "message": "Invalid action. Use 'lock' or 'unlock'"}),
            )
        }
    };

    let (internal_vin, showed_vin) = {
        let mut vehicles = VEHICLES.lock().await;
        match find_vehicle_mut(&mut vehicles, req.vin.as_deref()) {
            None => return Json(json!({"success": false, "message": "Vehicle not found"})),
            Some(vs) => {
                vs.debounce_lock = if command == "1" {
                    "unlock".to_string()
                } else {
                    "lock".to_string()
                };
                (vs.info.vin.clone(), vs.info.showed_vin.clone())
            }
        }
    };

    tokio::spawn(async move {
        match send_lock_command(&internal_vin, command).await {
            Ok(_) => info!("Lock command sent via dashboard"),
            Err(e) => {
                error!("Lock command failed: {e}");
                let mut vehicles = VEHICLES.lock().await;
                if let Some(vs) = vehicles
                    .iter_mut()
                    .find(|vs| vs.info.showed_vin == showed_vin)
                {
                    vs.debounce_lock = String::new();
                }
            }
        }
    });

    Json(json!({"success": true, "message": format!("{} command sent", req.action)}))
}

#[derive(Deserialize)]
struct AcCommand {
    action: String,
    temp: Option<i64>,
    time: Option<i64>,
    vin: Option<String>,
}

async fn api_command_ac(Json(req): Json<AcCommand>) -> impl IntoResponse {
    let command = match req.action.as_str() {
        "on" => "1",
        "off" => "0",
        _ => {
            return Json(json!({"success": false, "message": "Invalid action. Use 'on' or 'off'"}))
        }
    };

    let (internal_vin, showed_vin, oper_temp, oper_time) = {
        let mut vehicles = VEHICLES.lock().await;
        match find_vehicle_mut(&mut vehicles, req.vin.as_deref()) {
            None => return Json(json!({"success": false, "message": "Vehicle not found"})),
            Some(vs) => {
                let oper_temp = req.temp.unwrap_or(vs.status.ac_temp);
                let oper_time = req.time.unwrap_or(vs.status.ac_time);
                vs.debounce_ac = if command == "1" {
                    "on".to_string()
                } else {
                    "off".to_string()
                };
                (
                    vs.info.vin.clone(),
                    vs.info.showed_vin.clone(),
                    oper_temp,
                    oper_time,
                )
            }
        }
    };

    tokio::spawn(async move {
        match send_climate_command(&internal_vin, command, oper_time, oper_temp).await {
            Ok(_) => info!("AC command sent via dashboard"),
            Err(e) => {
                error!("AC command failed: {e}");
                let mut vehicles = VEHICLES.lock().await;
                if let Some(vs) = vehicles
                    .iter_mut()
                    .find(|vs| vs.info.showed_vin == showed_vin)
                {
                    vs.debounce_ac = String::new();
                }
            }
        }
    });

    Json(json!({"success": true, "message": format!("AC {} command sent", req.action)}))
}

#[derive(Deserialize)]
struct PetmodeCommand {
    action: String,
    vin: Option<String>,
}

async fn api_command_petmode(Json(req): Json<PetmodeCommand>) -> impl IntoResponse {
    let enable = match req.action.as_str() {
        "on" => true,
        "off" => false,
        _ => {
            return Json(json!({"success": false, "message": "Invalid action. Use 'on' or 'off'"}))
        }
    };

    let (internal_vin, showed_vin, ac_temp) = {
        let mut vehicles = VEHICLES.lock().await;
        match find_vehicle_mut(&mut vehicles, req.vin.as_deref()) {
            None => return Json(json!({"success": false, "message": "Vehicle not found"})),
            Some(vs) => {
                vs.status.petmode = enable;
                if let Ok(s) = serde_json::to_string(&vs.status) {
                    let _ = fs::write(state_file_path(&vs.info.showed_vin), s);
                }
                vs.mqtt_publish = true;
                (
                    vs.info.vin.clone(),
                    vs.info.showed_vin.clone(),
                    vs.status.ac_temp,
                )
            }
        }
    };

    if enable {
        let oper_time = 30i64;
        tokio::spawn(async move {
            while {
                let v = VEHICLES.lock().await;
                v.iter()
                    .find(|vs| vs.info.showed_vin == showed_vin)
                    .map(|vs| vs.status.petmode)
                    .unwrap_or(false)
            } {
                while {
                    let v = VEHICLES.lock().await;
                    v.iter()
                        .find(|vs| vs.info.showed_vin == showed_vin)
                        .map(|vs| vs.status.ac_status)
                        .unwrap_or(false)
                } {
                    time::sleep(Duration::from_secs(1)).await;
                }
                {
                    let mut v = VEHICLES.lock().await;
                    if let Some(vs) = v.iter_mut().find(|vs| vs.info.showed_vin == showed_vin) {
                        vs.debounce_ac = "on".to_string();
                    }
                }
                match send_climate_command(&internal_vin, "1", oper_time, ac_temp).await {
                    Ok(_) => info!("Pet mode AC on"),
                    Err(e) => {
                        error!("Pet mode AC failed: {e}");
                        let mut v = VEHICLES.lock().await;
                        if let Some(vs) = v.iter_mut().find(|vs| vs.info.showed_vin == showed_vin) {
                            vs.debounce_ac = String::new();
                        }
                    }
                }
                time::sleep(Duration::from_secs(oper_time as u64 * 60)).await;
            }
        });
    } else {
        let (ac_time, _) = {
            let v = VEHICLES.lock().await;
            v.iter()
                .find(|vs| vs.info.showed_vin == showed_vin)
                .map(|vs| (vs.status.ac_time, vs.status.ac_temp))
                .unwrap_or((30, 26))
        };
        {
            let mut v = VEHICLES.lock().await;
            if let Some(vs) = v.iter_mut().find(|vs| vs.info.showed_vin == showed_vin) {
                vs.debounce_ac = "off".to_string();
            }
        }
        tokio::spawn(async move {
            match send_climate_command(&internal_vin, "0", ac_time, ac_temp).await {
                Ok(_) => info!("Pet mode AC off"),
                Err(e) => {
                    error!("Pet mode AC off failed: {e}");
                    let mut v = VEHICLES.lock().await;
                    if let Some(vs) = v.iter_mut().find(|vs| vs.info.showed_vin == showed_vin) {
                        vs.debounce_ac = String::new();
                    }
                }
            }
        });
    }

    Json(json!({"success": true, "message": format!("Pet mode {} activated", req.action)}))
}

#[derive(Deserialize)]
struct AcTimeCommand {
    time: i64,
    vin: Option<String>,
}

async fn api_command_ac_time(Json(req): Json<AcTimeCommand>) -> impl IntoResponse {
    let mut vehicles = VEHICLES.lock().await;
    match find_vehicle_mut(&mut vehicles, req.vin.as_deref()) {
        None => Json(json!({"success": false, "message": "Vehicle not found"})),
        Some(vs) => {
            vs.status.ac_time = req.time;
            if let Ok(s) = serde_json::to_string(&vs.status) {
                let _ = fs::write(state_file_path(&vs.info.showed_vin), s);
            }
            vs.mqtt_publish = true;
            Json(json!({"success": true, "message": format!("AC timer set to {} min", req.time)}))
        }
    }
}

#[derive(Deserialize)]
struct AcTempCommand {
    temp: i64,
    vin: Option<String>,
}

async fn api_command_ac_temp(Json(req): Json<AcTempCommand>) -> impl IntoResponse {
    let mut vehicles = VEHICLES.lock().await;
    match find_vehicle_mut(&mut vehicles, req.vin.as_deref()) {
        None => Json(json!({"success": false, "message": "Vehicle not found"})),
        Some(vs) => {
            vs.status.ac_temp = req.temp;
            if let Ok(s) = serde_json::to_string(&vs.status) {
                let _ = fs::write(state_file_path(&vs.info.showed_vin), s);
            }
            vs.mqtt_publish = true;
            Json(
                json!({"success": true, "message": format!("AC temperature set to {}°C", req.temp)}),
            )
        }
    }
}

// ── API: service restart ──────────────────────────────────────────────────────

async fn api_restart() -> impl IntoResponse {
    info!("Service restart requested via dashboard");
    tokio::spawn(async {
        time::sleep(Duration::from_millis(500)).await;
        std::process::exit(0);
    });
    Json(json!({"success": true, "message": "Service is restarting…"}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gwm::{VehicleInfo, VehicleStatus};
    use crate::VehicleState;
    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    async fn body_json(res: axum::response::Response) -> serde_json::Value {
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        serde_json::from_slice(&bytes).unwrap()
    }

    /// Push a default test vehicle into VEHICLES if not already present.
    async fn ensure_test_vehicle() {
        let mut vehicles = VEHICLES.lock().await;
        if vehicles.is_empty() {
            vehicles.push(VehicleState {
                info: VehicleInfo {
                    showed_vin: "TEST00000000TEST".to_string(),
                    vin: "test_vin_internal".to_string(),
                    brand_name: "Test".to_string(),
                    ..VehicleInfo::default()
                },
                status: VehicleStatus::default(),
                debounce_lock: String::new(),
                debounce_ac: String::new(),
                mqtt_publish: false,
            });
        }
    }

    // ── Shape tests ───────────────────────────────────────────────────────────

    #[tokio::test]
    async fn status_endpoint_returns_ok_with_expected_keys() {
        let res = router()
            .oneshot(
                Request::builder()
                    .uri("/api/status")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let json = body_json(res).await;
        assert!(json.get("vehicles").is_some(), "missing 'vehicles' key");
    }

    #[tokio::test]
    async fn config_endpoint_returns_ok_with_expected_keys() {
        let res = router()
            .oneshot(
                Request::builder()
                    .uri("/api/config")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let json = body_json(res).await;
        assert!(json.get("mqtt").is_some(), "missing 'mqtt' key");
        assert!(
            json.get("credentials").is_some(),
            "missing 'credentials' key"
        );
    }

    #[tokio::test]
    async fn config_mqtt_section_has_expected_fields() {
        let res = router()
            .oneshot(
                Request::builder()
                    .uri("/api/config")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let json = body_json(res).await;
        let mqtt = &json["mqtt"];
        assert!(mqtt.get("broker").is_some());
        assert!(mqtt.get("port").is_some());
        assert_eq!(mqtt["password"].as_str().unwrap_or("x"), "");
    }

    #[tokio::test]
    async fn config_credentials_section_has_expected_fields() {
        let res = router()
            .oneshot(
                Request::builder()
                    .uri("/api/config")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let json = body_json(res).await;
        let creds = &json["credentials"];
        assert!(creds.get("email").is_some());
        assert!(creds.get("hasPassword").is_some());
        assert!(creds.get("hasPin").is_some());
        assert!(creds.get("refreshInterval").is_some());
    }

    // ── Invalid-action error tests ────────────────────────────────────────────

    #[tokio::test]
    async fn lock_invalid_action_returns_error() {
        let res = router()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/command/lock")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"action":"spin"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let json = body_json(res).await;
        assert_eq!(json["success"], false);
    }

    #[tokio::test]
    async fn ac_invalid_action_returns_error() {
        let res = router()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/command/ac")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"action":"blast"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let json = body_json(res).await;
        assert_eq!(json["success"], false);
    }

    #[tokio::test]
    async fn petmode_invalid_action_returns_error() {
        let res = router()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/command/petmode")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"action":"maybe"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let json = body_json(res).await;
        assert_eq!(json["success"], false);
    }

    // ── Local-state command tests ─────────────────────────────────────────────

    #[tokio::test]
    async fn ac_temp_command_returns_success() {
        ensure_test_vehicle().await;
        let res = router()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/command/ac_temp")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"temp":22}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let json = body_json(res).await;
        assert_eq!(json["success"], true);
        let vehicles = VEHICLES.lock().await;
        assert_eq!(vehicles[0].status.ac_temp, 22);
    }

    #[tokio::test]
    async fn ac_time_command_returns_success() {
        ensure_test_vehicle().await;
        let res = router()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/command/ac_time")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"time":20}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let json = body_json(res).await;
        assert_eq!(json["success"], true);
        let vehicles = VEHICLES.lock().await;
        assert_eq!(vehicles[0].status.ac_time, 20);
    }

    // ── Static file content-type tests ───────────────────────────────────────

    #[tokio::test]
    async fn style_endpoint_returns_css_content_type() {
        let res = router()
            .oneshot(
                Request::builder()
                    .uri("/style.css")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let ct = res.headers().get("content-type").unwrap().to_str().unwrap();
        assert!(ct.contains("text/css"), "content-type was: {ct}");
    }

    #[tokio::test]
    async fn script_endpoint_returns_js_content_type() {
        let res = router()
            .oneshot(
                Request::builder()
                    .uri("/app.js")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let ct = res.headers().get("content-type").unwrap().to_str().unwrap();
        assert!(ct.contains("javascript"), "content-type was: {ct}");
    }

    // ── validate_base_url unit tests ─────────────────────────────────────────

    #[test]
    fn validate_base_url_accepts_https() {
        assert!(validate_base_url("https://api.example.com/").is_ok());
    }

    #[test]
    fn validate_base_url_accepts_http() {
        assert!(validate_base_url("http://localhost:8080/").is_ok());
    }

    #[test]
    fn validate_base_url_rejects_empty() {
        let err = validate_base_url("").unwrap_err();
        assert!(err.contains("required"), "expected 'required' in: {err}");
    }

    #[test]
    fn validate_base_url_rejects_non_http_scheme() {
        assert!(validate_base_url("ftp://example.com/").is_err());
    }

    #[test]
    fn validate_base_url_rejects_plain_string() {
        assert!(validate_base_url("not-a-url").is_err());
    }

    // ── Credentials endpoint validation tests ────────────────────────────────

    #[tokio::test]
    async fn credentials_post_with_invalid_base_url_returns_error() {
        let res = router()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/config/credentials")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"email":"","password":"","pin":"","vehicleVin":"","refreshInterval":10,"baseUrl":"not-valid"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let json = body_json(res).await;
        assert_eq!(json["success"], false, "expected failure for invalid URL");
    }

    #[tokio::test]
    async fn credentials_post_with_empty_base_url_returns_error() {
        let res = router()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/config/credentials")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"email":"","password":"","pin":"","vehicleVin":"","refreshInterval":10,"baseUrl":""}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let json = body_json(res).await;
        assert_eq!(json["success"], false, "expected failure for empty URL");
    }

    #[tokio::test]
    async fn credentials_post_with_ftp_scheme_returns_error() {
        let res = router()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/config/credentials")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"email":"","password":"","pin":"","vehicleVin":"","refreshInterval":10,"baseUrl":"ftp://example.com/"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let json = body_json(res).await;
        assert_eq!(json["success"], false, "expected failure for ftp scheme");
    }

    #[tokio::test]
    async fn config_credentials_section_has_base_url_field() {
        let res = router()
            .oneshot(
                Request::builder()
                    .uri("/api/config")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let json = body_json(res).await;
        let creds = &json["credentials"];
        assert!(creds.get("baseUrl").is_some(), "missing 'baseUrl' field");
        assert!(
            !creds["baseUrl"].as_str().unwrap_or("").is_empty(),
            "baseUrl must be non-empty (falls back to default)"
        );
    }
}
