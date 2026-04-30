mod logging;

fn main() {
    let _ = logging::init_logger();
    gwm2mqtt_lib::run(parse_args());
}

fn parse_args() -> u16 {
    let default: u16 = option_env!("WEB_PORT")
        .unwrap_or("8080")
        .parse()
        .unwrap_or(8080);

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-p" | "--port" => match args.next().and_then(|v| v.parse::<u16>().ok()) {
                Some(port) => return port,
                None => {
                    eprintln!("error: -p requires a valid port number (1-65535)");
                    std::process::exit(1);
                }
            },
            "-h" | "--help" => {
                println!("Usage: gwm2mqtt [-p PORT]");
                println!();
                println!("Options:");
                println!("  -p, --port PORT  Web dashboard port (default: {default})");
                println!("  -h, --help       Show this help");
                std::process::exit(0);
            }
            unknown => {
                eprintln!("error: unknown argument '{unknown}'");
                eprintln!("Usage: gwm2mqtt [-p PORT]");
                std::process::exit(1);
            }
        }
    }
    default
}
