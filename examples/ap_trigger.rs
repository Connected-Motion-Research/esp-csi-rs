//! Example of Access Point Trigger for CSI Collection
//!
//! This configuration generates trigger traffic to connected ESP Station Monitors to stimulate CSI data collection.
//! CSI collected at the stations is sent back to this Access Point Trigger over UDP that is reconstructed as a `CSIDataPacket`.
//!
//! At least two ESP devices (one Station Monitor and one Access Point Trigger) are needed to enable this configuration.
//! More ESP monitor stations connecting to the Access Point can also be added to form a star topology.
//! The trigger traffic can be either an ICMP Echo Request (Ping) broadcast or unicast.
//!
//! Connection Options:
//! - Option 1: Allow one ESP configured as a Station Monitor to connect to a single ESP Access Point Trigger.
//! - Option 2: Allow multiple ESPs configured as a Station Monitors to connect to a single ESP Access Point Trigger.
//!
//! The `ap_ssid` and `ap_password` defined are the ones the ESP Station(s) needs to connect to the ESP Access Point.
//! `max_connections` defines the maximum number of ESP Stations that can connect to the ESP Access Point.
//! The default value of `max_connections` is 1. If you want to connect more stations you will need to increase it.
//!
//! There is an option also to hide the ESP Access Point SSID by setting `ssid_hidden` to `true`.

#![no_std]
#![no_main]

use embassy_executor::Spawner;
use embassy_time::{with_timeout, Duration, Timer};
use esp_bootloader_esp_idf::esp_app_desc;
use esp_csi_rs::collector::{ApOperationMode, ApTriggerConfig, CSIAccessPoint};
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

    // Create a CSI collector configuration
    // Access Point in Trigger Mode
    let mut csi_coll_ap = CSIAccessPoint::new(
        // Specify Any Access Point Configuration
        AccessPointConfiguration {
            ssid: "esp".try_into().unwrap(),
            password: "12345678".try_into().unwrap(),
            auth_method: esp_wifi::wifi::AuthMethod::WPA2Personal,
            max_connections: 1,
            ..Default::default()
        },
        // Specify Operation Mode as Trigger with Default Trigger Configuration
        ApOperationMode::Trigger(ApTriggerConfig::default()),
        controller,
    )
    .await;

    // Initalize CSI collector AP
    csi_coll_ap.init(interfaces, &spawner).await.unwrap();

    // Start AP to Enable CSI collection at stations
    // In trigger mode, the Access point sends trigger packets to stimulate CSI collection at connected stations.
    // The collected CSI is transferred back to the Access Point.
    csi_coll_ap.start_collection().await;
    println!("Started AP First Time");

    // Recieve Data Collected from Stations for 2 Seconds
    with_timeout(Duration::from_secs(60), async {
        loop {
            csi_coll_ap.print_csi_w_metadata().await;
        }
    })
    .await
    .unwrap_err();

    // Stop Collection
    csi_coll_ap.stop_collection().await;

    loop {
        Timer::after(Duration::from_secs(1)).await
    }
}
