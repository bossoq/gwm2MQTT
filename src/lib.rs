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
use mqtt::{
    parse_payload, publish_discovery, publish_state, setup as mqtt_setup, MQTT_CONFIG, QOS,
};
use regex::Regex;
use std::{collections::HashMap, fs, sync::LazyLock, time::Duration};
use tokio::{sync::Mutex, time};

const CONFIG_FILE: &str = "/opt/gwm2mqtt/configuration.json";
pub(crate) const STATE_FILE: &str = "/opt/gwm2mqtt/state.json";
static REFRESH_INTERVAL: LazyLock<u64> = LazyLock::new(|| {
    option_env!("REFRESH_INTERVAL")
        .unwrap_or("10")
        .parse()
        .unwrap_or(10)
});
static VEHICLE_VIN: LazyLock<&str> = LazyLock::new(|| option_env!("VEHICLE_VIN").unwrap_or(""));
pub(crate) static VEHICLE_INFO: LazyLock<Mutex<VehicleInfo>> =
    LazyLock::new(|| Mutex::new(VehicleInfo::default()));
pub(crate) static VEHICLE_STATUS: LazyLock<Mutex<VehicleStatus>> = LazyLock::new(|| {
    let vehicle_status = match fs::read_to_string(STATE_FILE) {
        Ok(content) => match serde_json::from_str::<VehicleStatus>(&content) {
            Ok(vehicle_status) => vehicle_status,
            Err(e) => {
                error!("Failed to parse state file: {}", e);
                VehicleStatus::default()
            }
        },
        Err(e) => {
            error!("Failed to read state file: {}", e);
            VehicleStatus::default()
        }
    };
    Mutex::new(vehicle_status)
});
pub(crate) static MQTT_PUBLISH: LazyLock<Mutex<bool>> = LazyLock::new(|| Mutex::new(false));
pub(crate) static DEBOUNCE_LOCK: LazyLock<Mutex<String>> =
    LazyLock::new(|| Mutex::new("".to_string()));
pub(crate) static DEBOUNCE_AC: LazyLock<Mutex<String>> =
    LazyLock::new(|| Mutex::new("".to_string()));
static MQTT_CLIENT: LazyLock<Mutex<Option<rumqttc::AsyncClient>>> =
    LazyLock::new(|| Mutex::new(None));

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
                Ok(event) => {
                    handle_event(event).await;
                }
                Err(e) => {
                    error!("Error: {:?}", e);
                }
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
    let vehicle_info = VEHICLE_INFO.lock().await.clone();
    let brand_name = vehicle_info.brand_name.clone().to_lowercase();
    let last_vin = vehicle_info.showed_vin.get(15..).unwrap_or("").to_string();
    let topic_prefix = format!("{}/{}_{}", &*MQTT_CONFIG.topic_prefix, brand_name, last_vin);
    {
        let mutex_client = MQTT_CLIENT.lock().await;
        let client = mutex_client.as_ref().unwrap_or_else(|| {
            panic!("MQTT client is not initialized");
        });
        match publish_discovery(client, &vehicle_info, &topic_prefix).await {
            Ok(_) => {
                info!("Discovery published");
            }
            Err(e) => {
                error!("Failed to publish discovery: {}", e);
            }
        }
    }
    let switch_topic = HashMap::from([
        ("lock", "online".to_string()),
        ("aircond", "online".to_string()),
        ("petmode", "online".to_string()),
    ]);
    publish_availability(switch_topic).await;
    {
        let mutex_client = MQTT_CLIENT.lock().await;
        let client = mutex_client.as_ref().unwrap_or_else(|| {
            panic!("MQTT client is not initialized");
        });
        match client
            .subscribe(format!("{}/{}/set", topic_prefix, "lock"), QOS)
            .await
        {
            Ok(_) => info!("Subscribed to lock topic"),
            Err(e) => {
                error!("Failed to subscribe to lock topic: {}", e);
            }
        };
        match client
            .subscribe(format!("{}/{}/set", topic_prefix, "aircond"), QOS)
            .await
        {
            Ok(_) => info!("Subscribed to aircond topic"),
            Err(e) => {
                error!("Failed to subscribe to aircond topic: {}", e);
            }
        };
        match client
            .subscribe(format!("{}/{}/set", topic_prefix, "petmode"), QOS)
            .await
        {
            Ok(_) => info!("Subscribed to petmode topic"),
            Err(e) => {
                error!("Failed to subscribe to petmode topic: {}", e);
            }
        };
        match client
            .subscribe(format!("{}/{}/set", topic_prefix, "ac_timer"), QOS)
            .await
        {
            Ok(_) => info!("Subscribed to ac_timer topic"),
            Err(e) => {
                error!("Failed to subscribe to ac_timer topic: {}", e);
            }
        };
        match client
            .subscribe(format!("{}/{}/set", topic_prefix, "ac_temperature"), QOS)
            .await
        {
            Ok(_) => info!("Subscribed to ac_temperature topic"),
            Err(e) => {
                error!("Failed to subscribe to ac_temperature topic: {}", e);
            }
        };
    }

    loop {
        update_vehicle_status().await;
        if *MQTT_PUBLISH.lock().await {
            publish_vehicle_status().await;
            *MQTT_PUBLISH.lock().await = false;
        }
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
    let env_vin = &VEHICLE_VIN;
    let vin_prefix = env_vin.get(..3).unwrap_or("");
    let vin_suffix = env_vin.get(env_vin.len().saturating_sub(4)..).unwrap_or("");
    let mut need_reconfigure = false;
    let mut vehicle_info = match read_vehicle_configuration().await {
        Ok(vehicle_info) => {
            if vehicle_info.vin.is_empty() || vehicle_info.model_id == 0 {
                info!("VIN or model ID is not set, reconfiguring");
                need_reconfigure = true;
                VehicleInfo::default()
            } else if env_vin.is_empty() {
                info!("VEHICLE_VIN is not set, using VIN from configuration file");
                vehicle_info
            } else {
                info!("VEHICLE_VIN is set, using VIN from environment variable");
                if vehicle_info.showed_vin.starts_with(vin_prefix)
                    && vehicle_info.showed_vin.ends_with(vin_suffix)
                {
                    info!("VIN from configuration file matches the environment variable");
                    vehicle_info
                } else {
                    info!("VIN from configuration file does not match the environment variable, reconfiguring");
                    need_reconfigure = true;
                    VehicleInfo::default()
                }
            }
        }
        Err(_) => {
            need_reconfigure = true;
            VehicleInfo::default()
        }
    };
    if need_reconfigure {
        let vehicle_infos = match get_vehicles().await {
            Ok(vehicle_infos) => vehicle_infos,
            Err(_) => return Err("Failed to get vehicle list".to_string()),
        };
        if vehicle_infos.is_empty() {
            return Err("No vehicle found".to_string());
        }
        if env_vin.is_empty() {
            info!("VEHICLE_VIN is not set, using the first vehicle in the list");
            vehicle_info = vehicle_infos[0].clone();
        } else {
            info!(
                "VEHICLE_VIN is set, using the vehicle with the VIN from the environment variable"
            );
            for vehicle in vehicle_infos {
                if vehicle.showed_vin.starts_with(vin_prefix)
                    && vehicle.showed_vin.ends_with(vin_suffix)
                {
                    vehicle_info = vehicle;
                    break;
                }
            }
            if vehicle_info.vin.is_empty() {
                return Err(
                    "Vehicle with the VIN from the environment variable not found".to_string(),
                );
            }
        }
    }
    let config_json = serde_json::to_string(&vehicle_info)
        .map_err(|e| format!("Failed to serialize configuration: {e}"))?;
    fs::write(CONFIG_FILE, config_json)
        .map_err(|e| format!("Failed to write configuration file: {e}"))?;
    let mut global_vehicle_info = VEHICLE_INFO.lock().await;
    *global_vehicle_info = vehicle_info.clone();
    let full_name = format!(
        "{} {} {} ({})",
        vehicle_info.brand_name, vehicle_info.vtype, vehicle_info.config, vehicle_info.showed_vin
    );
    info!("Using vehicle: {}", full_name);
    let mut vehicle_status = VEHICLE_STATUS.lock().await;
    if vehicle_status.vin == vehicle_info.vin {
        info!("State VIN matches. Publishing vehicle status");
    } else {
        info!("State VIN does not match. Resetting vehicle status");
        *vehicle_status = VehicleStatus::default();
        let state_json = serde_json::to_string(&*vehicle_status)
            .map_err(|e| format!("Failed to serialize state: {e}"))?;
        fs::write(STATE_FILE, state_json)
            .map_err(|e| format!("Failed to write state file: {e}"))?;
    }
    *MQTT_PUBLISH.lock().await = true;
    Ok(())
}

async fn handle_event(event: rumqttc::Event) {
    let vehicle_info = VEHICLE_INFO.lock().await.clone();
    let vehicle_status = VEHICLE_STATUS.lock().await.clone();
    let brand_name = vehicle_info.brand_name.clone().to_lowercase();
    let last_vin = vehicle_info.showed_vin.get(15..).unwrap_or("").to_string();
    let topic_prefix = format!("{}/{}_{}", &*MQTT_CONFIG.topic_prefix, brand_name, last_vin);
    let payload = parse_payload(event).await.unwrap_or_else(|e| {
        error!("Failed to parse payload: {}", e);
        serde_json::Value::Null
    });
    if payload.is_null() {
        return;
    }
    let topic = payload["topic"].as_str().unwrap_or("");
    let re = Regex::new(format!(r"^{}\/(?<topic>\w+)\/set$", topic_prefix.clone()).as_str())
        .unwrap_or_else(|e| {
            panic!("Failed to create regex: {}", e);
        });
    let captures = re.captures(topic);
    if captures.is_none() {
        return;
    }
    let captures = captures.unwrap_or_else(|| {
        panic!("Failed to capture topic from: {}", topic);
    });
    let capture_topic = captures.name("topic");
    if capture_topic.is_none() {
        return;
    }
    let capture_topic = capture_topic.map_or("", |m| m.as_str());
    match capture_topic {
        "lock" => {
            let command = payload["payload"].as_str().unwrap_or("");
            let command = if command == "ON"
                && (!vehicle_status.unlock_status || DEBOUNCE_LOCK.lock().await.clone() == "lock")
            {
                "1"
            } else if command == "OFF"
                && (vehicle_status.unlock_status || DEBOUNCE_LOCK.lock().await.clone() == "unlock")
            {
                "0"
            } else {
                return;
            };
            let switch_topic = HashMap::from([("lock", "offline".to_string())]);
            publish_availability(switch_topic).await;
            *DEBOUNCE_LOCK.lock().await = if command == "1" {
                "unlock".to_string()
            } else {
                "lock".to_string()
            };
            publish_vehicle_status().await;
            let vehicle_info_clone = vehicle_info.clone();
            tokio::spawn(async move {
                match send_lock_command(&vehicle_info_clone.vin, command).await {
                    Ok(_) => {
                        info!("Lock command sent");
                    }
                    Err(e) => {
                        error!("Failed to send lock command: {}", e);
                        *DEBOUNCE_LOCK.lock().await = "".to_string();
                    }
                }
            });
            let switch_topic = HashMap::from([("lock", "online".to_string())]);
            publish_vehicle_status().await;
            publish_availability(switch_topic).await;
        }
        "aircond" => {
            let command = payload["payload"].as_str().unwrap_or("");
            let command = if command == "ON"
                && (!vehicle_status.ac_status || DEBOUNCE_AC.lock().await.clone() == "off")
            {
                "1"
            } else if command == "OFF"
                && (vehicle_status.ac_status || DEBOUNCE_AC.lock().await.clone() == "on")
            {
                "0"
            } else {
                return;
            };
            let switch_topic = HashMap::from([("aircond", "offline".to_string())]);
            publish_availability(switch_topic).await;
            *DEBOUNCE_AC.lock().await = if command == "1" {
                "on".to_string()
            } else {
                "off".to_string()
            };
            publish_vehicle_status().await;
            let oper_time = vehicle_status.ac_time;
            let oper_temp = vehicle_status.ac_temp;
            let vehicle_info_clone = vehicle_info.clone();
            tokio::spawn(async move {
                match send_climate_command(&vehicle_info_clone.vin, command, oper_time, oper_temp)
                    .await
                {
                    Ok(_) => {
                        info!("Aircond command sent");
                    }
                    Err(e) => {
                        error!("Failed to send aircond command: {}", e);
                        *DEBOUNCE_AC.lock().await = "".to_string();
                    }
                }
            });
            let switch_topic = HashMap::from([("aircond", "online".to_string())]);
            publish_vehicle_status().await;
            publish_availability(switch_topic).await;
        }
        "ac_timer" => {
            let payload_str = payload["payload"].as_str();
            if payload_str.is_none() {
                return;
            }
            if let Some(payload_str) = payload_str {
                if let Ok(oper_time) = payload_str.parse::<i64>() {
                    info!("Setting aircond timer to {}", oper_time);
                    {
                        let mut mut_vehicle_status = VEHICLE_STATUS.lock().await;
                        mut_vehicle_status.ac_time = oper_time;
                        match serde_json::to_string(&*mut_vehicle_status) {
                            Ok(state_str) => match fs::write(STATE_FILE, state_str) {
                                Ok(_) => {}
                                Err(e) => {
                                    error!("Failed to write state file: {}", e);
                                }
                            },
                            Err(e) => {
                                error!("Failed to serialize vehicle status: {}", e);
                            }
                        }
                    }
                    publish_vehicle_status().await;
                } else {
                    error!("Invalid payload");
                }
            }
        }
        "ac_temperature" => {
            let payload_str = payload["payload"].as_str();
            if payload_str.is_none() {
                return;
            }
            if let Some(payload_str) = payload_str {
                if let Ok(oper_temp) = payload_str.parse::<i64>() {
                    info!("Setting temperature to {}", oper_temp);
                    {
                        let mut mut_vehicle_status = VEHICLE_STATUS.lock().await;
                        mut_vehicle_status.ac_temp = oper_temp;
                        match serde_json::to_string(&*mut_vehicle_status) {
                            Ok(state_str) => match fs::write(STATE_FILE, state_str) {
                                Ok(_) => {}
                                Err(e) => {
                                    error!("Failed to write state file: {}", e);
                                }
                            },
                            Err(e) => {
                                error!("Failed to serialize vehicle status: {}", e);
                            }
                        }
                    }
                    publish_vehicle_status().await;
                } else {
                    error!("Invalid payload");
                }
            }
        }
        "petmode" => {
            let cmd_str = payload["payload"].as_str().unwrap_or("");
            let current_petmode = VEHICLE_STATUS.lock().await.petmode;
            let command = if cmd_str == "ON" && !current_petmode {
                true
            } else if cmd_str == "OFF" && current_petmode {
                false
            } else {
                return;
            };
            info!("Setting pet mode to {}", command);
            {
                let mut mut_vehicle_status = VEHICLE_STATUS.lock().await;
                mut_vehicle_status.petmode = command;
                match serde_json::to_string(&*mut_vehicle_status) {
                    Ok(state_str) => match fs::write(STATE_FILE, state_str) {
                        Ok(_) => {}
                        Err(e) => {
                            error!("Failed to write state file: {}", e);
                        }
                    },
                    Err(e) => {
                        error!("Failed to serialize vehicle status: {}", e);
                    }
                }
            }
            publish_vehicle_status().await;
            if command {
                let oper_time = 30i64;
                let oper_temp = vehicle_status.ac_temp;
                let vehicle_info_clone = vehicle_info.clone();
                let duration = Duration::from_secs(oper_time as u64 * 60);
                tokio::spawn(async move {
                    debug!("Starting pet mode loop");
                    while VEHICLE_STATUS.lock().await.petmode {
                        while VEHICLE_STATUS.lock().await.ac_status {
                            debug!("AC is on, waiting for it to turn off");
                            update_vehicle_status().await;
                            time::sleep(Duration::from_secs(1)).await;
                        }
                        debug!("Turning on AC");
                        *DEBOUNCE_AC.lock().await = "on".to_string();
                        match send_climate_command(
                            &vehicle_info_clone.vin,
                            "1",
                            oper_time,
                            oper_temp,
                        )
                        .await
                        {
                            Ok(_) => {
                                info!("Aircond command sent");
                            }
                            Err(e) => {
                                error!("Failed to send aircond command: {}", e);
                                *DEBOUNCE_AC.lock().await = "".to_string();
                            }
                        }
                        time::sleep(duration).await;
                    }
                    debug!("Pet mode loop ended");
                });
            } else {
                debug!("Stopping pet mode loop");
                let oper_time = 30i64;
                let oper_temp = vehicle_status.ac_temp;
                let vehicle_info_clone = vehicle_info.clone();
                *DEBOUNCE_AC.lock().await = "off".to_string();
                tokio::spawn(async move {
                    match send_climate_command(&vehicle_info_clone.vin, "0", oper_time, oper_temp)
                        .await
                    {
                        Ok(_) => {
                            info!("Aircond command sent");
                        }
                        Err(e) => {
                            error!("Failed to send aircond command: {}", e);
                            *DEBOUNCE_AC.lock().await = "".to_string();
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

async fn update_vehicle_status() {
    info!("Updating vehicle status");
    let (vin, model_id) = {
        let vehicle_info = VEHICLE_INFO.lock().await;
        (vehicle_info.vin.clone(), vehicle_info.model_id)
    };
    match get_vehicle_status(&vin, model_id).await {
        Ok(mut vehicle_status) => {
            let mut global_vehicle_status = VEHICLE_STATUS.lock().await;
            vehicle_status.ac_time = global_vehicle_status.ac_time;
            vehicle_status.ac_temp = global_vehicle_status.ac_temp;
            vehicle_status.petmode = global_vehicle_status.petmode;
            *global_vehicle_status = vehicle_status.clone();
            match serde_json::to_string(&vehicle_status) {
                Ok(state_str) => match fs::write(STATE_FILE, state_str) {
                    Ok(_) => {}
                    Err(e) => {
                        error!("Failed to write state file: {}", e);
                    }
                },
                Err(e) => {
                    error!("Failed to serialize vehicle status: {}", e);
                }
            }
            info!("Vehicle status updated");
            debug!("Vehicle status: {:?}", vehicle_status);
            *MQTT_PUBLISH.lock().await = true;
        }
        Err(e) => {
            error!("Failed to get vehicle status {}", e);
        }
    };
}

async fn publish_vehicle_status() {
    let vehicle_info = VEHICLE_INFO.lock().await.clone();
    let brand_name = vehicle_info.brand_name.clone().to_lowercase();
    let last_vin = vehicle_info.showed_vin.get(15..).unwrap_or("").to_string();
    let topic_prefix = format!("{}/{}_{}", &*MQTT_CONFIG.topic_prefix, brand_name, last_vin);
    let mut vehicle_status = VEHICLE_STATUS.lock().await.clone();
    if *DEBOUNCE_LOCK.lock().await == "lock" {
        if !vehicle_status.unlock_status {
            *DEBOUNCE_LOCK.lock().await = "".to_string();
        } else {
            vehicle_status.unlock_status = false;
        }
    } else if *DEBOUNCE_LOCK.lock().await == "unlock" {
        if vehicle_status.unlock_status {
            *DEBOUNCE_LOCK.lock().await = "".to_string();
        } else {
            vehicle_status.unlock_status = true;
        }
    } else {
        *DEBOUNCE_LOCK.lock().await = "".to_string();
    }
    if *DEBOUNCE_AC.lock().await == "on" {
        if vehicle_status.ac_status {
            *DEBOUNCE_AC.lock().await = "".to_string();
        } else {
            vehicle_status.ac_status = true;
        }
    } else if *DEBOUNCE_AC.lock().await == "off" {
        if !vehicle_status.ac_status {
            *DEBOUNCE_AC.lock().await = "".to_string();
        } else {
            vehicle_status.ac_status = false;
        }
    } else {
        *DEBOUNCE_AC.lock().await = "".to_string();
    }
    let mutex_client = MQTT_CLIENT.lock().await;
    if let Some(client) = mutex_client.as_ref() {
        match publish_state(client, &vehicle_info, &vehicle_status, &topic_prefix).await {
            Ok(_) => {
                info!("State published");
            }
            Err(e) => {
                error!("Failed to publish state: {}", e);
            }
        };
    } else {
        error!("MQTT client is not initialized, skipping state publish");
    }
}

async fn publish_availability(topic: HashMap<&str, String>) {
    let vehicle_info = VEHICLE_INFO.lock().await.clone();
    let brand_name = vehicle_info.brand_name.clone().to_lowercase();
    let last_vin = vehicle_info.showed_vin.get(15..).unwrap_or("").to_string();
    let topic_prefix = format!("{}/{}_{}", &*MQTT_CONFIG.topic_prefix, brand_name, last_vin);
    {
        let mutex_client = MQTT_CLIENT.lock().await;
        let client = mutex_client.as_ref();
        if let Some(client) = client {
            for (topic, availability) in topic {
                match client
                    .publish(
                        format!("{}/{}/availability", topic_prefix, topic),
                        QOS,
                        true,
                        availability,
                    )
                    .await
                {
                    Ok(_) => {
                        info!("Availability published");
                    }
                    Err(e) => {
                        error!("Failed to publish availability: {}", e);
                    }
                }
            }
        } else {
            error!("MQTT client is not initialized, skipping availability publish");
        }
    }
}

async fn read_vehicle_configuration() -> Result<VehicleInfo, ()> {
    match fs::read_to_string(CONFIG_FILE) {
        Ok(content) => match serde_json::from_str::<VehicleInfo>(&content) {
            Ok(vehicle_info) => Ok(vehicle_info),
            Err(e) => {
                error!("Failed to parse configuration file: {}", e);
                Err(())
            }
        },
        Err(e) => {
            error!("Failed to read configuration file: {}", e);
            Err(())
        }
    }
}
