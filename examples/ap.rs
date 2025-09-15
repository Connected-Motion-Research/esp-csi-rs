//! Example of Access Point Mode for CSI Collection
//!
//! This configuration allows other ESP devices configured in Access Point mode to allow Stations to connect to and collect CSI data.
//!
//! At least two ESP devices (one Station and one Access Point) are needed to enable this configuration.
//! More ESP stations connecting to the Access Point can also be added to form a star topology.
//!
//! IMPORTANT NOTE: APs collect CSI data as well.
//!
//! Connection Options:
//! - Option 1: Allow one ESP configured as a Station to connect to the ESP Access Point.
//! - Option 2: Allow multiple ESPs configured as a Station to connect to the ESP Access Point.
//!
//! The `ap_ssid` and `ap_password` defined are the ones the ESP Station(s) needs to connect to the ESP  Access Point.
//! `max_connections` defines the maximum number of ESP Stations that can connect to the ESP Access Point.
//! The default value of `max_connections` is 1. If you want to connect more stations you will need to increase it.
//!
//! There is an option also to hide the ESP Access Point SSID by setting `ssid_hidden` to `true`.

#![no_std]
#![no_main]

use embassy_executor::Spawner;
use embassy_time::{Duration, Timer};
use esp_bootloader_esp_idf::esp_app_desc;
use esp_csi_rs::{
    config::{CSIConfig, TrafficConfig, WiFiConfig},
    CSICollector, NetworkArchitechture,
};
use esp_hal::rng::Rng;
use esp_hal::timer::timg::TimerGroup;
use esp_println as _;
use esp_println::println;
use esp_wifi::{init, EspWifiController};

esp_app_desc!();

extern crate alloc;

macro_rules! mk_static {
    ($t:ty,$val:expr) => {{
        static STATIC_CELL: static_cell::StaticCell<$t> = static_cell::StaticCell::new();
        #[deny(unused_attributes)]
        let x = STATIC_CELL.uninit().write(($val));
        x
    }};
}

#[esp_hal_embassy::main]
async fn main(spawner: Spawner) {
    // Configure System Clock
    let config = esp_hal::Config::default().with_cpu_clock(esp_hal::clock::CpuClock::max());
    // Take Peripherals
    let peripherals = esp_hal::init(config);

    // Allocate some heap space
    esp_alloc::heap_allocator!(size: 72 * 1024);

    // Initialize Embassy
    let timg1 = TimerGroup::new(peripherals.TIMG1);
    esp_hal_embassy::init(timg1.timer0);

    // Instantiate peripherals necessary to set up  WiFi
    let timer1 = esp_hal::timer::timg::TimerGroup::new(peripherals.TIMG0);
    let wifi = peripherals.WIFI;
    let timer = timer1.timer0;
    let mut rng = Rng::new(peripherals.RNG);

    // Initialize WiFi Controller
    let init = &*mk_static!(EspWifiController<'static>, init(timer, rng).unwrap());

    // Instantiate WiFi controller and interfaces
    let (controller, interfaces) = esp_wifi::wifi::new(&init, wifi).unwrap();

    // Obtain a random seed value
    let seed = rng.random() as u64;

    println!("WiFi Controller Initialized");

    // Create a CSI collector configuration
    // Device configured as a Access Point
    // Traffic is not enabled, so configuration is ignored. Connected stations are expected to generate traffic.
    // Network Architechture is AccessPoint-Station (NTP time collection not possible)
    let csi_collector = CSICollector::new(
        WiFiConfig {
            ap_ssid: "esp".try_into().unwrap(),
            ap_password: "12345678".try_into().unwrap(),
            max_connections: 2,
            ssid_hidden: false,
            ..Default::default()
        },
        esp_csi_rs::WiFiMode::AccessPoint,
        CSIConfig::default(),
        TrafficConfig::default(),
        false,
        NetworkArchitechture::AccessPointStation,
        None,
        false,
    );

    // Initalize CSI collector
    csi_collector
        .init(controller, interfaces, seed, &spawner)
        .unwrap();

    // Start Collecting CSI data
    // In access point mode, defining a duration does not matter. This is because there isnt any collection happening.
    // The Access point only runs to support stations connecting to it.
    csi_collector.start(None);

    loop {
        // Yeild to other tasks
        Timer::after(Duration::from_secs(1)).await
    }
}
