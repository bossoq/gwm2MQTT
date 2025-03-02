mod logging;

fn main() {
    let _ = logging::init_logger();
    gwm2mqtt_lib::run();
}
