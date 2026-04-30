#![recursion_limit = "256"]
pub(crate) mod crypto;
pub mod gwm;
mod mqtt;
pub(crate) mod web;

use gwm::{
    check_token, get_vehicle_status, get_vehicles, login, send_climate_command, send_lock_command,
};
use gwm::{VehicleInfo, VehicleStatus};
use log::{debug, error, info};
use mqtt::{parse_payload, publish_discovery, publish_state, setup as mqtt_setup, MQTT_CONFIG, QOS};
use regex::Regex;
use std::{fs, sync::LazyLock, time::Duration};
use tokio::{sync::Mutex, time};

const CONFIG_FILE: &str = "/opt/gwm2mqtt/configuration.json";

static REFRESH_INTERVAL: LazyLock<u64> = LazyLock::new(|| {
    option_env!("REFRESH_INTERVAL")
        .unwrap_or("10")
        .parse()
        .unwrap_or(10)
});
static VEHICLE_VIN: LazyLock<&str> = LazyLock::new(|| option_env!("VEHICLE_VIN").unwrap_or(""));

/// Per-vehicle runtime state: config, telemetry, debounce flags, and publish trigger.
#[derive(Clone)]
pub(crate) struct VehicleState {
    pub info: VehicleInfo,
    pub status: VehicleStatus,
    pub debounce_lock: String,
    pub debounce_ac: String,
    pub mqtt_publish: bool,
}

impl VehicleState {
    fn new(info: VehicleInfo, status: VehicleStatus) -> Self {
        Self {
            info,
            status,
            debounce_lock: String::new(),
            debounce_ac: String::new(),
            mqtt_publish: true,
        }
    }
}

pub(crate) static VEHICLES: LazyLock<Mutex<Vec<VehicleState>>> =
    LazyLock::new(|| Mutex::new(Vec::new()));

static MQTT_CLIENT: LazyLock<Mutex<Option<rumqttc::AsyncClient>>> =
    LazyLock::new(|| Mutex::new(None));

pub(crate) fn state_file_path(showed_vin: &str) -> String {
    format!("/opt/gwm2mqtt/state_{}.json", showed_vin)
}

fn vehicle_topic_prefix(info: &VehicleInfo) -> String {
    let brand = info.brand_name.to_lowercase();
    let last4 = info
        .showed_vin
        .get(info.showed_vin.len().saturating_sub(4)..)
        .unwrap_or("");
    format!("{}/{}_{}", &*MQTT_CONFIG.topic_prefix, brand, last4)
}

fn load_vehicle_status(showed_vin: &str) -> VehicleStatus {
    // Try per-vehicle path first; fall back to legacy state.json for migration
    let content = fs::read_to_string(state_file_path(showed_vin))
        .or_else(|_| fs::read_to_string("/opt/gwm2mqtt/state.json"))
        .unwrap_or_default();
    serde_json::from_str::<VehicleStatus>(&content).unwrap_or_default()
}

/// Apply debounce overrides to a status clone before publishing.
/// Clears the debounce sentinel once the API confirms the expected state.
fn apply_debounce(debounce_lock: &mut String, debounce_ac: &mut String, status: &mut VehicleStatus) {
    if debounce_lock == "lock" {
        if !status.unlock_status {
            *debounce_lock = String::new();
        } else {
            status.unlock_status = false;
        }
    } else if debounce_lock == "unlock" {
        if status.unlock_status {
            *debounce_lock = String::new();
        } else {
            status.unlock_status = true;
        }
    } else {
        *debounce_lock = String::new();
    }

    if debounce_ac == "on" {
        if status.ac_status {
            *debounce_ac = String::new();
        } else {
            status.ac_status = true;
        }
    } else if debounce_ac == "off" {
        if !status.ac_status {
            *debounce_ac = String::new();
        } else {
            status.ac_status = false;
        }
    } else {
        *debounce_ac = String::new();
    }
}

#[tokio::main]
pub async fn run(web_port: u16) {
    tokio::spawn(async move {
        if let Err(e) = web::start(web_port).await {
            error!("Web server error: {}", e);
        }
    });
    let (main_client, mut eventloop) = mqtt_setup().await.unwrap_or_else(|e| {
        panic!("Failed to set up MQTT client: {}", e);
    });
    *MQTT_CLIENT.lock().await = Some(main_client);
    tokio::spawn(async move {
        loop {
            match eventloop.poll().await {
                Ok(event) => handle_event(event).await,
                Err(e) => error!("Error: {:?}", e),
            }
        }
    });
    loop {
        match setup().await {
            Ok(_) => break,
            Err(e) => {
                error!(
                    "{e} — configure credentials via the web dashboard at /settings, retrying in 30 s"
                );
                time::sleep(Duration::from_secs(30)).await;
            }
        }
    }

    // Publish discovery and subscribe to command topics for every vehicle
    {
        let vehicle_infos: Vec<(VehicleInfo, String)> = {
            let vehicles = VEHICLES.lock().await;
            vehicles
                .iter()
                .map(|vs| (vs.info.clone(), vehicle_topic_prefix(&vs.info)))
                .collect()
        };
        let mutex_client = MQTT_CLIENT.lock().await;
        let client = mutex_client
            .as_ref()
            .unwrap_or_else(|| panic!("MQTT client is not initialized"));
        for (vehicle_info, topic_prefix) in &vehicle_infos {
            match publish_discovery(client, vehicle_info, topic_prefix).await {
                Ok(_) => info!("Discovery published for {}", vehicle_info.showed_vin),
                Err(e) => error!(
                    "Failed to publish discovery for {}: {}",
                    vehicle_info.showed_vin, e
                ),
            }
            for cmd in &["lock", "aircond", "petmode", "ac_timer", "ac_temperature"] {
                match client
                    .subscribe(format!("{}/{}/set", topic_prefix, cmd), QOS)
                    .await
                {
                    Ok(_) => {}
                    Err(e) => error!(
                        "Failed to subscribe to {}/{}/set: {}",
                        topic_prefix, cmd, e
                    ),
                }
            }
        }
    }

    publish_all_availability(&[("lock", "online"), ("aircond", "online"), ("petmode", "online")])
        .await;

    loop {
        update_all_vehicle_status().await;
        publish_pending_status().await;
        time::sleep(Duration::from_secs(*REFRESH_INTERVAL)).await;
    }
}

async fn setup() -> Result<(), String> {
    match login().await {
        Ok(true) => info!("Login successfully"),
        Ok(false) => return Err("Login returned false".to_string()),
        Err(e) => return Err(format!("Failed to login: {e}")),
    }
    let (token_valid, _) = check_token().await;
    if !token_valid {
        return Err("Token is invalid".to_string());
    }

    let all_vehicles = match get_vehicles().await {
        Ok(v) if !v.is_empty() => v,
        Ok(_) => return Err("No vehicle found".to_string()),
        Err(e) => return Err(format!("Failed to get vehicle list: {e}")),
    };

    // If VEHICLE_VIN is set, filter to that vehicle (backward-compat single-vehicle mode)
    let env_vin = *VEHICLE_VIN;
    let filtered: Vec<VehicleInfo> = if env_vin.is_empty() {
        all_vehicles
    } else {
        let prefix3 = env_vin.get(..3).unwrap_or("");
        let suffix4 = env_vin.get(env_vin.len().saturating_sub(4)..).unwrap_or("");
        let v: Vec<_> = all_vehicles
            .into_iter()
            .filter(|vi| {
                vi.showed_vin.starts_with(prefix3) && vi.showed_vin.ends_with(suffix4)
            })
            .collect();
        if v.is_empty() {
            return Err("Vehicle with specified VIN not found".to_string());
        }
        v
    };

    let mut vehicles_lock = VEHICLES.lock().await;
    vehicles_lock.clear();
    for vehicle_info in filtered {
        let mut status = load_vehicle_status(&vehicle_info.showed_vin);
        if !status.vin.is_empty() && status.vin != vehicle_info.vin {
            info!("State VIN mismatch for {}, resetting", vehicle_info.showed_vin);
            status = VehicleStatus::default();
        }
        info!(
            "Using vehicle: {} {} {} ({})",
            vehicle_info.brand_name,
            vehicle_info.vtype,
            vehicle_info.config,
            vehicle_info.showed_vin
        );
        vehicles_lock.push(VehicleState::new(vehicle_info, status));
    }

    let configs: Vec<&VehicleInfo> = vehicles_lock.iter().map(|vs| &vs.info).collect();
    match serde_json::to_string(&configs) {
        Ok(json) => {
            if let Err(e) = fs::write(CONFIG_FILE, json) {
                error!("Failed to write configuration file: {e}");
            }
        }
        Err(e) => error!("Failed to serialize configuration: {e}"),
    }

    Ok(())
}

async fn handle_event(event: rumqttc::Event) {
    let payload = parse_payload(event).await.unwrap_or_else(|e| {
        error!("Failed to parse payload: {}", e);
        serde_json::Value::Null
    });
    if payload.is_null() {
        return;
    }
    let topic = payload["topic"].as_str().unwrap_or("");

    // Match topic against every known vehicle's command prefix
    let matched: Option<(String, String, VehicleStatus, String, String)> = {
        let vehicles = VEHICLES.lock().await;
        let mut found = None;
        for vs in vehicles.iter() {
            let prefix = vehicle_topic_prefix(&vs.info);
            let pattern = format!(r"^{}/(?<topic>\w+)/set$", regex::escape(&prefix));
            if let Ok(re) = Regex::new(&pattern) {
                if let Some(caps) = re.captures(topic) {
                    let cap_topic = caps.name("topic").map_or("", |m| m.as_str()).to_string();
                    found = Some((
                        vs.info.showed_vin.clone(),
                        vs.info.vin.clone(),
                        vs.status.clone(),
                        vs.debounce_lock.clone(),
                        cap_topic,
                    ));
                    break;
                }
            }
        }
        found
    };

    let (showed_vin, internal_vin, vehicle_status, debounce_lock_val, capture_topic) =
        match matched {
            Some(m) => m,
            None => return,
        };

    match capture_topic.as_str() {
        "lock" => {
            let command = payload["payload"].as_str().unwrap_or("");
            let command = if command == "ON"
                && (!vehicle_status.unlock_status || debounce_lock_val == "lock")
            {
                "1"
            } else if command == "OFF"
                && (vehicle_status.unlock_status || debounce_lock_val == "unlock")
            {
                "0"
            } else {
                return;
            };
            publish_vehicle_availability(&showed_vin, &[("lock", "offline")]).await;
            {
                let mut vehicles = VEHICLES.lock().await;
                if let Some(vs) = vehicles.iter_mut().find(|vs| vs.info.showed_vin == showed_vin) {
                    vs.debounce_lock = if command == "1" {
                        "unlock".to_string()
                    } else {
                        "lock".to_string()
                    };
                }
            }
            publish_single_vehicle_status(&showed_vin).await;
            let showed_vin_clone = showed_vin.clone();
            tokio::spawn(async move {
                match send_lock_command(&internal_vin, command).await {
                    Ok(_) => info!("Lock command sent"),
                    Err(e) => {
                        error!("Failed to send lock command: {}", e);
                        let mut vehicles = VEHICLES.lock().await;
                        if let Some(vs) =
                            vehicles.iter_mut().find(|vs| vs.info.showed_vin == showed_vin_clone)
                        {
                            vs.debounce_lock = String::new();
                        }
                    }
                }
            });
            publish_single_vehicle_status(&showed_vin).await;
            publish_vehicle_availability(&showed_vin, &[("lock", "online")]).await;
        }
        "aircond" => {
            let command = payload["payload"].as_str().unwrap_or("");
            let debounce_ac_val = {
                let vehicles = VEHICLES.lock().await;
                vehicles
                    .iter()
                    .find(|vs| vs.info.showed_vin == showed_vin)
                    .map(|vs| vs.debounce_ac.clone())
                    .unwrap_or_default()
            };
            let command = if command == "ON"
                && (!vehicle_status.ac_status || debounce_ac_val == "off")
            {
                "1"
            } else if command == "OFF"
                && (vehicle_status.ac_status || debounce_ac_val == "on")
            {
                "0"
            } else {
                return;
            };
            publish_vehicle_availability(&showed_vin, &[("aircond", "offline")]).await;
            let (oper_time, oper_temp) = {
                let mut vehicles = VEHICLES.lock().await;
                match vehicles.iter_mut().find(|vs| vs.info.showed_vin == showed_vin) {
                    Some(vs) => {
                        vs.debounce_ac = if command == "1" {
                            "on".to_string()
                        } else {
                            "off".to_string()
                        };
                        (vs.status.ac_time, vs.status.ac_temp)
                    }
                    None => return,
                }
            };
            publish_single_vehicle_status(&showed_vin).await;
            let showed_vin_clone = showed_vin.clone();
            tokio::spawn(async move {
                match send_climate_command(&internal_vin, command, oper_time, oper_temp).await {
                    Ok(_) => info!("Aircond command sent"),
                    Err(e) => {
                        error!("Failed to send aircond command: {}", e);
                        let mut vehicles = VEHICLES.lock().await;
                        if let Some(vs) =
                            vehicles.iter_mut().find(|vs| vs.info.showed_vin == showed_vin_clone)
                        {
                            vs.debounce_ac = String::new();
                        }
                    }
                }
            });
            publish_single_vehicle_status(&showed_vin).await;
            publish_vehicle_availability(&showed_vin, &[("aircond", "online")]).await;
        }
        "ac_timer" => {
            if let Some(oper_time) = payload["payload"]
                .as_str()
                .and_then(|s| s.parse::<i64>().ok())
            {
                info!("Setting aircond timer to {}", oper_time);
                let mut vehicles = VEHICLES.lock().await;
                if let Some(vs) = vehicles.iter_mut().find(|vs| vs.info.showed_vin == showed_vin) {
                    vs.status.ac_time = oper_time;
                    if let Ok(s) = serde_json::to_string(&vs.status) {
                        let _ = fs::write(state_file_path(&showed_vin), s);
                    }
                    vs.mqtt_publish = true;
                }
            } else {
                error!("Invalid payload for ac_timer");
                return;
            }
            publish_single_vehicle_status(&showed_vin).await;
        }
        "ac_temperature" => {
            if let Some(oper_temp) = payload["payload"]
                .as_str()
                .and_then(|s| s.parse::<i64>().ok())
            {
                info!("Setting temperature to {}", oper_temp);
                let mut vehicles = VEHICLES.lock().await;
                if let Some(vs) = vehicles.iter_mut().find(|vs| vs.info.showed_vin == showed_vin) {
                    vs.status.ac_temp = oper_temp;
                    if let Ok(s) = serde_json::to_string(&vs.status) {
                        let _ = fs::write(state_file_path(&showed_vin), s);
                    }
                    vs.mqtt_publish = true;
                }
            } else {
                error!("Invalid payload for ac_temperature");
                return;
            }
            publish_single_vehicle_status(&showed_vin).await;
        }
        "petmode" => {
            let cmd_str = payload["payload"].as_str().unwrap_or("");
            let command = if cmd_str == "ON" && !vehicle_status.petmode {
                true
            } else if cmd_str == "OFF" && vehicle_status.petmode {
                false
            } else {
                return;
            };
            info!("Setting pet mode to {} for {}", command, showed_vin);
            let oper_temp = {
                let mut vehicles = VEHICLES.lock().await;
                match vehicles.iter_mut().find(|vs| vs.info.showed_vin == showed_vin) {
                    Some(vs) => {
                        vs.status.petmode = command;
                        if let Ok(s) = serde_json::to_string(&vs.status) {
                            let _ = fs::write(state_file_path(&showed_vin), s);
                        }
                        vs.mqtt_publish = true;
                        vs.status.ac_temp
                    }
                    None => return,
                }
            };
            publish_single_vehicle_status(&showed_vin).await;
            if command {
                let oper_time = 30i64;
                let showed_vin_clone = showed_vin.clone();
                let vin_clone = internal_vin.clone();
                let duration = Duration::from_secs(oper_time as u64 * 60);
                tokio::spawn(async move {
                    debug!("Starting pet mode loop for {}", showed_vin_clone);
                    loop {
                        let still_on = {
                            let v = VEHICLES.lock().await;
                            v.iter()
                                .find(|vs| vs.info.showed_vin == showed_vin_clone)
                                .map(|vs| vs.status.petmode)
                                .unwrap_or(false)
                        };
                        if !still_on {
                            break;
                        }
                        loop {
                            let ac_on = {
                                let v = VEHICLES.lock().await;
                                v.iter()
                                    .find(|vs| vs.info.showed_vin == showed_vin_clone)
                                    .map(|vs| vs.status.ac_status)
                                    .unwrap_or(false)
                            };
                            if !ac_on {
                                break;
                            }
                            debug!("AC is on for pet mode, waiting");
                            update_single_vehicle_status(&showed_vin_clone).await;
                            time::sleep(Duration::from_secs(1)).await;
                        }
                        {
                            let mut v = VEHICLES.lock().await;
                            if let Some(vs) =
                                v.iter_mut().find(|vs| vs.info.showed_vin == showed_vin_clone)
                            {
                                vs.debounce_ac = "on".to_string();
                            }
                        }
                        match send_climate_command(&vin_clone, "1", oper_time, oper_temp).await {
                            Ok(_) => info!("Pet mode AC on"),
                            Err(e) => {
                                error!("Pet mode AC failed: {e}");
                                let mut v = VEHICLES.lock().await;
                                if let Some(vs) =
                                    v.iter_mut().find(|vs| vs.info.showed_vin == showed_vin_clone)
                                {
                                    vs.debounce_ac = String::new();
                                }
                            }
                        }
                        time::sleep(duration).await;
                    }
                    debug!("Pet mode loop ended for {}", showed_vin_clone);
                });
            } else {
                debug!("Stopping pet mode for {}", showed_vin);
                let oper_time = 30i64;
                let showed_vin_clone = showed_vin.clone();
                {
                    let mut vehicles = VEHICLES.lock().await;
                    if let Some(vs) =
                        vehicles.iter_mut().find(|vs| vs.info.showed_vin == showed_vin)
                    {
                        vs.debounce_ac = "off".to_string();
                    }
                }
                tokio::spawn(async move {
                    match send_climate_command(&internal_vin, "0", oper_time, oper_temp).await {
                        Ok(_) => info!("Pet mode AC off"),
                        Err(e) => {
                            error!("Pet mode AC off failed: {e}");
                            let mut v = VEHICLES.lock().await;
                            if let Some(vs) =
                                v.iter_mut().find(|vs| vs.info.showed_vin == showed_vin_clone)
                            {
                                vs.debounce_ac = String::new();
                            }
                        }
                    }
                });
            }
        }
        _ => {
            error!("Unknown topic: {:?}", payload["topic"]);
        }
    }
}

async fn update_all_vehicle_status() {
    let showed_vins: Vec<String> = {
        let v = VEHICLES.lock().await;
        v.iter().map(|vs| vs.info.showed_vin.clone()).collect()
    };
    for showed_vin in &showed_vins {
        update_single_vehicle_status(showed_vin).await;
    }
}

pub(crate) async fn update_single_vehicle_status(showed_vin: &str) {
    info!("Updating vehicle status for {}", showed_vin);
    let (vin, model_id) = {
        let v = VEHICLES.lock().await;
        match v.iter().find(|vs| vs.info.showed_vin == showed_vin) {
            Some(vs) => (vs.info.vin.clone(), vs.info.model_id),
            None => return,
        }
    };
    match get_vehicle_status(&vin, model_id).await {
        Ok(mut new_status) => {
            let mut vehicles = VEHICLES.lock().await;
            if let Some(vs) = vehicles.iter_mut().find(|vs| vs.info.showed_vin == showed_vin) {
                new_status.ac_time = vs.status.ac_time;
                new_status.ac_temp = vs.status.ac_temp;
                new_status.petmode = vs.status.petmode;
                vs.status = new_status.clone();
                vs.mqtt_publish = true;
                match serde_json::to_string(&new_status) {
                    Ok(json) => {
                        let _ = fs::write(state_file_path(showed_vin), json);
                    }
                    Err(e) => error!("Failed to serialize state for {}: {}", showed_vin, e),
                }
            }
            debug!("Vehicle status updated for {}", showed_vin);
        }
        Err(e) => error!("Failed to get vehicle status for {}: {}", showed_vin, e),
    }
}

async fn publish_pending_status() {
    let pending: Vec<(VehicleInfo, VehicleStatus, String)> = {
        let mut vehicles = VEHICLES.lock().await;
        vehicles
            .iter_mut()
            .filter(|vs| vs.mqtt_publish)
            .map(|vs| {
                let mut status = vs.status.clone();
                apply_debounce(&mut vs.debounce_lock, &mut vs.debounce_ac, &mut status);
                vs.mqtt_publish = false;
                (vs.info.clone(), status, vehicle_topic_prefix(&vs.info))
            })
            .collect()
    };
    let mutex_client = MQTT_CLIENT.lock().await;
    if let Some(client) = mutex_client.as_ref() {
        for (info, status, prefix) in &pending {
            match publish_state(client, info, status, prefix).await {
                Ok(_) => info!("State published for {}", info.showed_vin),
                Err(e) => error!("Failed to publish state for {}: {}", info.showed_vin, e),
            }
        }
    } else {
        error!("MQTT client is not initialized, skipping state publish");
    }
}

async fn publish_single_vehicle_status(showed_vin: &str) {
    let result = {
        let mut vehicles = VEHICLES.lock().await;
        vehicles
            .iter_mut()
            .find(|vs| vs.info.showed_vin == showed_vin)
            .map(|vs| {
                let mut status = vs.status.clone();
                apply_debounce(&mut vs.debounce_lock, &mut vs.debounce_ac, &mut status);
                vs.mqtt_publish = false;
                (vs.info.clone(), status, vehicle_topic_prefix(&vs.info))
            })
    };
    if let Some((info, status, prefix)) = result {
        let mutex_client = MQTT_CLIENT.lock().await;
        if let Some(client) = mutex_client.as_ref() {
            match publish_state(client, &info, &status, &prefix).await {
                Ok(_) => info!("State published for {}", showed_vin),
                Err(e) => error!("Failed to publish state for {}: {}", showed_vin, e),
            }
        }
    }
}

async fn publish_all_availability(features: &[(&str, &str)]) {
    let prefixes: Vec<String> = {
        let v = VEHICLES.lock().await;
        v.iter().map(|vs| vehicle_topic_prefix(&vs.info)).collect()
    };
    let mutex_client = MQTT_CLIENT.lock().await;
    if let Some(client) = mutex_client.as_ref() {
        for prefix in &prefixes {
            for (feature, availability) in features {
                if let Err(e) = client
                    .publish(
                        format!("{}/{}/availability", prefix, feature),
                        QOS,
                        true,
                        *availability,
                    )
                    .await
                {
                    error!("Failed to publish availability: {}", e);
                }
            }
        }
    } else {
        error!("MQTT client is not initialized, skipping availability publish");
    }
}

async fn publish_vehicle_availability(showed_vin: &str, features: &[(&str, &str)]) {
    let prefix = {
        let v = VEHICLES.lock().await;
        v.iter()
            .find(|vs| vs.info.showed_vin == showed_vin)
            .map(|vs| vehicle_topic_prefix(&vs.info))
    };
    if let Some(prefix) = prefix {
        let mutex_client = MQTT_CLIENT.lock().await;
        if let Some(client) = mutex_client.as_ref() {
            for (feature, availability) in features {
                if let Err(e) = client
                    .publish(
                        format!("{}/{}/availability", prefix, feature),
                        QOS,
                        true,
                        *availability,
                    )
                    .await
                {
                    error!("Failed to publish availability: {}", e);
                }
            }
        }
    }
}
