//! Example of Access Point Monitor for CSI Collection
//!
//! This configuration allows other ESP Trigger Stations to connect to stimulate CSI data locally.
//! This configuration only monitors the traffic without actually collecting CSI data itself.
//!
//! At least two ESP devices (one Station Trigger and one Access Point Monitor) are needed to enable this configuration.
//! More ESP Trigger stations connecting to the Access Point can also be added to form a star topology.
//!
//!
//! Connection Options:
//! - Option 1: Allow one ESP configured as a Station Trigger to connect to a single ESP Access Point Monitor.
//! - Option 2: Allow multiple ESPs configured as a Station Triggers to connect to a single ESP Access Point Monitor.
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
use esp_csi_rs::collector::{ApOperationMode, CSIAccessPoint};
use esp_hal::rng::Rng;
use esp_hal::timer::timg::TimerGroup;
use esp_println as _;
use esp_println::println;
use esp_wifi::{init, wifi::AccessPointConfiguration, EspWifiController};

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
    let rng = Rng::new(peripherals.RNG);

    // Initialize WiFi Controller
    let init = &*mk_static!(EspWifiController<'static>, init(timer, rng).unwrap());

    // Instantiate WiFi controller and interfaces
    let (controller, interfaces) = esp_wifi::wifi::new(&init, wifi).unwrap();

    println!("WiFi Controller Initialized");

    // Create a CSI collector configuration for an Access Point
    // Configure Access Point as Monitor
    let mut csi_coll_ap = CSIAccessPoint::new(
        // Specify Any Access Point Configuration
        AccessPointConfiguration {
            ssid: "esp".try_into().unwrap(),
            password: "12345678".try_into().unwrap(),
            auth_method: esp_wifi::wifi::AuthMethod::WPA2Personal,
            max_connections: 1,
            ..Default::default()
        },
        // Specify Operation Mode
        ApOperationMode::Monitor,
        controller,
    )
    .await;

    // Initalize CSI collector AP
    csi_coll_ap.init(interfaces, &spawner).await.unwrap();

    // Start AP to Enable CSI collection at stations
    // In monitor mode, the Access point only runs to support stations connecting to it.
    csi_coll_ap.start_collection().await;
    println!("Started AP First Time");

    // Run for 2 Seconds
    Timer::after(Duration::from_secs(3)).await;

    // Stop & Recapture Controller for Next Collection Activity
    csi_coll_ap.stop_collection().await;
    println!("Stopped AP");

    // Ex. Update AP Configuration
    println!("Updating Configuration");
    csi_coll_ap
        .update_ap_config(AccessPointConfiguration::default())
        .await;

    // Wait some time before starting again
    println!("Starting Again in 3 seconds");
    Timer::after(Duration::from_secs(3)).await;

    // Start AP again to enable collection at stations
    csi_coll_ap.start_collection().await;
    println!("Started AP Again");

    // Run for 10 Seconds
    Timer::after(Duration::from_secs(10)).await;

    // Stop Access Point
    csi_coll_ap.stop_collection().await;
    println!("Stopped AP");

    loop {
        Timer::after(Duration::from_secs(1)).await
    }
}
