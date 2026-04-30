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
use tokio::time::{self, Duration};

use crate::gwm::{send_climate_command, send_lock_command, CRED_FILE};
use crate::mqtt::MQTT_CONFIG_FILE;
use crate::{DEBOUNCE_AC, DEBOUNCE_LOCK, MQTT_PUBLISH, STATE_FILE, VEHICLE_INFO, VEHICLE_STATUS};

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

// ── API: status ───────────────────────────────────────────────────────────────

async fn api_status() -> impl IntoResponse {
    let vehicle_info = VEHICLE_INFO.lock().await.clone();
    let vehicle_status = VEHICLE_STATUS.lock().await.clone();
    let debounce_lock = DEBOUNCE_LOCK.lock().await.clone();
    let debounce_ac = DEBOUNCE_AC.lock().await.clone();

    let info_json = serde_json::to_value(&vehicle_info).unwrap_or(Value::Null);
    let status_json = serde_json::to_value(&vehicle_status).unwrap_or(Value::Null);

    Json(json!({
        "info": info_json,
        "status": status_json,
        "debounce": { "lock": debounce_lock, "ac": debounce_ac }
    }))
}

// ── API: config read/write ────────────────────────────────────────────────────

async fn api_config() -> impl IntoResponse {
    // MQTT config from file (password masked)
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

    // Credentials from file (password/pin masked, presence indicated)
    let creds = fs::read_to_string(CRED_FILE)
        .ok()
        .and_then(|c| serde_json::from_str::<Value>(&c).ok())
        .unwrap_or(json!({}));

    let email = creds["email"].as_str().unwrap_or("").to_string();
    let vehicle_vin = creds["vehicleVin"].as_str().unwrap_or("").to_string();
    let refresh_interval = creds["refreshInterval"].as_i64().unwrap_or(10);
    let has_password = creds["password"].as_str().is_some_and(|s| !s.is_empty());
    let has_pin = creds["pin"].as_str().is_some_and(|s| !s.is_empty());

    Json(json!({
        "mqtt": mqtt_val,
        "credentials": {
            "email": email,
            "hasPassword": has_password,
            "hasPin": has_pin,
            "vehicleVin": vehicle_vin,
            "refreshInterval": refresh_interval,
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
    // Preserve existing password when left blank
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
}

async fn api_config_credentials(Json(req): Json<CredentialsRequest>) -> impl IntoResponse {
    // Merge with existing file so blank fields don't erase saved values
    let existing = fs::read_to_string(CRED_FILE)
        .ok()
        .and_then(|c| serde_json::from_str::<Value>(&c).ok())
        .unwrap_or(json!({}));

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

    match serde_json::to_string_pretty(&config) {
        Ok(content) => match fs::write(CRED_FILE, content) {
            Ok(_) => {
                info!("Credentials updated via dashboard");
                Json(
                    json!({"success": true, "message": "Account settings saved. Restart to apply."}),
                )
            }
            Err(e) => {
                error!("Failed to write credentials: {e}");
                Json(json!({"success": false, "message": format!("Write failed: {e}")}))
            }
        },
        Err(e) => Json(json!({"success": false, "message": format!("Encode error: {e}")})),
    }
}

// ── API: commands ─────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct LockCommand {
    action: String,
}

async fn api_command_lock(Json(req): Json<LockCommand>) -> impl IntoResponse {
    let vin = VEHICLE_INFO.lock().await.vin.clone();
    if vin.is_empty() {
        return Json(json!({"success": false, "message": "Vehicle not initialised yet"}));
    }
    let command = match req.action.as_str() {
        "lock" => "0",
        "unlock" => "1",
        _ => {
            return Json(
                json!({"success": false, "message": "Invalid action. Use 'lock' or 'unlock'"}),
            )
        }
    };

    *DEBOUNCE_LOCK.lock().await = if command == "1" {
        "unlock".to_string()
    } else {
        "lock".to_string()
    };

    tokio::spawn(async move {
        match send_lock_command(&vin, command).await {
            Ok(_) => info!("Lock command sent via dashboard"),
            Err(e) => {
                error!("Lock command failed: {e}");
                *DEBOUNCE_LOCK.lock().await = String::new();
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
}

async fn api_command_ac(Json(req): Json<AcCommand>) -> impl IntoResponse {
    let vin = VEHICLE_INFO.lock().await.vin.clone();
    if vin.is_empty() {
        return Json(json!({"success": false, "message": "Vehicle not initialised yet"}));
    }
    let (ac_temp, ac_time) = {
        let s = VEHICLE_STATUS.lock().await;
        (s.ac_temp, s.ac_time)
    };
    let command = match req.action.as_str() {
        "on" => "1",
        "off" => "0",
        _ => {
            return Json(json!({"success": false, "message": "Invalid action. Use 'on' or 'off'"}))
        }
    };

    let oper_temp = req.temp.unwrap_or(ac_temp);
    let oper_time = req.time.unwrap_or(ac_time);

    *DEBOUNCE_AC.lock().await = if command == "1" {
        "on".to_string()
    } else {
        "off".to_string()
    };

    tokio::spawn(async move {
        match send_climate_command(&vin, command, oper_time, oper_temp).await {
            Ok(_) => info!("AC command sent via dashboard"),
            Err(e) => {
                error!("AC command failed: {e}");
                *DEBOUNCE_AC.lock().await = String::new();
            }
        }
    });

    Json(json!({"success": true, "message": format!("AC {} command sent", req.action)}))
}

#[derive(Deserialize)]
struct PetmodeCommand {
    action: String,
}

async fn api_command_petmode(Json(req): Json<PetmodeCommand>) -> impl IntoResponse {
    let enable = match req.action.as_str() {
        "on" => true,
        "off" => false,
        _ => {
            return Json(json!({"success": false, "message": "Invalid action. Use 'on' or 'off'"}))
        }
    };

    {
        let mut s = VEHICLE_STATUS.lock().await;
        s.petmode = enable;
        if let Ok(state_str) = serde_json::to_string(&*s) {
            let _ = fs::write(STATE_FILE, state_str);
        }
    }
    *MQTT_PUBLISH.lock().await = true;

    if enable {
        let vin = VEHICLE_INFO.lock().await.vin.clone();
        let ac_temp = VEHICLE_STATUS.lock().await.ac_temp;
        let oper_time = 30i64;
        tokio::spawn(async move {
            while VEHICLE_STATUS.lock().await.petmode {
                while VEHICLE_STATUS.lock().await.ac_status {
                    time::sleep(Duration::from_secs(1)).await;
                }
                *DEBOUNCE_AC.lock().await = "on".to_string();
                match send_climate_command(&vin, "1", oper_time, ac_temp).await {
                    Ok(_) => info!("Pet mode AC on"),
                    Err(e) => {
                        error!("Pet mode AC failed: {e}");
                        *DEBOUNCE_AC.lock().await = String::new();
                    }
                }
                time::sleep(Duration::from_secs(oper_time as u64 * 60)).await;
            }
        });
    } else {
        let vin = VEHICLE_INFO.lock().await.vin.clone();
        let (ac_time, ac_temp) = {
            let s = VEHICLE_STATUS.lock().await;
            (s.ac_time, s.ac_temp)
        };
        *DEBOUNCE_AC.lock().await = "off".to_string();
        tokio::spawn(async move {
            match send_climate_command(&vin, "0", ac_time, ac_temp).await {
                Ok(_) => info!("Pet mode AC off"),
                Err(e) => {
                    error!("Pet mode AC off failed: {e}");
                    *DEBOUNCE_AC.lock().await = String::new();
                }
            }
        });
    }

    Json(json!({"success": true, "message": format!("Pet mode {} activated", req.action)}))
}

#[derive(Deserialize)]
struct AcTimeCommand {
    time: i64,
}

async fn api_command_ac_time(Json(req): Json<AcTimeCommand>) -> impl IntoResponse {
    {
        let mut s = VEHICLE_STATUS.lock().await;
        s.ac_time = req.time;
        if let Ok(state_str) = serde_json::to_string(&*s) {
            let _ = fs::write(STATE_FILE, state_str);
        }
    }
    *MQTT_PUBLISH.lock().await = true;
    Json(json!({"success": true, "message": format!("AC timer set to {} min", req.time)}))
}

#[derive(Deserialize)]
struct AcTempCommand {
    temp: i64,
}

async fn api_command_ac_temp(Json(req): Json<AcTempCommand>) -> impl IntoResponse {
    {
        let mut s = VEHICLE_STATUS.lock().await;
        s.ac_temp = req.temp;
        if let Ok(state_str) = serde_json::to_string(&*s) {
            let _ = fs::write(STATE_FILE, state_str);
        }
    }
    *MQTT_PUBLISH.lock().await = true;
    Json(json!({"success": true, "message": format!("AC temperature set to {}°C", req.temp)}))
}

#[cfg(test)]
mod tests {
    use super::*;
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
        assert!(json.get("info").is_some(), "missing 'info' key");
        assert!(json.get("status").is_some(), "missing 'status' key");
        assert!(json.get("debounce").is_some(), "missing 'debounce' key");
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
        // Password must be masked (empty) in GET response
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

    // ── Local-state command tests (no network; file write errors are ignored) ─

    #[tokio::test]
    async fn ac_temp_command_returns_success() {
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
        assert_eq!(VEHICLE_STATUS.lock().await.ac_temp, 22);
    }

    #[tokio::test]
    async fn ac_time_command_returns_success() {
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
        assert_eq!(VEHICLE_STATUS.lock().await.ac_time, 20);
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
}
