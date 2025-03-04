use log::{debug, error, info};
use rand::prelude::*;
use rumqttc::{AsyncClient, Event, EventLoop, MqttOptions, QoS};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{collections::HashMap, env::var, fs, sync::LazyLock, time::Duration};

use crate::gwm;

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MQTTConfiguration {
    broker: String,
    port: u16,
    user: String,
    password: String,
    discovery_topic: String,
    pub topic_prefix: String,
}

const MQTT_CONFIG_FILE: &str = "data/mqtt.json";
pub static MQTT_CONFIG: LazyLock<MQTTConfiguration> = LazyLock::new(|| {
    let mqtt_broker = var("MQTT_BROKER");
    let mqtt_port = var("MQTT_PORT");
    let mqtt_user = var("MQTT_USER");
    let mqtt_password = var("MQTT_PASSWORD");
    let mqtt_discovery_topic = var("MQTT_DISCOVERY_TOPIC");
    let mqtt_topic_prefix = var("MQTT_TOPIC_PREFIX");
    if mqtt_broker.is_ok() && mqtt_port.is_ok() {
        let mqtt_config = MQTTConfiguration {
            broker: mqtt_broker.unwrap(),
            port: mqtt_port.unwrap().parse().unwrap_or(1883),
            user: mqtt_user.unwrap_or("".to_string()),
            password: mqtt_password.unwrap_or("".to_string()),
            discovery_topic: mqtt_discovery_topic.unwrap_or("homeassistant".to_string()),
            topic_prefix: mqtt_topic_prefix.unwrap_or("gwm2mqtt".to_string()),
        };
        fs::write(
            MQTT_CONFIG_FILE,
            serde_json::to_string(&mqtt_config).unwrap(),
        )
        .unwrap();
        mqtt_config
    } else {
        match fs::read_to_string(MQTT_CONFIG_FILE) {
            Ok(content) => match serde_json::from_str::<MQTTConfiguration>(&content) {
                Ok(config) => config,
                Err(e) => {
                    error!("Failed to parse MQTT configuration file: {}", e);
                    MQTTConfiguration {
                        broker: "localhost".to_string(),
                        port: 1883,
                        user: "".to_string(),
                        password: "".to_string(),
                        discovery_topic: "homeassistant".to_string(),
                        topic_prefix: "gwm2mqtt".to_string(),
                    }
                }
            },
            Err(e) => {
                error!("Failed to read MQTT configuration file: {}", e);
                MQTTConfiguration {
                    broker: "localhost".to_string(),
                    port: 1883,
                    user: "".to_string(),
                    password: "".to_string(),
                    discovery_topic: "homeassistant".to_string(),
                    topic_prefix: "gwm2mqtt".to_string(),
                }
            }
        }
    }
});

pub const QOS: QoS = QoS::AtMostOnce;

pub async fn setup() -> Result<(AsyncClient, EventLoop), Box<dyn std::error::Error>> {
    info!("Setting up MQTT client");
    // randomize the client id 6 alphanumeric characters
    let id: String = rand::rng()
        .sample_iter(rand::distr::Alphanumeric)
        .take(6)
        .map(char::from)
        .collect();
    let id = format!("gwm2mqtt-{}", id);
    let mut mqttoptions = MqttOptions::new(id, &*MQTT_CONFIG.broker, MQTT_CONFIG.port);
    mqttoptions.set_keep_alive(Duration::from_secs(10));
    mqttoptions.set_credentials(&*MQTT_CONFIG.user, &*MQTT_CONFIG.password);

    let (client, eventloop) = AsyncClient::new(mqttoptions, 10);
    info!("Connected to MQTT broker");
    Ok((client, eventloop))
}

pub async fn parse_payload(event: Event) -> Result<Value, Box<dyn std::error::Error>> {
    match event {
        Event::Incoming(incoming) => match incoming {
            rumqttc::Packet::Publish(publish) => {
                debug!("Received message on topic: {}", publish.topic);
                debug!("Payload: {:?}", publish.payload);
                let payload = String::from_utf8_lossy(&publish.payload);
                let value: Value = json!({
                    "topic": publish.topic,
                    "payload": payload,
                });
                Ok(value)
            }
            _ => Ok(Value::Null),
        },
        _ => Ok(Value::Null),
    }
}

pub async fn publish_discovery(
    client: &AsyncClient,
    vehicle_info: &gwm::VehicleInfo,
    topic_prefix: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    info!("Publishing discovery");
    let name = format!(
        "{}_{}",
        vehicle_info.brand_name.clone(),
        vehicle_info.showed_vin[15..].to_string()
    );
    let prefix = format!(
        "{}_{}",
        vehicle_info.showed_vin[..3].to_string(),
        vehicle_info.showed_vin[15..].to_string()
    );
    let device = json!({
        "ids": name,
        "name": name,
        "sw": "GWM2MQTT 1.0",
        "mdl": vehicle_info.vtype.clone(),
        "mf": vehicle_info.brand_name.clone(),
        "sn": vehicle_info.showed_vin.clone(),
    });
    let availability_topic = format!("{}/availability", topic_prefix);
    let availability = json!(
        [{"topic": availability_topic.clone(), "value_template": "{{ value }}"}]
    );
    let attributes_topic = format!("{}/attributes", topic_prefix);
    let mut payload_map: HashMap<String, String> = HashMap::new();
    payload_map.insert(availability_topic.clone(), "online".to_string());
    payload_map.insert(
        format!(
            "{}/sensor/{}_{}/config",
            &*MQTT_CONFIG.discovery_topic, &prefix, "total_mileage"
        ),
        json!({
            "availability": availability,
            "name": format!("{} Mileage", vehicle_info.showed_vin),
            "unique_id": format!("{}_total_mileage", prefix),
            "icon": "mdi:speedometer",
            "unit_of_measurement": "km",
            "device_class": "distance",
            "state_class": "total_increasing",
            "state_topic": format!("{}/state", topic_prefix),
            "value_template": "{{ value_json.mileage if (value_json is defined and value_json.mileage is defined and value_json.mileage|int >= 0) else '0' }}",
            "json_attributes_topic": attributes_topic.clone(),
            "json_attributes_template": "{{ value }}",
            "device": device,
        })
        .to_string()
    );
    payload_map.insert(
        format!(
            "{}/sensor/{}_{}/config",
            &*MQTT_CONFIG.discovery_topic, &prefix, "charge"
        ),
        json!({
            "availability": availability,
            "name": format!("{} Charge", vehicle_info.showed_vin),
            "unique_id": format!("{}_charge", prefix),
            "icon": "mdi:battery",
            "unit_of_measurement": "%",
            "device_class": "battery",
            "state_class": "measurement",
            "state_topic": format!("{}/state", topic_prefix),
            "value_template": "{{ value_json.soc if (value_json is defined and value_json.soc is defined and value_json.soc|int >= 0 and value_json.soc|int <= 100) else '0' }}",
            "json_attributes_topic": attributes_topic.clone(),
            "json_attributes_template": "{{ value }}",
            "device": device,
        })
        .to_string()
    );
    payload_map.insert(
        format!(
            "{}/sensor/{}_{}/config",
            &*MQTT_CONFIG.discovery_topic, &prefix, "range"
        ),
        json!({
            "availability": availability,
            "name": format!("{} Range", vehicle_info.showed_vin),
            "unique_id": format!("{}_range", prefix),
            "icon": "mdi:speedometer",
            "unit_of_measurement": "km",
            "device_class": "distance",
            "state_class": "measurement",
            "state_topic": format!("{}/state", topic_prefix),
            "value_template": "{{ value_json.range if (value_json is defined and value_json.range is defined and value_json.range|int >= 0) else '0' }}",
            "json_attributes_topic": attributes_topic.clone(),
            "json_attributes_template": "{{ value }}",
            "device": device,
        })
        .to_string()
    );
    payload_map.insert(
        format!(
            "{}/sensor/{}_{}/config",
            &*MQTT_CONFIG.discovery_topic, &prefix, "charge_time"
        ),
        json!({
            "availability": availability,
            "name": format!("{} Charge Time", vehicle_info.showed_vin),
            "unique_id": format!("{}_charge_time", prefix),
            "icon": "mdi:timer",
            "unit_of_measurement": "min",
            "device_class": "duration",
            "state_class": "measurement",
            "state_topic": format!("{}/state", topic_prefix),
            "value_template": "{{ value_json.charge_time if (value_json is defined and value_json.charge_time is defined and value_json.charge_time|int >= 0 and value_json.charging_status is defined and value_json.charging_status) else '0' }}",
            "json_attributes_topic": attributes_topic.clone(),
            "json_attributes_template": "{{ value }}",
            "device": device,
        })
        .to_string()
    );
    payload_map.insert(
        format!(
            "{}/sensor/{}_{}/config",
            &*MQTT_CONFIG.discovery_topic, &prefix, "charge_status"
        ),
        json!({
            "availability": availability,
            "name": format!("{} Charge Status", vehicle_info.showed_vin),
            "unique_id": format!("{}_charge_status", prefix),
            "icon": "mdi:ev-plug-type2",
            "device_class": "enum",
            "state_topic": format!("{}/state", topic_prefix),
            "value_template": "{{ value_json.charging_status_desc if (value_json is defined and value_json.charging_status_desc is defined) else 'Unknown' }}",
            "options": ["Unknown", "Not Charging", "Charging", "Awaiting Charge", "Finish Charge", "Unknown 4", "Unknown 5", "Error"],
            "json_attributes_topic": attributes_topic.clone(),
            "json_attributes_template": "{{ value }}",
            "device": device,
        })
        .to_string()
    );
    payload_map.insert(
        format!(
            "{}/sensor/{}_{}/config",
            &*MQTT_CONFIG.discovery_topic, &prefix, "fl_tire_pressure"
        ),
        json!({
            "availability": availability,
            "name": format!("{} FL Tire Pressure", vehicle_info.showed_vin),
            "unique_id": format!("{}_fl_tire_pressure", prefix),
            "icon": "mdi:tire",
            "unit_of_measurement": "psi",
            "device_class": "pressure",
            "state_class": "measurement",
            "state_topic": format!("{}/state", topic_prefix),
            "value_template": "{{ value_json.fl_tire_pressure if (value_json is defined and value_json.fl_tire_pressure is defined and value_json.fl_tire_pressure|int >= 0) else '0.00' }}",
            "suggested_display_precision": 2,
            "json_attributes_topic": attributes_topic.clone(),
            "json_attributes_template": "{{ value }}",
            "device": device,
        })
        .to_string()
    );
    payload_map.insert(
        format!(
            "{}/sensor/{}_{}/config",
            &*MQTT_CONFIG.discovery_topic, &prefix, "fr_tire_pressure"
        ),
        json!({
            "availability": availability,
            "name": format!("{} FR Tire Pressure", vehicle_info.showed_vin),
            "unique_id": format!("{}_fr_tire_pressure", prefix),
            "icon": "mdi:tire",
            "unit_of_measurement": "psi",
            "device_class": "pressure",
            "state_class": "measurement",
            "state_topic": format!("{}/state", topic_prefix),
            "value_template": "{{ value_json.fr_tire_pressure if (value_json is defined and value_json.fr_tire_pressure is defined and value_json.fr_tire_pressure|int >= 0) else '0.00' }}",
            "suggested_display_precision": 2,
            "json_attributes_topic": attributes_topic.clone(),
            "json_attributes_template": "{{ value }}",
            "device": device,
        })
        .to_string()
    );
    payload_map.insert(
        format!(
            "{}/sensor/{}_{}/config",
            &*MQTT_CONFIG.discovery_topic, &prefix, "rl_tire_pressure"
        ),
        json!({
            "availability": availability,
            "name": format!("{} RL Tire Pressure", vehicle_info.showed_vin),
            "unique_id": format!("{}_rl_tire_pressure", prefix),
            "icon": "mdi:tire",
            "unit_of_measurement": "psi",
            "device_class": "pressure",
            "state_class": "measurement",
            "state_topic": format!("{}/state", topic_prefix),
            "value_template": "{{ value_json.rl_tire_pressure if (value_json is defined and value_json.rl_tire_pressure is defined and value_json.rl_tire_pressure|int >= 0) else '0.00' }}",
            "suggested_display_precision": 2,
            "json_attributes_topic": attributes_topic.clone(),
            "json_attributes_template": "{{ value }}",
            "device": device,
        })
        .to_string()
    );
    payload_map.insert(
        format!(
            "{}/sensor/{}_{}/config",
            &*MQTT_CONFIG.discovery_topic, &prefix, "rr_tire_pressure"
        ),
        json!({
            "availability": availability,
            "name": format!("{} RR Tire Pressure", vehicle_info.showed_vin),
            "unique_id": format!("{}_rr_tire_pressure", prefix),
            "icon": "mdi:tire",
            "unit_of_measurement": "psi",
            "device_class": "pressure",
            "state_class": "measurement",
            "state_topic": format!("{}/state", topic_prefix),
            "value_template": "{{ value_json.rr_tire_pressure if (value_json is defined and value_json.rr_tire_pressure is defined and value_json.rr_tire_pressure|int >= 0) else '0.00' }}",
            "suggested_display_precision": 2,
            "json_attributes_topic": attributes_topic.clone(),
            "json_attributes_template": "{{ value }}",
            "device": device,
        })
        .to_string()
    );
    payload_map.insert(
        format!(
            "{}/sensor/{}_{}/config",
            &*MQTT_CONFIG.discovery_topic, &prefix, "fl_tire_temperature"
        ),
        json!({
            "availability": availability,
            "name": format!("{} FL Tire Temperature", vehicle_info.showed_vin),
            "unique_id": format!("{}_fl_tire_temperature", prefix),
            "icon": "mdi:tire",
            "unit_of_measurement": "°C",
            "device_class": "temperature",
            "state_class": "measurement",
            "state_topic": format!("{}/state", topic_prefix),
            "value_template": "{{ value_json.fl_tire_temp if (value_json is defined and value_json.fl_tire_temp is defined and value_json.fl_tire_temp|int >= 0) else '0' }}",
            "json_attributes_topic": attributes_topic.clone(),
            "json_attributes_template": "{{ value }}",
            "device": device,
        })
        .to_string()
    );
    payload_map.insert(
        format!(
            "{}/sensor/{}_{}/config",
            &*MQTT_CONFIG.discovery_topic, &prefix, "fr_tire_temperature"
        ),
        json!({
            "availability": availability,
            "name": format!("{} FR Tire Temperature", vehicle_info.showed_vin),
            "unique_id": format!("{}_fr_tire_temperature", prefix),
            "icon": "mdi:tire",
            "unit_of_measurement": "°C",
            "device_class": "temperature",
            "state_class": "measurement",
            "state_topic": format!("{}/state", topic_prefix),
            "value_template": "{{ value_json.fr_tire_temp if (value_json is defined and value_json.fr_tire_temp is defined and value_json.fr_tire_temp|int >= 0) else '0' }}",
            "json_attributes_topic": attributes_topic.clone(),
            "json_attributes_template": "{{ value }}",
            "device": device,
        })
        .to_string()
    );
    payload_map.insert(
        format!(
            "{}/sensor/{}_{}/config",
            &*MQTT_CONFIG.discovery_topic, &prefix, "rl_tire_temperature"
        ),
        json!({
            "availability": availability,
            "name": format!("{} RL Tire Temperature", vehicle_info.showed_vin),
            "unique_id": format!("{}_rl_tire_temperature", prefix),
            "icon": "mdi:tire",
            "unit_of_measurement": "°C",
            "device_class": "temperature",
            "state_class": "measurement",
            "state_topic": format!("{}/state", topic_prefix),
            "value_template": "{{ value_json.rl_tire_temp if (value_json is defined and value_json.rl_tire_temp is defined and value_json.rl_tire_temp|int >= 0) else '0' }}",
            "json_attributes_topic": attributes_topic.clone(),
            "json_attributes_template": "{{ value }}",
            "device": device,
        })
        .to_string()
    );
    payload_map.insert(
        format!(
            "{}/sensor/{}_{}/config",
            &*MQTT_CONFIG.discovery_topic, &prefix, "rr_tire_temperature"
        ),
        json!({
            "availability": availability,
            "name": format!("{} RR Tire Temperature", vehicle_info.showed_vin),
            "unique_id": format!("{}_rr_tire_temperature", prefix),
            "icon": "mdi:tire",
            "unit_of_measurement": "°C",
            "device_class": "temperature",
            "state_class": "measurement",
            "state_topic": format!("{}/state", topic_prefix),
            "value_template": "{{ value_json.rr_tire_temp if (value_json is defined and value_json.rr_tire_temp is defined and value_json.rr_tire_temp|int >= 0) else '0' }}",
            "json_attributes_topic": attributes_topic.clone(),
            "json_attributes_template": "{{ value }}",
            "device": device,
        })
        .to_string()
    );
    payload_map.insert(
        format!(
            "{}/device_tracker/{}_{}/config",
            &*MQTT_CONFIG.discovery_topic, &prefix, "tracker"
        ),
        json!({
            "availability": availability,
            "name": format!("{} Tracker", vehicle_info.showed_vin),
            "unique_id": format!("{}_tracker", prefix),
            "icon": "mdi:car",
            "platform": "device_tracker",
            "source_type": "gps",
            "json_attributes_topic": format!("{}/tracker/attributes", topic_prefix),
            "json_attributes_template": "{{ value }}",
            "device": device,
        })
        .to_string(),
    );
    payload_map.insert(
        format!(
            "{}/binary_sensor/{}_{}/config",
            &*MQTT_CONFIG.discovery_topic, &prefix, "charging_port_plugged"
        ),
        json!({
            "availability": availability,
            "name": format!("{} Charging Port Plugged", vehicle_info.showed_vin),
            "unique_id": format!("{}_charging_port_plugged", prefix),
            "icon": "mdi:power-plug",
            "device_class": "plug",
            "state_topic": format!("{}/state", topic_prefix),
            "value_template": "{{ value_json.charging_port_plugged if (value_json is defined and value_json.charging_port_plugged is defined) else 'OFF' }}",
            "json_attributes_topic": attributes_topic.clone(),
            "json_attributes_template": "{{ value }}",
            "device": device,
        })
        .to_string()
    );
    payload_map.insert(
        format!(
            "{}/binary_sensor/{}_{}/config",
            &*MQTT_CONFIG.discovery_topic, &prefix, "schedule_charge"
        ),
        json!({
            "availability": availability,
            "name": format!("{} Schedule Charge", vehicle_info.showed_vin),
            "unique_id": format!("{}_schedule_charge", prefix),
            "icon": "mdi:timer",
            "device_class": "power",
            "state_topic": format!("{}/state", topic_prefix),
            "value_template": "{{ value_json.schedule_charge if (value_json is defined and value_json.schedule_charge is defined) else 'OFF' }}",
            "json_attributes_topic": attributes_topic.clone(),
            "json_attributes_template": "{{ value }}",
            "device": device,
        })
        .to_string()
    );
    payload_map.insert(
        format!(
            "{}/binary_sensor/{}_{}/config",
            &*MQTT_CONFIG.discovery_topic, &prefix, "ac_status"
        ),
        json!({
            "availability": availability,
            "name": format!("{} AC Status", vehicle_info.showed_vin),
            "unique_id": format!("{}_ac_status", prefix),
            "icon": "mdi:air-conditioner",
            "device_class": "power",
            "state_topic": format!("{}/state", topic_prefix),
            "value_template": "{{ value_json.ac_status if (value_json is defined and value_json.ac_status is defined) else 'OFF' }}",
            "json_attributes_topic": attributes_topic.clone(),
            "json_attributes_template": "{{ value }}",
            "device": device,
        })
        .to_string()
    );
    payload_map.insert(
        format!(
            "{}/binary_sensor/{}_{}/config",
            &*MQTT_CONFIG.discovery_topic, &prefix, "air_filter_status"
        ),
        json!({
            "availability": availability,
            "name": format!("{} Air Filter Status", vehicle_info.showed_vin),
            "unique_id": format!("{}_air_filter_status", prefix),
            "icon": "mdi:air-purifier",
            "device_class": "power",
            "state_topic": format!("{}/state", topic_prefix),
            "value_template": "{{ value_json.air_filter_status if (value_json is defined and value_json.air_filter_status is defined) else 'OFF' }}",
            "json_attributes_topic": attributes_topic.clone(),
            "json_attributes_template": "{{ value }}",
            "device": device,
        })
        .to_string()
    );
    payload_map.insert(
        format!(
            "{}/binary_sensor/{}_{}/config",
            &*MQTT_CONFIG.discovery_topic, &prefix, "unlock_status"
        ),
        json!({
            "availability": availability,
            "name": format!("{} Locked Status", vehicle_info.showed_vin),
            "unique_id": format!("{}_unlock_status", prefix),
            "icon": "mdi:lock",
            "device_class": "lock",
            "state_topic": format!("{}/state", topic_prefix),
            "value_template": "{{ value_json.unlock_status if (value_json is defined and value_json.unlock_status is defined) else 'OFF' }}",
            "json_attributes_topic": attributes_topic.clone(),
            "json_attributes_template": "{{ value }}",
            "device": device,
        })
        .to_string()
    );
    payload_map.insert(
        format!(
            "{}/binary_sensor/{}_{}/config",
            &*MQTT_CONFIG.discovery_topic, &prefix, "fl_door_open"
        ),
        json!({
            "availability": availability,
            "name": format!("{} FL Door Open Status", vehicle_info.showed_vin),
            "unique_id": format!("{}_fl_door_open", prefix),
            "icon": "mdi:door",
            "device_class": "door",
            "state_topic": format!("{}/state", topic_prefix),
            "value_template": "{{ value_json.fl_door_open if (value_json is defined and value_json.fl_door_open is defined) else 'OFF' }}",
            "json_attributes_topic": attributes_topic.clone(),
            "json_attributes_template": "{{ value }}",
            "device": device,
        })
        .to_string()
    );
    payload_map.insert(
        format!(
            "{}/binary_sensor/{}_{}/config",
            &*MQTT_CONFIG.discovery_topic, &prefix, "fr_door_open"
        ),
        json!({
            "availability": availability,
            "name": format!("{} FR Door Open Status", vehicle_info.showed_vin),
            "unique_id": format!("{}_fr_door_open", prefix),
            "icon": "mdi:door",
            "device_class": "door",
            "state_topic": format!("{}/state", topic_prefix),
            "value_template": "{{ value_json.fr_door_open if (value_json is defined and value_json.fr_door_open is defined) else 'OFF' }}",
            "json_attributes_topic": attributes_topic.clone(),
            "json_attributes_template": "{{ value }}",
            "device": device,
        })
        .to_string()
    );
    payload_map.insert(
        format!(
            "{}/binary_sensor/{}_{}/config",
            &*MQTT_CONFIG.discovery_topic, &prefix, "rl_door_open"
        ),
        json!({
            "availability": availability,
            "name": format!("{} RL Door Open Status", vehicle_info.showed_vin),
            "unique_id": format!("{}_rl_door_open", prefix),
            "icon": "mdi:door",
            "device_class": "door",
            "state_topic": format!("{}/state", topic_prefix),
            "value_template": "{{ value_json.rl_door_open if (value_json is defined and value_json.rl_door_open is defined) else 'OFF' }}",
            "json_attributes_topic": attributes_topic.clone(),
            "json_attributes_template": "{{ value }}",
            "device": device,
        })
        .to_string()
    );
    payload_map.insert(
        format!(
            "{}/binary_sensor/{}_{}/config",
            &*MQTT_CONFIG.discovery_topic, &prefix, "rr_door_open"
        ),
        json!({
            "availability": availability,
            "name": format!("{} RR Door Open Status", vehicle_info.showed_vin),
            "unique_id": format!("{}_rr_door_open", prefix),
            "icon": "mdi:door",
            "device_class": "door",
            "state_topic": format!("{}/state", topic_prefix),
            "value_template": "{{ value_json.rr_door_open if (value_json is defined and value_json.rr_door_open is defined) else 'OFF' }}",
            "json_attributes_topic": attributes_topic.clone(),
            "json_attributes_template": "{{ value }}",
            "device": device,
        })
        .to_string()
    );
    payload_map.insert(
        format!(
            "{}/binary_sensor/{}_{}/config",
            &*MQTT_CONFIG.discovery_topic, &prefix, "trunk_open"
        ),
        json!({
            "availability": availability,
            "name": format!("{} Trunk Open Status", vehicle_info.showed_vin),
            "unique_id": format!("{}_trunk_open", prefix),
            "icon": "mdi:door",
            "device_class": "door",
            "state_topic": format!("{}/state", topic_prefix),
            "value_template": "{{ value_json.trunk_open if (value_json is defined and value_json.trunk_open is defined) else 'OFF' }}",
            "json_attributes_topic": attributes_topic.clone(),
            "json_attributes_template": "{{ value }}",
            "device": device,
        })
        .to_string()
    );
    payload_map.insert(
        format!(
            "{}/binary_sensor/{}_{}/config",
            &*MQTT_CONFIG.discovery_topic, &prefix, "fl_window_open"
        ),
        json!({
            "availability": availability,
            "name": format!("{} FL Window Open Status", vehicle_info.showed_vin),
            "unique_id": format!("{}_fl_window_open", prefix),
            "icon": "mdi:window-open",
            "device_class": "window",
            "state_topic": format!("{}/state", topic_prefix),
            "value_template": "{{ value_json.fl_window_open if (value_json is defined and value_json.fl_window_open is defined) else 'OFF' }}",
            "json_attributes_topic": attributes_topic.clone(),
            "json_attributes_template": "{{ value }}",
            "device": device,
        })
        .to_string()
    );
    payload_map.insert(
        format!(
            "{}/binary_sensor/{}_{}/config",
            &*MQTT_CONFIG.discovery_topic, &prefix, "fr_window_open"
        ),
        json!({
            "availability": availability,
            "name": format!("{} FR Window Open Status", vehicle_info.showed_vin),
            "unique_id": format!("{}_fr_window_open", prefix),
            "icon": "mdi:window-open",
            "device_class": "window",
            "state_topic": format!("{}/state", topic_prefix),
            "value_template": "{{ value_json.fr_window_open if (value_json is defined and value_json.fr_window_open is defined) else 'OFF' }}",
            "json_attributes_topic": attributes_topic.clone(),
            "json_attributes_template": "{{ value }}",
            "device": device,
        })
        .to_string()
    );
    payload_map.insert(
        format!(
            "{}/binary_sensor/{}_{}/config",
            &*MQTT_CONFIG.discovery_topic, &prefix, "rl_window_open"
        ),
        json!({
            "availability": availability,
            "name": format!("{} RL Window Open Status", vehicle_info.showed_vin),
            "unique_id": format!("{}_rl_window_open", prefix),
            "icon": "mdi:window-open",
            "device_class": "window",
            "state_topic": format!("{}/state", topic_prefix),
            "value_template": "{{ value_json.rl_window_open if (value_json is defined and value_json.rl_window_open is defined) else 'OFF' }}",
            "json_attributes_topic": attributes_topic.clone(),
            "json_attributes_template": "{{ value }}",
            "device": device,
        })
        .to_string()
    );
    payload_map.insert(
        format!(
            "{}/binary_sensor/{}_{}/config",
            &*MQTT_CONFIG.discovery_topic, &prefix, "rr_window_open"
        ),
        json!({
            "availability": availability,
            "name": format!("{} RR Window Open Status", vehicle_info.showed_vin),
            "unique_id": format!("{}_rr_window_open", prefix),
            "icon": "mdi:window-open",
            "device_class": "window",
            "state_topic": format!("{}/state", topic_prefix),
            "value_template": "{{ value_json.rr_window_open if (value_json is defined and value_json.rr_window_open is defined) else 'OFF' }}",
            "json_attributes_topic": attributes_topic.clone(),
            "json_attributes_template": "{{ value }}",
            "device": device,
        })
        .to_string()
    );
    payload_map.insert(
        format!(
            "{}/binary_sensor/{}_{}/config",
            &*MQTT_CONFIG.discovery_topic, &prefix, "sunroof_open"
        ),
        json!({
            "availability": availability,
            "name": format!("{} Sunroof Open Status", vehicle_info.showed_vin),
            "unique_id": format!("{}_sunroof_open", prefix),
            "icon": "mdi:window-open",
            "device_class": "window",
            "state_topic": format!("{}/state", topic_prefix),
            "value_template": "{{ value_json.sunroof_open if (value_json is defined and value_json.sunroof_open is defined) else 'OFF' }}",
            "json_attributes_topic": attributes_topic.clone(),
            "json_attributes_template": "{{ value }}",
            "device": device,
        })
        .to_string()
    );
    payload_map.insert(
        format!(
            "{}/binary_sensor/{}_{}/config",
            &*MQTT_CONFIG.discovery_topic, &prefix, "fl_tire_pressure_alarm"
        ),
        json!({
            "availability": availability,
            "name": format!("{} FL Tire Pressure Alarm", vehicle_info.showed_vin),
            "unique_id": format!("{}_fl_tire_pressure_alarm", prefix),
            "icon": "mdi:tire",
            "device_class": "problem",
            "state_topic": format!("{}/state", topic_prefix),
            "value_template": "{{ value_json.fl_tire_pressure_alarm if (value_json is defined and value_json.fl_tire_pressure_alarm is defined) else 'OFF' }}",
            "json_attributes_topic": attributes_topic.clone(),
            "json_attributes_template": "{{ value }}",
            "device": device,
        })
        .to_string()
    );
    payload_map.insert(
        format!(
            "{}/binary_sensor/{}_{}/config",
            &*MQTT_CONFIG.discovery_topic, &prefix, "fr_tire_pressure_alarm"
        ),
        json!({
            "availability": availability,
            "name": format!("{} FR Tire Pressure Alarm", vehicle_info.showed_vin),
            "unique_id": format!("{}_fr_tire_pressure_alarm", prefix),
            "icon": "mdi:tire",
            "device_class": "problem",
            "state_topic": format!("{}/state", topic_prefix),
            "value_template": "{{ value_json.fr_tire_pressure_alarm if (value_json is defined and value_json.fr_tire_pressure_alarm is defined) else 'OFF' }}",
            "json_attributes_topic": attributes_topic.clone(),
            "json_attributes_template": "{{ value }}",
            "device": device,
        })
        .to_string()
    );
    payload_map.insert(
        format!(
            "{}/binary_sensor/{}_{}/config",
            &*MQTT_CONFIG.discovery_topic, &prefix, "rl_tire_pressure_alarm"
        ),
        json!({
            "availability": availability,
            "name": format!("{} RL Tire Pressure Alarm", vehicle_info.showed_vin),
            "unique_id": format!("{}_rl_tire_pressure_alarm", prefix),
            "icon": "mdi:tire",
            "device_class": "problem",
            "state_topic": format!("{}/state", topic_prefix),
            "value_template": "{{ value_json.rl_tire_pressure_alarm if (value_json is defined and value_json.rl_tire_pressure_alarm is defined) else 'OFF' }}",
            "json_attributes_topic": attributes_topic.clone(),
            "json_attributes_template": "{{ value }}",
            "device": device,
        })
        .to_string()
    );
    payload_map.insert(
        format!(
            "{}/binary_sensor/{}_{}/config",
            &*MQTT_CONFIG.discovery_topic, &prefix, "rr_tire_pressure_alarm"
        ),
        json!({
            "availability": availability,
            "name": format!("{} RR Tire Pressure Alarm", vehicle_info.showed_vin),
            "unique_id": format!("{}_rr_tire_pressure_alarm", prefix),
            "icon": "mdi:tire",
            "device_class": "problem",
            "state_topic": format!("{}/state", topic_prefix),
            "value_template": "{{ value_json.rr_tire_pressure_alarm if (value_json is defined and value_json.rr_tire_pressure_alarm is defined) else 'OFF' }}",
            "json_attributes_topic": attributes_topic.clone(),
            "json_attributes_template": "{{ value }}",
            "device": device,
        })
        .to_string()
    );
    payload_map.insert(
        format!(
            "{}/binary_sensor/{}_{}/config",
            &*MQTT_CONFIG.discovery_topic, &prefix, "fl_tire_temp_alarm"
        ),
        json!({
            "availability": availability,
            "name": format!("{} FL Tire Temperature Alarm", vehicle_info.showed_vin),
            "unique_id": format!("{}_fl_tire_temp_alarm", prefix),
            "icon": "mdi:tire",
            "device_class": "problem",
            "state_topic": format!("{}/state", topic_prefix),
            "value_template": "{{ value_json.fl_tire_temp_alarm if (value_json is defined and value_json.fl_tire_temp_alarm is defined) else 'OFF' }}",
            "json_attributes_topic": attributes_topic.clone(),
            "json_attributes_template": "{{ value }}",
            "device": device,
        })
        .to_string()
    );
    payload_map.insert(
        format!(
            "{}/binary_sensor/{}_{}/config",
            &*MQTT_CONFIG.discovery_topic, &prefix, "fr_tire_temp_alarm"
        ),
        json!({
            "availability": availability,
            "name": format!("{} FR Tire Temperature Alarm", vehicle_info.showed_vin),
            "unique_id": format!("{}_fr_tire_temp_alarm", prefix),
            "icon": "mdi:tire",
            "device_class": "problem",
            "state_topic": format!("{}/state", topic_prefix),
            "value_template": "{{ value_json.fr_tire_temp_alarm if (value_json is defined and value_json.fr_tire_temp_alarm is defined) else 'OFF' }}",
            "json_attributes_topic": attributes_topic.clone(),
            "json_attributes_template": "{{ value }}",
            "device": device,
        })
        .to_string()
    );
    payload_map.insert(
        format!(
            "{}/binary_sensor/{}_{}/config",
            &*MQTT_CONFIG.discovery_topic, &prefix, "rl_tire_temp_alarm"
        ),
        json!({
            "availability": availability,
            "name": format!("{} RL Tire Temperature Alarm", vehicle_info.showed_vin),
            "unique_id": format!("{}_rl_tire_temp_alarm", prefix),
            "icon": "mdi:tire",
            "device_class": "problem",
            "state_topic": format!("{}/state", topic_prefix),
            "value_template": "{{ value_json.rl_tire_temp_alarm if (value_json is defined and value_json.rl_tire_temp_alarm is defined) else 'OFF' }}",
            "json_attributes_topic": attributes_topic.clone(),
            "json_attributes_template": "{{ value }}",
            "device": device,
        })
        .to_string()
    );
    payload_map.insert(
        format!(
            "{}/binary_sensor/{}_{}/config",
            &*MQTT_CONFIG.discovery_topic, &prefix, "rr_tire_temp_alarm"
        ),
        json!({
            "availability": availability,
            "name": format!("{} RR Tire Temperature Alarm", vehicle_info.showed_vin),
            "unique_id": format!("{}_rr_tire_temp_alarm", prefix),
            "icon": "mdi:tire",
            "device_class": "problem",
            "state_topic": format!("{}/state", topic_prefix),
            "value_template": "{{ value_json.rr_tire_temp_alarm if (value_json is defined and value_json.rr_tire_temp_alarm is defined) else 'OFF' }}",
            "json_attributes_topic": attributes_topic.clone(),
            "json_attributes_template": "{{ value }}",
            "device": device,
        })
        .to_string()
    );
    payload_map.insert(
        format!(
            "{}/binary_sensor/{}_{}/config",
            &*MQTT_CONFIG.discovery_topic, &prefix, "head_light"
        ),
        json!({
            "availability": availability,
            "name": format!("{} Headlight Status", vehicle_info.showed_vin),
            "unique_id": format!("{}_head_light", prefix),
            "icon": "mdi:car-light-high",
            "device_class": "light",
            "state_topic": format!("{}/state", topic_prefix),
            "value_template": "{{ value_json.head_light if (value_json is defined and value_json.head_light is defined) else 'OFF' }}",
            "json_attributes_topic": attributes_topic.clone(),
            "json_attributes_template": "{{ value }}",
            "device": device,
        })
        .to_string()
    );
    payload_map.insert(
        format!(
            "{}/binary_sensor/{}_{}/config",
            &*MQTT_CONFIG.discovery_topic, &prefix, "left_turn_light"
        ),
        json!({
            "availability": availability,
            "name": format!("{} Left Turn Status", vehicle_info.showed_vin),
            "unique_id": format!("{}_left_turn_light", prefix),
            "icon": "mdi:car-light-dimmed",
            "device_class": "light",
            "state_topic": format!("{}/state", topic_prefix),
            "value_template": "{{ value_json.left_turn_light if (value_json is defined and value_json.left_turn_light is defined) else 'OFF' }}",
            "json_attributes_topic": attributes_topic.clone(),
            "json_attributes_template": "{{ value }}",
            "device": device,
        })
        .to_string()
    );
    payload_map.insert(
        format!(
            "{}/binary_sensor/{}_{}/config",
            &*MQTT_CONFIG.discovery_topic, &prefix, "right_turn_light"
        ),
        json!({
            "availability": availability,
            "name": format!("{} Right Turn Status", vehicle_info.showed_vin),
            "unique_id": format!("{}_right_turn_light", prefix),
            "icon": "mdi:car-light-dimmed",
            "device_class": "light",
            "state_topic": format!("{}/state", topic_prefix),
            "value_template": "{{ value_json.right_turn_light if (value_json is defined and value_json.right_turn_light is defined) else 'OFF' }}",
            "json_attributes_topic": attributes_topic.clone(),
            "json_attributes_template": "{{ value }}",
            "device": device,
        })
        .to_string()
    );
    payload_map.insert(
        format!(
            "{}/switch/{}_{}/config",
            &*MQTT_CONFIG.discovery_topic, &prefix, "lock_control"
        ),
        json!({
            "availability": [
                {
                    "topic": format!("{}/lock/availability", topic_prefix), "value_template": "{{ value }}"
                }
            ],
            "name": format!("{} Lock Control", vehicle_info.showed_vin),
            "unique_id": format!("{}_lock_control", prefix),
            "icon": "mdi:lock",
            "state_topic": format!("{}/state", topic_prefix),
            "value_template": "{{ value_json.unlock_status if (value_json is defined and value_json.unlock_status is defined) else 'OFF' }}",
            "command_topic": format!("{}/lock/set", topic_prefix),
            "json_attributes_topic": attributes_topic.clone(),
            "json_attributes_template": "{{ value }}",
            "device": device,
        })
        .to_string(),
    );
    payload_map.insert(
        format!(
            "{}/switch/{}_{}/config",
            &*MQTT_CONFIG.discovery_topic, &prefix, "climate_control"
        ),
        json!({
            "availability": [
                {
                    "topic": format!("{}/aircond/availability", topic_prefix), "value_template": "{{ value }}"
                }
            ],
            "name": format!("{} Climate Control", vehicle_info.showed_vin),
            "unique_id": format!("{}_climate_control", prefix),
            "icon": "mdi:air-conditioner",
            "state_topic": format!("{}/state", topic_prefix),
            "value_template": "{{ value_json.ac_status if (value_json is defined and value_json.ac_status is defined) else 'OFF' }}",
            "command_topic": format!("{}/aircond/set", topic_prefix),
            "json_attributes_topic": attributes_topic.clone(),
            "json_attributes_template": "{{ value }}",
            "device": device,
        })
        .to_string(),
    );
    payload_map.insert(
        format!(
            "{}/number/{}_{}/config",
            &*MQTT_CONFIG.discovery_topic, &prefix, "ac_timer"
        ),
        json!({
            "availability": availability,
            "name": format!("{} AC Timer", vehicle_info.showed_vin),
            "unique_id": format!("{}_ac_timer", prefix),
            "icon": "mdi:timer",
            "device_class": "duration",
            "unit_of_measurement": "min",
            "command_topic": format!("{}/ac_timer/set", topic_prefix),
            "min": 1,
            "max": 30,
            "step": 1,
            "state_topic": format!("{}/state", topic_prefix),
            "value_template": "{{ value_json.ac_time if (value_json is defined and value_json.ac_time is defined and value_json.ac_time|int > 0 and value_json.ac_time|int <= 30) else '15' }}",
            "json_attributes_topic": attributes_topic.clone(),
            "json_attributes_template": "{{ value }}",
            "device": device,
        })
        .to_string(),
    );
    payload_map.insert(
        format!(
            "{}/number/{}_{}/config",
            &*MQTT_CONFIG.discovery_topic, &prefix, "ac_temperature"
        ),
        json!({
            "availability": availability,
            "name": format!("{} AC Temperature", vehicle_info.showed_vin),
            "unique_id": format!("{}_ac_temperature", prefix),
            "icon": "mdi:thermometer",
            "device_class": "temperature",
            "unit_of_measurement": "°C",
            "command_topic": format!("{}/ac_temperature/set", topic_prefix),
            "min": 17,
            "max": 31,
            "step": 1,
            "state_topic": format!("{}/state", topic_prefix),
            "value_template": "{{ value_json.ac_temp if (value_json is defined and value_json.ac_temp is defined and value_json.ac_temp|int >= 17 and value_json.ac_temp|int <= 31) else '26' }}",
            "json_attributes_topic": attributes_topic.clone(),
            "json_attributes_template": "{{ value }}",
            "device": device,
        })
        .to_string(),
    );
    payload_map.insert(
        format!(
            "{}/switch/{}_{}/config",
            &*MQTT_CONFIG.discovery_topic, &prefix, "petmode"
        ),
        json!({
            "availability": availability,
            "name": format!("{} Pet Mode", vehicle_info.showed_vin),
            "unique_id": format!("{}_petmode", prefix),
            "icon": "mdi:paw",
            "state_topic": format!("{}/state", topic_prefix),
            "value_template": "{{ value_json.petmode if (value_json is defined and value_json.petmode is defined) else 'OFF' }}",
            "command_topic": format!("{}/petmode/set", topic_prefix),
            "json_attributes_topic": attributes_topic.clone(),
            "json_attributes_template": "{{ value }}",
            "device": device,
        })
        .to_string(),
    );
    for (topic, payload) in payload_map.iter() {
        debug!("Publishing: {} => {}", topic, payload);
        match client.publish(topic, QOS, true, payload.to_string()).await {
            Ok(_) => {
                debug!("Published: {} => {}", topic, payload);
            }
            Err(e) => {
                error!("Failed to publish discovery: {}", e);
            }
        }
    }
    info!("Discovery published");
    Ok(())
}

pub async fn publish_state(
    client: &AsyncClient,
    vehicle_info: &gwm::VehicleInfo,
    vehicle_status: &gwm::VehicleStatus,
    topic_prefix: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    info!("Publishing state");
    let availability_topic = format!("{}/availability", topic_prefix);
    let attributes_topic = format!("{}/attributes", topic_prefix);
    let state_topic = format!("{}/state", topic_prefix);
    let tracker_attributes_topic = format!("{}/tracker/attributes", topic_prefix);
    let mut payload_map: HashMap<String, String> = HashMap::new();
    payload_map.insert(availability_topic.clone(), "online".to_string());
    payload_map.insert(
        attributes_topic.clone(),
        json!({
            "vin": vehicle_info.showed_vin,
            "brand": vehicle_info.brand_name,
            "model": vehicle_info.vtype,
            "submodel": vehicle_info.config,
            "charge": vehicle_status.soc,
            "range": vehicle_status.range,
            "mileage": vehicle_status.mileage,
            "updated_on": vehicle_status.updated_at
        })
        .to_string(),
    );
    payload_map.insert(
        state_topic.clone(),
        json!({
            "mileage": vehicle_status.mileage,
            "soc": vehicle_status.soc,
            "range": vehicle_status.range,
            "charge_time": vehicle_status.charge_time,
            "charging_status": vehicle_status.charging_status,
            "charging_status_desc": vehicle_status.charging_status_desc,
            "charging_port_plugged": if vehicle_status.charging_port_plugged {
                "ON"
            } else {
                "OFF"
            },
            "schedule_charge": if vehicle_status.schedule_charge {
                "ON"
            } else {
                "OFF"
            },
            "ac_status": if vehicle_status.ac_status {
                "ON"
            } else {
                "OFF"
            },
            "air_filter_status": if vehicle_status.air_filter_status {
                "ON"
            } else {
                "OFF"
            },
            "unlock_status": if vehicle_status.unlock_status {
                "ON"
            } else {
                "OFF"
            },
            "fl_door_open": if vehicle_status.fl_door_open {
                "ON"
            } else {
                "OFF"
            },
            "fr_door_open": if vehicle_status.fr_door_open {
                "ON"
            } else {
                "OFF"
            },
            "rl_door_open": if vehicle_status.rl_door_open {
                "ON"
            } else {
                "OFF"
            },
            "rr_door_open": if vehicle_status.rr_door_open {
                "ON"
            } else {
                "OFF"
            },
            "trunk_open": if vehicle_status.trunk_open {
                "ON"
            } else {
                "OFF"
            },
            "fl_window_open": if vehicle_status.fl_window_open {
                "ON"
            } else {
                "OFF"
            },
            "fr_window_open": if vehicle_status.fr_window_open {
                "ON"
            } else {
                "OFF"
            },
            "rl_window_open": if vehicle_status.rl_window_open {
                "ON"
            } else {
                "OFF"
            },
            "rr_window_open": if vehicle_status.rr_window_open {
                "ON"
            } else {
                "OFF"
            },
            "sunroof_open": if vehicle_status.sunroof_open {
                "ON"
            } else {
                "OFF"
            },
            "fl_tire_pressure": vehicle_status.fl_tire_pressure,
            "fr_tire_pressure": vehicle_status.fr_tire_pressure,
            "rl_tire_pressure": vehicle_status.rl_tire_pressure,
            "rr_tire_pressure": vehicle_status.rr_tire_pressure,
            "fl_tire_temp": vehicle_status.fl_tire_temp,
            "fr_tire_temp": vehicle_status.fr_tire_temp,
            "rl_tire_temp": vehicle_status.rl_tire_temp,
            "rr_tire_temp": vehicle_status.rr_tire_temp,
            "fl_tire_pressure_alarm": if vehicle_status.fl_tire_pressure_alarm {
                "ON"
            } else {
                "OFF"
            },
            "fr_tire_pressure_alarm": if vehicle_status.fr_tire_pressure_alarm {
                "ON"
            } else {
                "OFF"
            },
            "rl_tire_pressure_alarm": if vehicle_status.rl_tire_pressure_alarm {
                "ON"
            } else {
                "OFF"
            },
            "rr_tire_pressure_alarm": if vehicle_status.rr_tire_pressure_alarm {
                "ON"
            } else {
                "OFF"
            },
            "fl_tire_temp_alarm": if vehicle_status.fl_tire_temp_alarm {
                "ON"
            } else {
                "OFF"
            },
            "fr_tire_temp_alarm": if vehicle_status.fr_tire_temp_alarm {
                "ON"
            } else {
                "OFF"
            },
            "rl_tire_temp_alarm": if vehicle_status.rl_tire_temp_alarm {
                "ON"
            } else {
                "OFF"
            },
            "rr_tire_temp_alarm": if vehicle_status.rr_tire_temp_alarm {
                "ON"
            } else {
                "OFF"
            },
            "head_light": if vehicle_status.head_light {
                "ON"
            } else {
                "OFF"
            },
            "left_turn_light": if vehicle_status.left_turn_light {
                "ON"
            } else {
                "OFF"
            },
            "right_turn_light": if vehicle_status.right_turn_light {
                "ON"
            } else {
                "OFF"
            },
            "ac_time": vehicle_status.ac_time,
            "ac_temp": vehicle_status.ac_temp,
            "petmode": if vehicle_status.petmode {
                "ON"
            } else {
                "OFF"
            },
        })
        .to_string(),
    );
    payload_map.insert(
        tracker_attributes_topic.clone(),
        json!({
            "vin": vehicle_info.showed_vin,
            "brand": vehicle_info.brand_name,
            "model": vehicle_info.vtype,
            "submodel": vehicle_info.config,
            "charge": vehicle_status.soc,
            "range": vehicle_status.range,
            "mileage": vehicle_status.mileage,
            "latitude": vehicle_status.latitude,
            "longitude": vehicle_status.longitude,
            "gps_accuracy": 0,
        })
        .to_string(),
    );
    for (topic, payload) in payload_map.iter() {
        debug!("Publishing: {} => {}", topic, payload);
        match client.publish(topic, QOS, true, payload.to_string()).await {
            Ok(_) => {
                debug!("Published: {} => {}", topic, payload);
            }
            Err(e) => {
                error!("Failed to publish state: {}", e);
            }
        }
    }
    info!("State published");
    Ok(())
}
