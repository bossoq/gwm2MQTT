#![recursion_limit = "256"]
mod gwm;
mod mqtt;

use gwm::{check_token, get_vehicle_status, get_vehicles, send_climate_command, send_lock_command};
use gwm::{VehicleInfo, VehicleStatus};
use log::{debug, error, info};
use mqtt::{
    parse_payload, publish_discovery, publish_state, setup as mqtt_setup, MQTT_CONFIG, QOS,
};
use regex::Regex;
use serde_json;
use std::{env::var, fs, sync::LazyLock, time::Duration};
use tokio::{sync::Mutex, time};

const CONFIG_FILE: &str = "data/configuration.json";
const STATE_FILE: &str = "data/state.json";
static REFRESH_INTERVAL: LazyLock<u64> = LazyLock::new(|| {
    var("REFRESH_INTERVAL")
        .unwrap_or("10".to_string())
        .parse()
        .unwrap_or(10)
});
static VEHICLE_VIN: LazyLock<String> =
    LazyLock::new(|| var("VEHICLE_VIN").unwrap_or("".to_string()));
static VEHICLE_INFO: LazyLock<Mutex<VehicleInfo>> =
    LazyLock::new(|| Mutex::new(VehicleInfo::default()));
static VEHICLE_STATUS: LazyLock<Mutex<VehicleStatus>> = LazyLock::new(|| {
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
static DISCOVERY_PUBLISH: LazyLock<Mutex<bool>> = LazyLock::new(|| Mutex::new(false));
static MQTT_PUBLISH: LazyLock<Mutex<bool>> = LazyLock::new(|| Mutex::new(false));

#[tokio::main]
pub async fn run() {
    let (client, mut eventloop) = mqtt_setup().await.unwrap();
    let client_mutex = Mutex::new(client);
    setup().await;
    let vehicle_info = VEHICLE_INFO.lock().await.clone();
    let brand_name = vehicle_info.brand_name.clone().to_lowercase();
    let last_vin = vehicle_info.showed_vin[15..].to_string();
    let topic = format!("{}/{}_{}", &*MQTT_CONFIG.topic_prefix, brand_name, last_vin);
    let client_lock = client_mutex.lock().await;
    client_lock
        .subscribe(format!("{}/{}/set", topic, "lock".to_string()), QOS)
        .await
        .unwrap();
    client_lock
        .subscribe(format!("{}/{}/set", topic, "aircond".to_string()), QOS)
        .await
        .unwrap();
    client_lock
        .subscribe(format!("{}/{}/set", topic, "petmode".to_string()), QOS)
        .await
        .unwrap();
    client_lock
        .subscribe(format!("{}/{}/set", topic, "ac_timer".to_string()), QOS)
        .await
        .unwrap();
    client_lock
        .subscribe(
            format!("{}/{}/set", topic, "ac_temperature".to_string()),
            QOS,
        )
        .await
        .unwrap();
    drop(client_lock);

    tokio::select! {
        _ = async {
            let mut client_lock = client_mutex.lock().await;
            let topic = format!("{}/{}_{}", &*MQTT_CONFIG.topic_prefix, brand_name, last_vin);
            loop {
                if !*DISCOVERY_PUBLISH.lock().await {
                    match publish_discovery(&mut client_lock, &vehicle_info, &topic).await {
                        Ok(_) => {
                            info!("Discovery published");
                            *DISCOVERY_PUBLISH.lock().await = true;
                        }
                        Err(e) => {
                            error!("Failed to publish discovery: {}", e);
                        }
                    }
                }
                if *MQTT_PUBLISH.lock().await {
                    match publish_state(&mut client_lock, &vehicle_info, &VEHICLE_STATUS.lock().await.clone(), &topic.clone()).await {
                        Ok(_) => {
                            info!("State published");
                            *MQTT_PUBLISH.lock().await = false;
                        }
                        Err(e) => {
                            error!("Failed to publish state: {}", e);
                        }
                    };
                }
            }
        } => {}
        _ = async {
            loop {
                update_vehicle_status().await;
            }
        } => {}
        _ = async {
            let topic_prefix = format!("{}/{}_{}", &*MQTT_CONFIG.topic_prefix, brand_name, last_vin);
            loop {
                match eventloop.poll().await {
                    Ok(notification) => {
                        let payload = parse_payload(notification).await.unwrap();
                        if payload.is_null() {
                            continue;
                        }
                        let topic = payload["topic"].as_str().unwrap_or("");
                        let re = Regex::new(format!(r"^{}\/(?<topic>\w+)\/set$", topic_prefix).as_str()).unwrap();
                        let captures = re.captures(topic);
                        if captures.is_none() {
                            continue;
                        }
                        let captures = captures.unwrap();
                        let capture_topic = captures.name("topic");
                        if capture_topic.is_none() {
                            continue;
                        }
                        let capture_topic = capture_topic.unwrap().as_str();
                        let vehicle_info = VEHICLE_INFO.lock().await.clone();
                        match capture_topic {
                            "lock" => {
                                let command = payload["payload"].as_str().unwrap_or("");
                                let command = if command == "UNLOCK" {
                                    "1"
                                } else if command == "LOCK" {
                                    "0"
                                } else {
                                    continue;
                                };
                                tokio::spawn(async move {
                                    match send_lock_command(&vehicle_info.vin, command).await {
                                        Ok(_) => {
                                            info!("Lock command sent");
                                            update_vehicle_status().await;
                                        }
                                        Err(e) => {
                                            error!("Failed to send lock command: {}", e);
                                        }
                                    }
                                });
                            }
                            "aircond" => {
                                let command = payload["payload"].as_str().unwrap_or("");
                                let command = if command == "ON" {
                                    "1"
                                } else if command == "OFF" {
                                    "0"
                                } else {
                                    continue;
                                };
                                let oper_time = VEHICLE_STATUS.lock().await.ac_time;
                                let oper_temp = VEHICLE_STATUS.lock().await.ac_temp;
                                tokio::spawn(async move {
                                    match send_climate_command(&vehicle_info.vin, command, oper_time, oper_temp).await {
                                        Ok(_) => {
                                            info!("Aircond command sent");
                                            update_vehicle_status().await;
                                        }
                                        Err(e) => {
                                            error!("Failed to send aircond command: {}", e);
                                        }
                                    }
                                });
                            }
                            "ac_timer" => {
                                println!("{:?}", payload);
                                let payload_str = payload["payload"].as_str();
                                if payload_str.is_none() {
                                    continue
                                }
                                let payload_i64 = payload_str.unwrap().parse::<i64>();
                                match payload_i64 {
                                    Ok(oper_time) => {
                                        info!("Setting aircond timer to {}", oper_time);
                                        let mut vehicle_status = VEHICLE_STATUS.lock().await;
                                        vehicle_status.ac_time = oper_time;
                                        fs::write(STATE_FILE, serde_json::to_string(&*vehicle_status).unwrap()).unwrap();
                                    }
                                    Err(_) => {
                                        error!("Invalid payload");
                                    }
                                }
                            }
                            "ac_temperature" => {
                                let payload_str = payload["payload"].as_str();
                                if payload_str.is_none() {
                                    continue
                                }
                                let payload_i64 = payload_str.unwrap().parse::<i64>();
                                match payload_i64 {
                                    Ok(oper_temp) => {
                                        info!("Setting temperature to {}", oper_temp);
                                        let mut vehicle_status = VEHICLE_STATUS.lock().await;
                                        vehicle_status.ac_temp = oper_temp;
                                        fs::write(STATE_FILE, serde_json::to_string(&*vehicle_status).unwrap()).unwrap();
                                    }
                                    Err(_) => {
                                        error!("Invalid payload");
                                    }
                                }
                            }
                            // petmode_topic => {
                            //     let mut vehicle_status = VEHICLE_STATUS.lock().await;
                            //     vehicle_status.pet_mode = payload["payload"].as_bool().unwrap_or(false);
                            //     fs::write(STATE_FILE, serde_json::to_string(&*vehicle_status).unwrap()).unwrap();
                            // }
                            _ => {
                                error!("Unknown topic: {:?}", payload["topic"]);
                            }
                        }
                    }
                    Err(e) => {
                        error!("Error: {:?}", e);
                    }
                }
            }
        } => {}
    }
}

async fn setup() {
    let (token_valid, _) = check_token().await;
    if !token_valid {
        panic!("Token is invalid");
    }
    let env_vin = &VEHICLE_VIN;
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
                if vehicle_info.showed_vin.starts_with(&env_vin[..3])
                    && vehicle_info
                        .showed_vin
                        .ends_with(&env_vin[env_vin.len() - 4..])
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
            Err(_) => {
                panic!("Failed to get vehicle list");
            }
        };
        if vehicle_infos.is_empty() {
            panic!("No vehicle found");
        }
        if env_vin.is_empty() {
            info!("VEHICLE_VIN is not set, using the first vehicle in the list");
            vehicle_info = vehicle_infos[0].clone();
        } else {
            info!(
                "VEHICLE_VIN is set, using the vehicle with the VIN from the environment variable"
            );
            for vehicle in vehicle_infos {
                if vehicle.showed_vin.starts_with(&env_vin[..3])
                    && vehicle.showed_vin.ends_with(&env_vin[env_vin.len() - 4..])
                {
                    vehicle_info = vehicle;
                    break;
                }
            }
            if vehicle_info.vin.is_empty() {
                panic!("Vehicle with the VIN from the environment variable not found");
            }
        }
    }
    fs::write(CONFIG_FILE, serde_json::to_string(&vehicle_info).unwrap()).unwrap();
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
        fs::write(STATE_FILE, serde_json::to_string(&*vehicle_status).unwrap()).unwrap();
    }
    *MQTT_PUBLISH.lock().await = true;
}

async fn update_vehicle_status() {
    info!("Updating vehicle status");
    let vehicle_info = VEHICLE_INFO.lock().await;
    match get_vehicle_status(&vehicle_info.vin, vehicle_info.model_id).await {
        Ok(mut vehicle_status) => {
            let mut global_vehicle_status = VEHICLE_STATUS.lock().await;
            vehicle_status.ac_time = global_vehicle_status.ac_time;
            vehicle_status.ac_temp = global_vehicle_status.ac_temp;
            *global_vehicle_status = vehicle_status.clone();
            fs::write(STATE_FILE, serde_json::to_string(&vehicle_status).unwrap()).unwrap();
            info!("Vehicle status updated");
            debug!("Vehicle status: {:?}", vehicle_status);
            *MQTT_PUBLISH.lock().await = true;
        }
        Err(e) => {
            error!("Failed to get vehicle status {}", e);
        }
    };
    time::sleep(Duration::from_secs(*REFRESH_INTERVAL)).await;
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
