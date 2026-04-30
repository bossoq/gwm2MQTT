use chrono::prelude::*;
use jsonwebtoken::{decode, Algorithm, DecodingKey, Validation};
use log::{debug, error, info};
use md5::compute;
use rand::prelude::*;
use regex::Regex;
use reqwest::{header::HeaderMap, Client};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fs,
    sync::{LazyLock, RwLock},
};
use tokio::sync::Mutex;
use url::Url;
use urlencoding::encode;
use uuid::Uuid;

static EMAIL: LazyLock<Option<&str>> = LazyLock::new(|| option_env!("EMAIL"));
static PASSWORD: LazyLock<Option<&str>> = LazyLock::new(|| option_env!("PASSWORD"));
static PIN: LazyLock<Option<&str>> = LazyLock::new(|| option_env!("PIN"));

pub(crate) const DEFAULT_BASEURL: &str = "https://example.api.com/";
pub(crate) const BASE_URL_FILE: &str = "/opt/gwm2mqtt/baseurl.json";

static CACHED_BASE_URL: LazyLock<RwLock<Option<String>>> = LazyLock::new(|| RwLock::new(None));

pub(crate) fn get_base_url() -> String {
    if let Ok(guard) = CACHED_BASE_URL.read() {
        if let Some(url) = guard.as_ref() {
            return url.clone();
        }
    }
    let url = fs::read_to_string(BASE_URL_FILE)
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .and_then(|v| v["baseUrl"].as_str().map(String::from))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| DEFAULT_BASEURL.to_string());
    if let Ok(mut guard) = CACHED_BASE_URL.write() {
        *guard = Some(url.clone());
    }
    url
}

pub fn set_base_url_cache(url: String) {
    if let Ok(mut guard) = CACHED_BASE_URL.write() {
        *guard = Some(url);
    }
}
const LOGIN: &str = "app-api/api/v1.0/userAuth/loginAccount";
const REFRESHTOKEN: &str = "app-api/api/v1.0/userAuth/refreshToken";
const ACQUIREVEHICLES: &str = "app-api/api/v1.0/vehicle/acquireVehicles";
const GETVEHICLESTATUS: &str = "app-api/api/v2.0/vehicle/getLastStatus";
const GETREMOTECMDSTATUS: &str = "app-api/api/v1.0/vehicle/getRemoteCtrlResultT5";
const SENDREMOTECMD: &str = "app-api/api/v1.0/vehicle/T5/sendCmd";

static STD_HEADER: LazyLock<HeaderMap> = LazyLock::new(|| {
    let mut header = HeaderMap::new();
    header.insert(
        "rs",
        "2".parse().unwrap_or_else(|e| {
            panic!("Failed to parse header: {}", e);
        }),
    );
    header.insert(
        "country",
        "TH".parse().unwrap_or_else(|e| {
            panic!("Failed to parse header: {}", e);
        }),
    );
    header.insert(
        "channel",
        "APP".parse().unwrap_or_else(|e| {
            panic!("Failed to parse header: {}", e);
        }),
    );
    header.insert(
        "timezone",
        "GMT+07,00".parse().unwrap_or_else(|e| {
            panic!("Failed to parse header: {}", e);
        }),
    );
    header.insert(
        "language",
        "th".parse().unwrap_or_else(|e| {
            panic!("Failed to parse header: {}", e);
        }),
    );
    header.insert(
        "terminal",
        "GW_APP_Haval".parse().unwrap_or_else(|e| {
            panic!("Failed to parse header: {}", e);
        }),
    );
    header.insert(
        "cver",
        "1.8.3".parse().unwrap_or_else(|e| {
            panic!("Failed to parse header: {}", e);
        }),
    );
    header.insert(
        "regioncode",
        "TH".parse().unwrap_or_else(|e| {
            panic!("Failed to parse header: {}", e);
        }),
    );
    header.insert(
        "iccid",
        "a71dfe6316224f36bb760151d5ec2d5c"
            .parse()
            .unwrap_or_else(|e| {
                panic!("Failed to parse header: {}", e);
            }),
    );
    header.insert(
        "appid",
        "1".parse().unwrap_or_else(|e| {
            panic!("Failed to parse header: {}", e);
        }),
    );
    header.insert(
        "systemtype",
        "1".parse().unwrap_or_else(|e| {
            panic!("Failed to parse header: {}", e);
        }),
    );
    header.insert(
        "enterpriseid",
        "CC01".parse().unwrap_or_else(|e| {
            panic!("Failed to parse header: {}", e);
        }),
    );
    header.insert(
        "brand",
        "1".parse().unwrap_or_else(|e| {
            panic!("Failed to parse header: {}", e);
        }),
    );
    header.insert(
        "content-type",
        "application/json; charset=UTF-8"
            .parse()
            .unwrap_or_else(|e| {
                panic!("Failed to parse header: {}", e);
            }),
    );
    header.insert(
        "user-agent",
        "okhttp/4.2.2".parse().unwrap_or_else(|e| {
            panic!("Failed to parse header: {}", e);
        }),
    );
    header
});

const BTAPPKEY: &str = "2186661209";
const APP_SEC: &str = "a9664fd3f97665e202e73880de03a0d8";
const DEVICE_ID: &str = "a71dfe6316224f36bb760151d5ec2d5c";

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct Credentials {
    access_token: String,
    refresh_token: String,
    md5_pin: String,
}
impl Credentials {
    fn new(access_token: &str, refresh_token: &str, md5_pin: &str) -> Self {
        Credentials {
            access_token: access_token.to_string(),
            refresh_token: refresh_token.to_string(),
            md5_pin: md5_pin.to_string(),
        }
    }
}
impl Default for Credentials {
    fn default() -> Credentials {
        Credentials {
            access_token: "".to_string(),
            refresh_token: "".to_string(),
            md5_pin: "".to_string(),
        }
    }
}
static CREDENTIALS: LazyLock<Mutex<Credentials>> =
    LazyLock::new(|| Mutex::new(Credentials::default()));
// static CREDENTIALS: LazyLock<Mutex<Credentials>> = LazyLock::new(|| {
//     let access_token = var("ACCESS_TOKEN");
//     let refresh_token = var("REFRESH_TOKEN");
//     let pin = var("PIN");
//     if access_token.is_ok() && refresh_token.is_ok() && pin.is_ok() {
//         return Mutex::new(Credentials::new(
//             &access_token.unwrap(),
//             &refresh_token.unwrap(),
//             &format!("{:X}", compute(pin.unwrap())).to_ascii_lowercase(),
//         ));
//     } else {
//         match fs::read_to_string(CRED_FILE) {
//             Ok(content) => match serde_json::from_str::<Credentials>(&content) {
//                 Ok(creds) => Mutex::new(creds),
//                 Err(e) => {
//                     error!("Failed to parse credentials: {}", e);
//                     panic!("Failed to parse credentials");
//                 }
//             },
//             Err(_) => {
//                 error!("No credentials found, exiting");
//                 panic!("No credentials found");
//             }
//         }
//     }
// });
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct AccessToken {
    bean_id: String,
    channel: String,
    country: String,
    exp: i64,
    gw_id: String,
    iat: i64,
    iss: String,
    role_code: String,
}
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct RefreshToken {
    bean_id: String,
    exp: i64,
    role_code: String,
}
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct VehicleInfo {
    pub brand_name: String,
    pub config: String,
    pub model_id: i64,
    pub showed_vin: String,
    pub vin: String,
    pub vtype: String,
}
impl Default for VehicleInfo {
    fn default() -> Self {
        VehicleInfo {
            brand_name: "".to_string(),
            config: "".to_string(),
            model_id: 0,
            showed_vin: "".to_string(),
            vin: "".to_string(),
            vtype: "".to_string(),
        }
    }
}
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct VehicleStatus {
    pub vin: String,
    pub mileage: i64,                 // km
    pub soc: i64,                     // %
    pub range: i64,                   // km
    pub charge_time: i64,             // minutes
    pub charging_status: bool,        // stop 0 start 1 awaiting 2 finish 3 error 6 true if charging
    pub charging_status_desc: String, // stop 0 start 1 awaiting 2 finish 3 error 6 true if charging
    pub charging_port_plugged: bool,  // unplugged 0 plugged 1 true if plugged
    pub ac_status: bool,              // off 0 on 1 true if on
    pub air_filter_status: bool,      // off 0 on 1 true if on
    pub unlock_status: bool,          // lock 0 unlock 1 true if unlocked
    pub fl_door_open: bool,           // close 0 open 1 true if openned
    pub fr_door_open: bool,           // close 0 open 1 true if openned
    pub rl_door_open: bool,           // close 0 open 1 true if openned
    pub rr_door_open: bool,           // close 0 open 1 true if openned
    pub trunk_open: bool,             // close 0 open 1 true if openned
    pub fl_window_open: bool,         // close 1 open 0 true if openned
    pub fr_window_open: bool,         // close 1 open 0 true if openned
    pub rl_window_open: bool,         // close 1 open 0 true if openned
    pub rr_window_open: bool,         // close 1 open 0 true if openned
    pub sunroof_open: bool,           // close 3 open 6 true if openned
    pub fl_tire_pressure: f64,        // kPa convert to psi
    pub fr_tire_pressure: f64,        // kPa convert to psi
    pub rl_tire_pressure: f64,        // kPa convert to psi
    pub rr_tire_pressure: f64,        // kPa convert to psi
    pub fl_tire_temp: i64,            // °C
    pub fr_tire_temp: i64,            // °C
    pub rl_tire_temp: i64,            // °C
    pub rr_tire_temp: i64,            // °C
    pub fl_tire_pressure_alarm: bool, // clear 0 alarm 1 true if alarm
    pub fr_tire_pressure_alarm: bool, // clear 0 alarm 1 true if alarm
    pub rl_tire_pressure_alarm: bool, // clear 0 alarm 1 true if alarm
    pub rr_tire_pressure_alarm: bool, // clear 0 alarm 1 true if alarm
    pub fl_tire_temp_alarm: bool,     // clear 0 alarm 1 true if alarm
    pub fr_tire_temp_alarm: bool,     // clear 0 alarm 1 true if alarm
    pub rl_tire_temp_alarm: bool,     // clear 0 alarm 1 true if alarm
    pub rr_tire_temp_alarm: bool,     // clear 0 alarm 1 true if alarm
    pub head_light: bool,             // off 0 on 1 true if on
    pub left_turn_light: bool,        // off 0 on 1 true if on
    pub right_turn_light: bool,       // off 0 on 1 true if on
    pub longitude: f64,
    pub latitude: f64,
    pub ac_time: i64,
    pub ac_temp: i64,
    pub petmode: bool,
    pub updated_at: DateTime<Utc>,
}
impl Default for VehicleStatus {
    fn default() -> Self {
        VehicleStatus {
            vin: "".to_string(),
            mileage: 0,
            soc: 0,
            range: 0,
            charge_time: 0,
            charging_status: false,
            charging_status_desc: "".to_string(),
            charging_port_plugged: false,
            ac_status: false,
            air_filter_status: false,
            unlock_status: false,
            fl_door_open: false,
            fr_door_open: false,
            rl_door_open: false,
            rr_door_open: false,
            trunk_open: false,
            fl_window_open: false,
            fr_window_open: false,
            rl_window_open: false,
            rr_window_open: false,
            sunroof_open: false,
            fl_tire_pressure: 0.0,
            fr_tire_pressure: 0.0,
            rl_tire_pressure: 0.0,
            rr_tire_pressure: 0.0,
            fl_tire_temp: 0,
            fr_tire_temp: 0,
            rl_tire_temp: 0,
            rr_tire_temp: 0,
            fl_tire_pressure_alarm: false,
            fr_tire_pressure_alarm: false,
            rl_tire_pressure_alarm: false,
            rr_tire_pressure_alarm: false,
            fl_tire_temp_alarm: false,
            fr_tire_temp_alarm: false,
            rl_tire_temp_alarm: false,
            rr_tire_temp_alarm: false,
            head_light: false,
            left_turn_light: false,
            right_turn_light: false,
            longitude: 0.0,
            latitude: 0.0,
            ac_time: 15,
            ac_temp: 26,
            petmode: false,
            updated_at: Utc::now(),
        }
    }
}

fn random_string(len: usize) -> String {
    let rng = rand::rng();
    rng.sample_iter(rand::distr::Alphanumeric)
        .take(len)
        .map(char::from)
        .collect::<String>()
        .to_ascii_lowercase()
}

fn calc_header(method: &str, url_path: &str, payload_str: &str, headers: HeaderMap) -> HeaderMap {
    let nonce = random_string(16);
    let timestamp = Utc::now().timestamp_millis();
    let sign = format!(
        "{}{}bt-auth-appkey:{}bt-auth-nonce:{}bt-auth-timestamp:{}{}{}",
        method, url_path, BTAPPKEY, nonce, timestamp, payload_str, APP_SEC
    );
    let sign = encode(&sign).into_owned();
    let mut hasher = Sha256::new();
    hasher.update(sign);
    let sign = format!("{:X}", hasher.finalize()).to_ascii_lowercase();
    let mut headers = headers.clone();
    headers.insert(
        "bt-auth-appkey",
        BTAPPKEY.parse().unwrap_or_else(|e| {
            panic!("Failed to parse bt-auth-appkey: {}", e);
        }),
    );
    headers.insert(
        "bt-auth-sign",
        sign.parse().unwrap_or_else(|e| {
            panic!("Failed to parse bt-auth-sign: {}", e);
        }),
    );
    headers.insert(
        "bt-auth-nonce",
        nonce.parse().unwrap_or_else(|e| {
            panic!("Failed to parse bt-auth-nonce: {}", e);
        }),
    );
    headers.insert(
        "bt-auth-timestamp",
        timestamp.to_string().parse().unwrap_or_else(|e| {
            panic!("Failed to parse bt-auth-timestamp: {}", e);
        }),
    );
    headers
}

fn sign_calc_get(url: &Url, headers: HeaderMap) -> HeaderMap {
    let mut payload_str = String::new();
    let url_path = url.path();
    if let Some(query) = url.query() {
        let mut query = query.split('&').collect::<Vec<&str>>();
        query.sort();
        query.iter().for_each(|q| {
            let mut q = q.split('=');
            let key = q.next();
            let value = q.next();
            if let Some(key) = key {
                if let Some(value) = value {
                    payload_str.push_str(key.to_string().to_ascii_lowercase().as_str());
                    payload_str.push('=');
                    payload_str.push_str(value);
                }
            }
        });
    }
    let re = Regex::new(r"\s+").unwrap_or_else(|e| {
        panic!("Failed to create regex: {}", e);
    });
    let payload_str = re.replace_all(&payload_str, "").to_string();
    calc_header("GET", url_path, &payload_str, headers)
}

fn sign_calc_post(payload: &Value, url: &Url, headers: HeaderMap) -> HeaderMap {
    let payload_str = format!("json={}", payload).replace(' ', "");
    let url_path = url.path();
    calc_header("POST", url_path, &payload_str, headers)
}

fn read_cred_file() -> Result<Value, String> {
    crate::crypto::read_cred_json()
}

pub async fn login() -> Result<bool, String> {
    info!("Logging in using email");
    // Dashboard/file settings take priority; compile-time env vars are the fallback.
    let (email_owned, password_owned): (String, String) = {
        let from_file = read_cred_file().ok().and_then(|v| {
            let e = v["email"]
                .as_str()
                .filter(|s| !s.is_empty())
                .map(String::from)?;
            let p = v["password"]
                .as_str()
                .filter(|s| !s.is_empty())
                .map(String::from)?;
            Some((e, p))
        });
        if let Some(pair) = from_file {
            pair
        } else {
            match (*EMAIL, *PASSWORD) {
                (Some(e), Some(p)) if !e.is_empty() && !p.is_empty() => {
                    (e.to_string(), p.to_string())
                }
                _ => {
                    return Err(
                        "No credentials found. Save via the dashboard or set EMAIL/PASSWORD \
                         at build time."
                            .to_string(),
                    )
                }
            }
        }
    };
    let email = email_owned.as_str();
    let password = password_owned.as_str();
    let url = Url::parse(&get_base_url())
        .map_err(|e| format!("Invalid base URL: {e}"))?
        .join(LOGIN)
        .map_err(|e| format!("Failed to build login URL: {e}"))?;
    let mut headers = STD_HEADER.clone();
    headers.insert(
        "accesstoken",
        "".parse().unwrap_or_else(|e| {
            panic!("Failed to parse header: {}", e);
        }),
    );
    let payload = json!({
        "account": email,
        "password": password,
        "agreement": ["1", "2"],
        "appType": 0,
        "country": "TH",
        "deviceId": DEVICE_ID,
        "model": "GWM2MQTT",
        "pushToken": "",
        "type": 1,
        "isEncrypt": false
    });
    let headers = sign_calc_post(&payload, &url, headers);
    let client = Client::new();
    let res = match client
        .post(url)
        .headers(headers)
        .json(&payload)
        .send()
        .await
    {
        Ok(res) => res,
        Err(e) => {
            error!("Failed to login: {}", e);
            return Err("Failed to login".to_string());
        }
    };
    let res = match res.json::<serde_json::Value>().await {
        Ok(res) => res,
        Err(e) => {
            error!("Failed to parse response: {}", e);
            return Err("Failed to parse response".to_string());
        }
    };
    if res["code"] != "000000" {
        error!(
            "Failed to login: {}",
            res["description"].as_str().unwrap_or("Unknown error")
        );
        return Ok(false);
    }
    let access_token = match res["data"]["accessToken"].as_str() {
        Some(token) => token,
        None => {
            error!("No access token found in response");
            return Err("No access token found in response".to_string());
        }
    };
    let refresh_token = match res["data"]["refreshToken"].as_str() {
        Some(token) => token,
        None => {
            error!("No refresh token found in response");
            return Err("No refresh token found in response".to_string());
        }
    };
    let mut creds = CREDENTIALS.lock().await;
    // Dashboard/file settings take priority; compile-time PIN is the fallback.
    let pin_str = read_cred_file()
        .ok()
        .and_then(|v| {
            v["pin"]
                .as_str()
                .filter(|s| !s.is_empty())
                .map(String::from)
        })
        .unwrap_or_else(|| PIN.unwrap_or("").to_string());
    let md5pin = if !pin_str.is_empty() {
        format!("{:X}", compute(pin_str.as_str())).to_ascii_lowercase()
    } else {
        String::new()
    };
    *creds = Credentials::new(access_token, refresh_token, &md5pin);
    Ok(true)
}

pub async fn check_token() -> (bool, bool) {
    info!("Check Token Validity");
    let creds = CREDENTIALS.lock().await;
    let access_token = creds.access_token.clone();
    let refresh_token = creds.refresh_token.clone();
    if access_token.is_empty() {
        error!("No access token found");
        return (false, false);
    }
    if refresh_token.is_empty() {
        error!("No refresh token found");
        return (false, false);
    }
    let key = DecodingKey::from_secret(&[]);
    let mut validation = Validation::new(Algorithm::RS256);
    validation.insecure_disable_signature_validation();
    validation.validate_aud = false;
    let mut token_valid = false;
    let mut token_expired = false;
    match decode::<AccessToken>(&access_token, &key, &validation) {
        Ok(_) => {
            info!("Access token is valid");
            token_valid = true;
        }
        Err(e) => {
            if e.kind() != &jsonwebtoken::errors::ErrorKind::ExpiredSignature {
                error!("Failed to decode access token: {}", e);
            } else {
                info!("Access token is expired");
                token_valid = true;
                token_expired = true;
            }
        }
    }
    match decode::<RefreshToken>(&refresh_token, &key, &validation) {
        Ok(_) => {
            info!("Refresh token is valid");
        }
        Err(e) => {
            if e.kind() != &jsonwebtoken::errors::ErrorKind::ExpiredSignature {
                error!("Failed to decode refresh token: {}", e);
                token_valid = false;
            } else {
                info!("Refresh token is expired");
                token_valid = false;
                token_expired = true;
            }
        }
    }
    (token_valid, token_expired)
}

pub async fn get_accesstoken() -> Result<String, String> {
    info!("Getting access token");
    let key = DecodingKey::from_secret(&[]);
    let mut validation = Validation::new(Algorithm::RS256);
    validation.insecure_disable_signature_validation();
    validation.validate_aud = false;
    // Read all needed credential data under a single lock to avoid TOCTOU between
    // the validity check and the subsequent read/refresh.
    let (is_expired, access_token, refresh_token, md5_pin) = {
        let creds = CREDENTIALS.lock().await;
        if creds.access_token.is_empty() || creds.refresh_token.is_empty() {
            error!("No credentials found");
            return Err("No credentials found".to_string());
        }
        match decode::<RefreshToken>(&creds.refresh_token, &key, &validation) {
            Ok(_) => {}
            Err(e) => {
                error!("Refresh token invalid or expired: {}", e);
                return Err("Refresh token invalid or expired".to_string());
            }
        }
        let is_expired = match decode::<AccessToken>(&creds.access_token, &key, &validation) {
            Ok(_) => false,
            Err(e) => {
                if e.kind() == &jsonwebtoken::errors::ErrorKind::ExpiredSignature {
                    true
                } else {
                    error!("Access token invalid: {}", e);
                    return Err("Access token invalid".to_string());
                }
            }
        };
        (
            is_expired,
            creds.access_token.clone(),
            creds.refresh_token.clone(),
            creds.md5_pin.clone(),
        )
    };
    if !is_expired {
        info!("Got access token");
        return Ok(access_token);
    }
    info!("Token is expired, refreshing");
    let url = Url::parse(&get_base_url())
        .map_err(|e| format!("Invalid base URL: {e}"))?
        .join(REFRESHTOKEN)
        .map_err(|e| format!("Failed to build refresh token URL: {e}"))?;
    let mut headers = STD_HEADER.clone();
    headers.insert(
        "accesstoken",
        "".parse().unwrap_or_else(|e| {
            panic!("Failed to parse header: {}", e);
        }),
    );
    let payload = json!({
        "accessToken": access_token,
        "deviceId": DEVICE_ID,
        "refreshToken": refresh_token,
    });
    let headers = sign_calc_post(&payload, &url, headers);
    let client = Client::new();
    let res = match client
        .post(url)
        .headers(headers)
        .json(&payload)
        .send()
        .await
    {
        Ok(res) => res,
        Err(e) => {
            error!("Failed to refresh token: {}", e);
            return Err("Failed to refresh token".to_string());
        }
    };
    let res = match res.json::<serde_json::Value>().await {
        Ok(res) => res,
        Err(e) => {
            error!("Failed to parse response: {}", e);
            return Err("Failed to parse response".to_string());
        }
    };
    let new_access_token = match res["data"]["accessToken"].as_str() {
        Some(token) => token.to_string(),
        None => {
            error!("No access token found in response");
            return Err("No access token found in response".to_string());
        }
    };
    let new_refresh_token = match res["data"]["refreshToken"].as_str() {
        Some(token) => token.to_string(),
        None => {
            error!("No refresh token found in response");
            return Err("No refresh token found in response".to_string());
        }
    };
    *CREDENTIALS.lock().await = Credentials::new(&new_access_token, &new_refresh_token, &md5_pin);
    info!("Access token refreshed");
    Ok(new_access_token)
}

pub async fn get_vehicles() -> Result<Vec<VehicleInfo>, String> {
    info!("Getting vehicles");
    let access_token = match get_accesstoken().await {
        Ok(token) => token,
        Err(e) => return Err(e),
    };
    let url = Url::parse(&get_base_url())
        .map_err(|e| format!("Invalid base URL: {e}"))?
        .join(ACQUIREVEHICLES)
        .map_err(|e| format!("Failed to build vehicles URL: {e}"))?;
    let mut headers = STD_HEADER.clone();
    headers.insert(
        "accesstoken",
        access_token.parse().unwrap_or_else(|e| {
            panic!("Failed to parse header: {}", e);
        }),
    );
    let headers = sign_calc_get(&url, headers);
    let client = Client::new();
    let res = match client.get(url).headers(headers).send().await {
        Ok(res) => res,
        Err(e) => {
            error!("Failed to get vehicles: {}", e);
            return Err("Failed to get vehicles".to_string());
        }
    };
    let res = match res.json::<serde_json::Value>().await {
        Ok(res) => {
            if res["code"] != "000000" {
                error!(
                    "Failed to get vehicles: {}",
                    res["description"].as_str().unwrap_or("Unknown error")
                );
                return Err("Failed to get vehicles".to_string());
            }
            let data = res["data"].clone();
            let data_arr = match data.as_array() {
                Some(arr) => arr,
                None => {
                    error!("No vehicles found in response");
                    return Err("No vehicles found in response".to_string());
                }
            };
            let res = data_arr
                .iter()
                .map(|v| {
                    VehicleInfo::deserialize(v).unwrap_or_else(|e| {
                        error!("Failed to parse vehicle info: {}", e);
                        VehicleInfo::default()
                    })
                })
                .collect::<Vec<VehicleInfo>>();
            res
        }
        Err(e) => {
            error!("Failed to parse response: {}", e);
            return Err("Failed to parse response".to_string());
        }
    };
    info!("Vehicles retrieved");
    Ok(res)
}

pub async fn get_vehicle_status(vin: &str, model_id: i64) -> Result<VehicleStatus, String> {
    info!("Getting vehicle status");
    let access_token = match get_accesstoken().await {
        Ok(token) => token,
        Err(e) => return Err(e),
    };
    let url = Url::parse(&get_base_url())
        .map_err(|e| format!("Invalid base URL: {e}"))?
        .join(GETVEHICLESTATUS)
        .map_err(|e| format!("Failed to build vehicle status URL: {e}"))?
        .join(&format!("?vin={}&modelId={}", vin, model_id))
        .map_err(|e| format!("Failed to build vehicle status URL with query: {e}"))?;
    let mut headers = STD_HEADER.clone();
    headers.insert(
        "accesstoken",
        access_token.parse().unwrap_or_else(|e| {
            panic!("Failed to parse header: {}", e);
        }),
    );
    let headers = sign_calc_get(&url, headers);
    let client = Client::new();
    let res = match client.get(url).headers(headers).send().await {
        Ok(res) => res,
        Err(e) => {
            error!("Failed to get vehicle status: {}", e);
            return Err("Failed to get vehicle status".to_string());
        }
    };
    let res = match res.json::<serde_json::Value>().await {
        Ok(res) => {
            if res["code"] != "000000" {
                error!(
                    "Failed to get vehicle status: {}",
                    res["description"].as_str().unwrap_or("Unknown error")
                );
                return Err("Failed to get vehicle status".to_string());
            }
            parse_vehicle_status(vin, &res["data"])
        }
        Err(e) => {
            error!("Failed to parse response: {}", e);
            return Err("Failed to parse response".to_string());
        }
    };
    info!("Vehicle status retrieved");
    Ok(res)
}

pub async fn send_climate_command(
    vin: &str,
    switch_order: &str,
    oper_time: i64,
    temperature: i64,
) -> Result<bool, String> {
    info!("Sending climate command");
    let access_token = match get_accesstoken().await {
        Ok(token) => token,
        Err(e) => return Err(e),
    };
    let md5_pin = CREDENTIALS.lock().await.md5_pin.clone();
    if md5_pin.is_empty() {
        return Err("No PIN set".to_string());
    }
    let url = Url::parse(&get_base_url())
        .map_err(|e| format!("Invalid base URL: {e}"))?
        .join(SENDREMOTECMD)
        .map_err(|e| format!("Failed to build climate command URL: {e}"))?;
    let seq_no = format!("{}1234", Uuid::new_v4().to_string().replace("-", ""));
    let mut headers = STD_HEADER.clone();
    headers.insert(
        "accesstoken",
        access_token.parse().unwrap_or_else(|e| {
            panic!("Failed to parse header: {}", e);
        }),
    );
    headers.insert(
        "vin",
        vin.parse().unwrap_or_else(|e| {
            panic!("Failed to parse header: {}", e);
        }),
    );
    let payload = json!({
        "instructions": {
            "0x04": {
                "airConditioner": {
                    "operationTime": oper_time.to_string(),
                    "switchOrder": switch_order,
                    "temperature": temperature.to_string(),
                }
            }
        },
        "remoteType": "0",
        "securityPassword": md5_pin,
        "seqNo": seq_no,
        "type": "2",
        "vin": vin,
    });
    let headers = sign_calc_post(&payload, &url, headers);
    let client = Client::new();
    let res = match client
        .post(url)
        .headers(headers)
        .json(&payload)
        .send()
        .await
    {
        Ok(res) => res,
        Err(e) => {
            error!("Failed to send climate command: {}", e);
            return Err("Failed to send climate command".to_string());
        }
    };
    let res = match res.json::<serde_json::Value>().await {
        Ok(res) => res,
        Err(e) => {
            error!("Failed to parse response: {}", e);
            return Err("Failed to parse response".to_string());
        }
    };
    if res["code"] != "000000" {
        error!(
            "Failed to send climate command: {}",
            res["description"].as_str().unwrap_or("Unknown error")
        );
        return Err("Failed to send climate command".to_string());
    }
    let mut i = 0;
    while i < 10 {
        match get_remote_cmd_status(vin, &seq_no, "0x04").await {
            Ok(status) => {
                if status {
                    info!("Climate command sent");
                    return Ok(true);
                } else {
                    debug!("Waiting for climate command to be sent");
                }
            }
            Err(e) => return Err(e),
        }
        i += 1;
        debug!("Waiting for climate command to be sent");
        tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
    }
    error!("Failed to send climate command");
    Err("Failed to send climate command".to_string())
}

pub async fn send_lock_command(vin: &str, switch_order: &str) -> Result<bool, String> {
    info!("Sending lock command");
    let access_token = match get_accesstoken().await {
        Ok(token) => token,
        Err(e) => return Err(e),
    };
    let md5_pin = CREDENTIALS.lock().await.md5_pin.clone();
    if md5_pin.is_empty() {
        return Err("No PIN set".to_string());
    }
    let url = Url::parse(&get_base_url())
        .map_err(|e| format!("Invalid base URL: {e}"))?
        .join(SENDREMOTECMD)
        .map_err(|e| format!("Failed to build lock command URL: {e}"))?;
    let seq_no = format!("{}1234", Uuid::new_v4().to_string().replace("-", ""));
    let mut headers = STD_HEADER.clone();
    headers.insert(
        "accesstoken",
        access_token.parse().unwrap_or_else(|e| {
            panic!("Failed to parse header: {}", e);
        }),
    );
    headers.insert(
        "vin",
        vin.parse().unwrap_or_else(|e| {
            panic!("Failed to parse header: {}", e);
        }),
    );
    let payload = json!({
        "instructions": {
            "0x05": {
                "switchOrder": switch_order
            }
        },
        "remoteType": "0",
        "securityPassword": md5_pin,
        "seqNo": seq_no,
        "type": "2",
        "vin": vin,
    });
    let headers = sign_calc_post(&payload, &url, headers);
    let client = Client::new();
    let res = match client
        .post(url)
        .headers(headers)
        .json(&payload)
        .send()
        .await
    {
        Ok(res) => res,
        Err(e) => {
            error!("Failed to send lock command: {}", e);
            return Err("Failed to send lock command".to_string());
        }
    };
    let res = match res.json::<serde_json::Value>().await {
        Ok(res) => res,
        Err(e) => {
            error!("Failed to parse response: {}", e);
            return Err("Failed to parse response".to_string());
        }
    };
    if res["code"] != "000000" {
        error!(
            "Failed to send lock command: {}",
            res["description"].as_str().unwrap_or("Unknown error")
        );
        return Err("Failed to send lock command".to_string());
    }
    let mut i = 0;
    while i < 10 {
        match get_remote_cmd_status(vin, &seq_no, "0x05").await {
            Ok(status) => {
                if status {
                    info!("Lock command sent");
                    return Ok(true);
                } else {
                    debug!("Waiting for lock command to be sent");
                }
            }
            Err(e) => return Err(e),
        }
        i += 1;
        debug!("Waiting for lock command to be sent");
        tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
    }
    error!("Failed to send lock command");
    Err("Failed to send lock command".to_string())
}

async fn get_remote_cmd_status(vin: &str, seq_no: &str, remote_type: &str) -> Result<bool, String> {
    info!("Getting remote command status");
    let access_token = match get_accesstoken().await {
        Ok(token) => token,
        Err(e) => return Err(e),
    };
    let url = Url::parse(&get_base_url())
        .map_err(|e| format!("Invalid base URL: {e}"))?
        .join(GETREMOTECMDSTATUS)
        .map_err(|e| format!("Failed to build remote cmd status URL: {e}"))?
        .join(&format!("?seqNo={}", seq_no))
        .map_err(|e| format!("Failed to build remote cmd status URL with query: {e}"))?;
    let mut headers = STD_HEADER.clone();
    headers.insert(
        "accesstoken",
        access_token.parse().unwrap_or_else(|e| {
            panic!("Failed to parse header: {}", e);
        }),
    );
    headers.insert(
        "vin",
        vin.parse().unwrap_or_else(|e| {
            panic!("Failed to parse header: {}", e);
        }),
    );
    let headers = sign_calc_get(&url, headers);
    let client = Client::new();
    match client.get(url).headers(headers).send().await {
        Ok(res) => match res.json::<serde_json::Value>().await {
            Ok(res) => {
                if res["code"] != "000000" {
                    error!(
                        "Failed to get remote command status: {}",
                        res["description"].as_str().unwrap_or("Unknown error")
                    );
                    Err("Failed to get remote command status".to_string())
                } else {
                    info!("Remote command status retrieved");
                    if let Some(res_remote_type) = res["data"][0]["remoteType"].as_str() {
                        if res_remote_type == remote_type {
                            info!("Command {} acknowledged by vehicle", remote_type);
                            return Ok(true);
                        }
                        debug!(
                            "Response remote type {} does not match expected {}",
                            res_remote_type, remote_type
                        );
                        Ok(false)
                    } else {
                        debug!("No command result data yet for this sequence");
                        Ok(false)
                    }
                }
            }
            Err(e) => {
                error!("Failed to parse response: {}", e);
                Err("Failed to parse response".to_string())
            }
        },
        Err(e) => {
            error!("Failed to get remote command status: {}", e);
            Err("Failed to get remote command status".to_string())
        }
    }
}

pub fn parse_vehicle_status(vin: &str, data: &Value) -> VehicleStatus {
    let charge_status_desc = HashMap::from([
        ("0", "Not Charging"),
        ("1", "Charging"),
        ("2", "Awaiting Charge"),
        ("3", "Finish Charge"),
        ("4", "Unknown 4"),
        ("5", "Unknown 5"),
        ("6", "Error"),
    ]);
    let default_arr = vec![];
    let items = data["items"]
        .as_array()
        .unwrap_or_else(|| {
            error!("No items found in vehicle status response");
            &default_arr
        })
        .iter()
        .map(|i| {
            let key = i["code"].as_str().unwrap_or("Unknown");
            let value = i["value"].clone();
            (key.to_string(), value)
        })
        .collect::<Map<String, Value>>();
    let null = Value::Null;
    let get = |key: &str| items.get(key).unwrap_or(&null);
    VehicleStatus {
        vin: vin.to_string(),
        mileage: get("2103010").as_i64().unwrap_or(0),
        soc: get("2013021").as_i64().unwrap_or(0),
        range: get("2011007").as_i64().unwrap_or(0),
        charge_time: get("2013022")
            .as_str()
            .unwrap_or("0")
            .parse::<i64>()
            .unwrap_or(0),
        charging_status: get("2041142").as_str().unwrap_or("0") == "1",
        charging_status_desc: charge_status_desc
            .get(&get("2041142").as_str().unwrap_or("0"))
            .unwrap_or(&"Unknown Mapping")
            .to_string(),
        charging_port_plugged: get("2042082").as_str().unwrap_or("0") == "1",
        ac_status: get("2202001").as_str().unwrap_or("0") == "1",
        air_filter_status: get("2078020").as_str().unwrap_or("0") == "1",
        unlock_status: get("2208001").as_str().unwrap_or("0") == "1",
        fl_door_open: get("2206004").as_str().unwrap_or("0") == "1",
        fr_door_open: get("2206002").as_str().unwrap_or("0") == "1",
        rl_door_open: get("2206005").as_str().unwrap_or("0") == "1",
        rr_door_open: get("2206003").as_str().unwrap_or("0") == "1",
        trunk_open: get("2206001").as_str().unwrap_or("0") == "1",
        fl_window_open: get("2210002").as_str().unwrap_or("1") == "0",
        fr_window_open: get("2210001").as_str().unwrap_or("1") == "0",
        rl_window_open: get("2210004").as_str().unwrap_or("1") == "0",
        rr_window_open: get("2210003").as_str().unwrap_or("1") == "0",
        sunroof_open: get("2210005").as_str().unwrap_or("3") == "6",
        fl_tire_pressure: (get("2101001").as_f64().unwrap_or(0.0) * 14.503773773020923).round()
            / 100.0,
        fr_tire_pressure: (get("2101002").as_f64().unwrap_or(0.0) * 14.503773773020923).round()
            / 100.0,
        rl_tire_pressure: (get("2101003").as_f64().unwrap_or(0.0) * 14.503773773020923).round()
            / 100.0,
        rr_tire_pressure: (get("2101004").as_f64().unwrap_or(0.0) * 14.503773773020923).round()
            / 100.0,
        fl_tire_temp: get("2101005")
            .as_str()
            .unwrap_or("0")
            .parse::<i64>()
            .unwrap_or(0),
        fr_tire_temp: get("2101006")
            .as_str()
            .unwrap_or("0")
            .parse::<i64>()
            .unwrap_or(0),
        rl_tire_temp: get("2101007")
            .as_str()
            .unwrap_or("0")
            .parse::<i64>()
            .unwrap_or(0),
        rr_tire_temp: get("2101008")
            .as_str()
            .unwrap_or("0")
            .parse::<i64>()
            .unwrap_or(0),
        fl_tire_pressure_alarm: get("2102001").as_str().unwrap_or("0") == "1",
        fr_tire_pressure_alarm: get("2102002").as_str().unwrap_or("0") == "1",
        rl_tire_pressure_alarm: get("2102003").as_str().unwrap_or("0") == "1",
        rr_tire_pressure_alarm: get("2102004").as_str().unwrap_or("0") == "1",
        fl_tire_temp_alarm: get("2102007").as_str().unwrap_or("0") == "1",
        fr_tire_temp_alarm: get("2102008").as_str().unwrap_or("0") == "1",
        rl_tire_temp_alarm: get("2102009").as_str().unwrap_or("0") == "1",
        rr_tire_temp_alarm: get("2102010").as_str().unwrap_or("0") == "1",
        head_light: get("2204007").as_str().unwrap_or("0") == "1",
        left_turn_light: get("2204009").as_str().unwrap_or("0") == "1",
        right_turn_light: get("2204010").as_str().unwrap_or("0") == "1",
        longitude: data["longitude"].as_f64().unwrap_or(0.0),
        latitude: data["latitude"].as_f64().unwrap_or(0.0),
        updated_at: DateTime::from_timestamp_millis(data["updateTime"].as_i64().unwrap_or(0))
            .unwrap_or(Utc::now()),
        // ac_time, ac_temp, petmode are local-only; caller must restore from global state
        ..VehicleStatus::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_base_url_is_parseable() {
        assert!(Url::parse(DEFAULT_BASEURL).is_ok());
    }

    #[test]
    fn default_base_url_value() {
        assert_eq!(DEFAULT_BASEURL, "https://example.api.com/");
    }

    #[test]
    fn set_base_url_cache_updates_get_base_url() {
        set_base_url_cache("https://cached.test.example/".to_string());
        assert_eq!(get_base_url(), "https://cached.test.example/");
        // Reset so other tests see a clean state
        if let Ok(mut g) = CACHED_BASE_URL.write() {
            *g = None;
        }
    }

    #[test]
    fn get_base_url_returns_default_when_cache_empty_and_no_file() {
        // Clear cache first
        if let Ok(mut g) = CACHED_BASE_URL.write() {
            *g = None;
        }
        // BASE_URL_FILE does not exist in test env, so default is returned
        let url = get_base_url();
        assert!(!url.is_empty());
        assert!(
            Url::parse(&url).is_ok(),
            "returned URL must be parseable: {url}"
        );
        // Restore cache to None for other tests
        if let Ok(mut g) = CACHED_BASE_URL.write() {
            *g = None;
        }
    }
}
