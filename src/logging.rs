const TRIGGER_FILE_SIZE: u64 = 10 * 1024 * 1024;
const LOG_FILE_COUNT: u32 = 10;
const FILE_PATH: &str = "/opt/gwm2mqtt/logs/app.log";
const ARCHIVE_PATTERN: &str = "/opt/gwm2mqtt/logs/app.{}.log";

use log::{error, info, LevelFilter, SetLoggerError};
use log4rs::{
    append::{
        console::{ConsoleAppender, Target},
        rolling_file::{
            policy::compound::{
                roll::fixed_window::FixedWindowRoller, trigger::size::SizeTrigger, CompoundPolicy,
            },
            RollingFileAppender,
        },
    },
    config::{Appender, Config, Root},
    encode::pattern::PatternEncoder,
    filter::threshold::ThresholdFilter,
};

pub fn init_logger() -> Result<(), SetLoggerError> {
    let level = LevelFilter::Debug;
    let stderr = ConsoleAppender::builder()
        .target(Target::Stderr)
        .encoder(Box::new(PatternEncoder::new("{d} {l} {t} - {m}{n}")))
        .build();
    let trigger = SizeTrigger::new(TRIGGER_FILE_SIZE);
    let roller = FixedWindowRoller::builder()
        .build(ARCHIVE_PATTERN, LOG_FILE_COUNT)
        .unwrap_or_else(|e| {
            panic!("Failed to create log roller: {}", e);
        });
    let policy = CompoundPolicy::new(Box::new(trigger), Box::new(roller));
    let file = RollingFileAppender::builder()
        .encoder(Box::new(PatternEncoder::new("{d} {l} {t} - {m}{n}")))
        .append(true)
        .build(FILE_PATH, Box::new(policy))
        .unwrap_or_else(|e| {
            panic!("Failed to create rolling file appender: {}", e);
        });
    let config = Config::builder()
        .appender(Appender::builder().build("file", Box::new(file)))
        .appender(
            Appender::builder()
                .filter(Box::new(ThresholdFilter::new(LevelFilter::Info)))
                .build("stderr", Box::new(stderr)),
        )
        .build(
            Root::builder()
                .appender("file")
                .appender("stderr")
                .build(level),
        )
        .unwrap_or_else(|e| {
            panic!("Failed to build log configuration: {}", e);
        });

    match log4rs::init_config(config) {
        Ok(_) => {
            info!("Logger initialized");
            Ok(())
        }
        Err(e) => {
            error!("Failed to initialize logger: {}", e);
            Err(e)
        }
    }
}
