//! # A crate for CSI collection on ESP devices
//! ## Overview
//! This crate builds on the low level Espressif abstractions to enable the collection of Channel State Information (CSI) on ESP devices with ease.
//! Currently this crate supports only the ESP `no-std` development framework.
//!
//! ### Choosing a device
//! In terms of hardware, you need to make sure that the device you choose supports WiFI and CSI collection.
//! Currently supported deveices are:
//! - ESP32
//! - ESP32-C2
//! - ESP32-C3
//! - ESP32-C6
//! - ESP32-S3
//!
//! In terms of software toolchain setup, you will need to specify the hardware you will be using. To minimize headache, it is recommended that you generate a project using `esp-generate` as explained next.
//!
//! ### Creating a project
//! To use this crate you would need to create and setup a project for your ESP device then import the crate. This crate is compatible with `no-std` ESP development projects. You should also select the corresponding device by activating it in the crate features.
//!
//! To create a projects it is highly recommended to refer the to instructions in [The Rust on ESP Book](https://docs.esp-rs.org/book/) before proceeding. The book explains the full esp-rs ecosystem, how to get started, and how to generate projects for both `std` and `no-std`.
//!
//! Espressif has developed a project generation tool, `esp-generate`, to ease this process and is recommended for new projects. As an example, you can create a `no-std` project as follows:
//!
//! ```bash
//! cargo install esp-generate
//! esp-generate --chip=esp32c3 [project-name]
//! ```
//!
//! ## Feature Flags
#![doc = document_features::document_features!()]
//! ## Using the `esp-csi-rs` Crate
//! With the exception of sniffer mode, the collection of CSI requires at least two WiFI enabled devices; an Access Point and a Station. Both devices could be ESP devices one programmed as a Station and another as an Access Point. Alternatively, the simplest setup is using one ESP device as a Station connecting to an existing Access Point like a home router.
//! This crate supports creating both Access Points and Stations and there are several examples to demonstrate in the repository. When both devices are ESPs, the Access Point and the Station are able to collect CSI data.
//!
//! ### Traffic Generation
//! To recieve CSI data, there needs to be regular traffic on the network. There are two options to generate traffic:
//! - **Internal Trigger**: The `CSICollector` can be configured to generate dummy traffic at a desired rate. This traffic would trigger CSI data collection. The crate provides the option of using ICMP or UDP to generate traffic.
//! - **External Trigger**: Instead of generating its own traffic, an external trigger can be provided (Ex. a UDP broadcast). The station would in turn return a UDP packet on port 10987 with the CSI data. Additionally, if the trigger is an ICMP Echo Request Packet the sequence number is also returned.
//!
//! **Note**: If sequence numbers are desired using an external trigger, sequence number capture should be enabled in the CSI configuration. It is highly recommened to also add a MAC address filter for the trigger source to reduce processing overhead and false positives. As already mentioned, the traffic trigger also needs to be an ICMP echo request for sequence number support
//!
//! ### External Trigger Data Formatting
//! If sending an external trigger, the returned UDP message is formatted as follows:
//!
//! `[0..1]`   : 2 bytes for sequence number (u16) - big endian
//!
//! `[2]`      : 1 byte for CSI data format (Refer to the `RxCSIFmt` enum for details)
//!
//! `[2..7]`   : 4 bytes for capture timestamp (u32) - big endian
//!
//! `[7..n]`   : Up to 612 bytes of raw CSI data (i8)
//!
//! ### Data Collection
//! There are three ways to collect CSI data using `esp-csi-rs`:
//! - **Process a** `CSIDataPacket`: The `get_csi_data()` method returns a `CSIDataPacket` struct that contains all the captured CSI and its metadata. This data can be processed locally, stored to a file, or even sent to a sotrage device (Ex. SD Card).
//! - **Print to console**: `CSIDataPacket` offers a `print_csi_w_metadata` method that formats and prints the CSI data to the console. This data can then be stored and processed by a host device.
//! - **Send to Trigger**: Recieve a UDP Message by providing an external ICMP echo request trigger.
//!
//! ### Example for Collecting CSI in Station Mode
//!
//! There are more examples in the repository. The example below demonstrates how to collect CSI data with an ESP configured in Station mode.
//!
//! This configuration allows the collection of CSI data by connecting to a WiFi router or ESP Access Point. Connection Options include:
//! - **Option 1**: Connect to an existing commercial router
//! - **Option 2**: Connect to another ESP programmed in AP Mode or AP/STA Mode
//!
//! #### Step 1: Create a CSI Collection Configuration/Profile
//!```rust, no_run
//! let csi_collector = CSICollector::new(
//!     WiFiConfig {
//!         // SSID & Password of the Access Point or Router the Station will be connect to
//!         ssid: "AP_SSID".try_into().unwrap(),
//!         password: "AP_PASSWORD".try_into().unwrap(),
//!         ..Default::default()
//!     },
//!     // We Will Connect as a Station
//!     esp_csi_rs::WiFiMode::Station,
//!     // Use Default CSI Configuration Parameters
//!     CSIConfig::default(),
//!     // Generate UDP traffic every 1 second
//!     TrafficConfig {
//!         traffic_type: TrafficType::UDP,
//!         traffic_interval_ms: 1000,
//!     },
//!     // Enable traffic
//!     true,
//!     // Define architecture deployed Ex. We're going to connect to a router
//!     NetworkArchitechture::RouterStation,
//!     // Don't Filter any Mac Addresses
//!     None,
//!     // Disable Sequence Number Capture Enable
//!     false,
//! );
//!```
//!
//! #### Step 2: Initialize CSI Collection
//!```rust, no_run
//!csi_collector.init(wifi, init, seed, &spawner).unwrap();
//!```
//!
//! #### Step 3: Start Collection
//!```rust, no_run
//! // Starts Collection for 10 seconds then stops
//!csi_collector.start(10);
//!```
//!
//! #### Step 4: Retrieve CSI Data
//! ```rust, no_run
//! let csi = csi_collector.get_csi_data().await;
//! // CSIDataPacket processing code
//!```
//! Alternatively, you can print the CSI data & metadata directly to console as follows:
//!```rust, no_run
//! csi_collector.print_csi_w_metadata().await;
//!```

#![no_std]

use core::cell::RefCell;
use core::sync::atomic::{AtomicBool, AtomicU16, AtomicU32};
use embassy_executor::Spawner;
use embassy_sync::pubsub::{PubSubBehavior, PubSubChannel};
use embassy_sync::watch::{Receiver, Watch};
use embassy_time::{with_timeout, Duration, Instant, Timer};

#[cfg(feature = "println")]
use esp_println as _;
#[cfg(feature = "println")]
use esp_println::print;
#[cfg(feature = "println")]
use esp_println::println;

#[cfg(feature = "defmt")]
use defmt::info;
#[cfg(feature = "defmt")]
use defmt::println;

use esp_wifi::wifi::Interfaces;
use esp_wifi::wifi::Sniffer;
use smoltcp::phy::ChecksumCapabilities;
use smoltcp::wire::{Icmpv4Packet, Icmpv4Repr, Ipv4Packet, Ipv4Repr};

use embassy_net::{
    raw::{IpProtocol, IpVersion, PacketMetadata as RawPacketMetadata, RawSocket},
    udp::{PacketMetadata, UdpSocket},
    IpAddress, IpEndpoint, Ipv4Address, Ipv4Cidr, Runner, Stack, StackResources, StaticConfigV4,
};
use esp_alloc as _;
use esp_backtrace as _;
use esp_wifi::wifi::{
    AccessPointConfiguration, ClientConfiguration, Configuration, CsiConfig, WifiController,
    WifiDevice, WifiEvent, WifiState,
};

use enumset::enum_set;

use core::{net::Ipv4Addr, str::FromStr};
use embassy_sync::{
    blocking_mutex::raw::CriticalSectionRawMutex, blocking_mutex::Mutex, once_lock::OnceLock,
    signal::Signal,
};

use ieee80211::{data_frame::DataFrame, match_frames};

use heapless::Vec;

pub mod collector;
pub mod config;
mod csi;
mod error;
mod time;

use crate::config::*;
pub use crate::csi::CSIDataPacket;
use crate::error::{Error, Result};
pub use crate::time::*;

macro_rules! mk_static {
    ($t:ty,$val:expr) => {{
        static STATIC_CELL: static_cell::StaticCell<$t> = static_cell::StaticCell::new();
        #[deny(unused_attributes)]
        let x = STATIC_CELL.uninit().write(($val));
        x
    }};
}

const NTP_UNIX_OFFSET: u32 = 2_208_988_800; // 1900 to 1970 offset in seconds
const NTP_SERVER: &str = "pool.ntp.org";
const NTP_PORT: u16 = 123;

static DATE_TIME: OnceLock<DateTimeCapture> = OnceLock::new();
static START_COLLECTION: Signal<CriticalSectionRawMutex, Option<u64>> = Signal::new();
static START_COLLECTION_: Signal<CriticalSectionRawMutex, bool> = Signal::new();
static DATE_TIME_VALID: AtomicBool = AtomicBool::new(false);
static LAST_SEQ_NUM: AtomicU16 = AtomicU16::new(0);
static LAST_TIMESTAMP: AtomicU32 = AtomicU32::new(0);
static SEQ_NUM_EN: AtomicBool = AtomicBool::new(false);

static CONTROLLER_CONFIG: Mutex<CriticalSectionRawMutex, RefCell<Option<Configuration>>> =
    Mutex::new(RefCell::new(None));
static NETWORK_CONFIG: Mutex<CriticalSectionRawMutex, RefCell<NetworkArchitechture>> =
    Mutex::new(RefCell::new(NetworkArchitechture::Sniffer));
static COLLECTION_CONFIG: Mutex<CriticalSectionRawMutex, RefCell<Option<CSIConfig>>> =
    Mutex::new(RefCell::new(None));
static OPERATION_MODE: Mutex<CriticalSectionRawMutex, RefCell<WiFiMode>> =
    Mutex::new(RefCell::new(WiFiMode::Sniffer));
static PROC_CSI_DATA: Watch<CriticalSectionRawMutex, CSIDataPacket, 3> = Watch::new();
static CSI_PACKET: PubSubChannel<CriticalSectionRawMutex, CSIDataPacket, 4, 1, 1> =
    PubSubChannel::new();
static DHCP_COMPLETE: Signal<CriticalSectionRawMutex, ()> = Signal::new();

/// A mapping of the different possible recieved CSI data formats supported by the Espressif WiFi driver.
/// `RxCSIFmt`` encodes the different formats (each column in the table) in one byte to save space when transmitting back CSI data.
/// The driver can be found here:
/// <https://docs.espressif.com/projects/esp-idf/en/latest/esp32s3/api-guides/wifi.html#wi-fi-channel-state-information>
#[derive(Debug, Clone)]
#[repr(u8)]
pub enum RxCSIFmt {
    /// Sec Chnl = None, Sig Mode = non-Ht, Chnl BW = 20MHz, non-STBC
    Bw20,
    /// Sec Chnl = None, Sig Mode = Ht, Chnl BW = 20MHz, non-STBC         
    HtBw20,
    /// Sec Chnl = None, Sig Mode = Ht, Chnl BW = 20MHz, STBC
    HtBw20Stbc,
    /// Sec Chnl = Below, Sig Mode = non-Ht, Chnl BW = 20MHz, non-STBC
    SecbBw20,
    /// Sec Chnl = Below, Sig Mode = Ht, Chnl BW = 20MHz, non-STBC
    SecbHtBw20,
    /// Sec Chnl = Below, Sig Mode = Ht, Chnl BW = 20MHz, STBC
    SecbHtBw20Stbc,
    /// Sec Chnl = Below, Sig Mode = Ht, Chnl BW = 40MHz, non-STBC
    SecbHtBw40,
    /// Sec Chnl = Below, Sig Mode = Ht, Chnl BW = 40MHz, STBC
    SecbHtBw40Stbc,
    /// Sec Chnl = Above, Sig Mode = non-Ht, Chnl BW = 20MHz, non-STBC
    SecaBw20,
    /// Sec Chnl = Above, Sig Mode = Ht, Chnl BW = 20MHz, non-STBC
    SecaHtBw20,
    /// Sec Chnl = Above, Sig Mode = Ht, Chnl BW = 20MHz, STBC
    SecaHtBw20Stbc,
    /// Sec Chnl = Above, Sig Mode = Ht, Chnl BW = 40MHz, non-STBC
    SecaHtBw40,
    /// Sec Chnl = Above, Sig Mode = Ht, Chnl BW = 40MHz, STBC
    SecaHtBw40Stbc,
    /// Not a defined format
    Undefined,
}

// Date Time Struct
#[derive(Debug, Clone)]
struct DateTimeCapture {
    captured_at: Instant,
    captured_secs: u64,
    captured_millis: u64,
}

#[derive(Debug, Clone)]
struct DateTime {
    year: u64,
    month: u64,
    day: u64,
    hour: u64,
    minute: u64,
    second: u64,
    millisecond: u64,
}

/// Device operation modes
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WiFiMode {
    /// Access Point mode
    AccessPoint,
    /// Station (client) mode
    Station,
    /// Access Point + Station mode
    AccessPointStation,
    /// Monitor (sniffer) mode
    Sniffer,
}

/// Network Architechture Options
#[derive(Debug, Clone, Copy)]
pub enum NetworkArchitechture {
    /// Router Connected to One Station
    RouterStation,
    /// Router Connected to Access Point + Station Connected to One or More Station(s)
    RouterAccessPointStation,
    /// Access Point Connected to One or More Station(s)
    AccessPointStation,
    /// Standalone Station (Sniffer)
    Sniffer,
}

#[derive(Debug, Clone)]
struct IpInfo {
    pub local_address: Ipv4Cidr,
    pub gateway_address: Ipv4Address,
}

/// Main Driver Struct for CSI Collection
pub struct CSICollector {
    /// WiFi Configuration
    pub wifi_config: WiFiConfig,
    /// Device Operation Mode
    pub op_mode: WiFiMode,
    /// CSI Collection Parameters
    pub csi_config: CSIConfig,
    /// Traffic Generator Configuration
    pub traffic_config: TrafficConfig,
    /// Traffic Generation Enable
    pub traffic_enabled: bool,
    /// Network Architechture Option
    pub net_arch: NetworkArchitechture,
    /// MAC Address Filter fo CSI Data
    pub mac_filter: Option<[u8; 6]>,
    /// Enable Sequence Number Capture
    pub sequence_no_en: bool,
    csi_data_rx: Receiver<'static, CriticalSectionRawMutex, CSIDataPacket, 3>,
}

impl CSICollector {
    /// Creates a new CSICollector instance with a defined configuration/profile.
    pub fn new(
        wifi_config: WiFiConfig,
        op_mode: WiFiMode,
        csi_config: CSIConfig,
        traffic_config: TrafficConfig,
        traffic_enabled: bool,
        net_arch: NetworkArchitechture,
        mac_filter: Option<[u8; 6]>,
        sequence_no_en: bool,
    ) -> Self {
        SEQ_NUM_EN.store(sequence_no_en, core::sync::atomic::Ordering::SeqCst);
        let csi_data_rx = PROC_CSI_DATA.receiver().unwrap();
        Self {
            wifi_config,
            op_mode,
            csi_config,
            traffic_config,
            traffic_enabled,
            net_arch,
            mac_filter,
            sequence_no_en,
            csi_data_rx,
        }
    }

    /// Creates a new CSICollector instance with default configuration.
    /// Device is set to Sniffer mode by default.
    /// Generally you should resort to the `new` method unless you want to only snif packets.
    pub fn new_with_defaults() -> Self {
        SEQ_NUM_EN.store(false, core::sync::atomic::Ordering::SeqCst);
        let proc_csi_data_rx = PROC_CSI_DATA.receiver().unwrap();
        Self {
            wifi_config: WiFiConfig::default(),
            op_mode: WiFiMode::Sniffer,
            csi_config: CSIConfig::default(),
            traffic_config: TrafficConfig::default(),
            traffic_enabled: false,
            net_arch: NetworkArchitechture::Sniffer,
            mac_filter: None,
            sequence_no_en: false,
            csi_data_rx: proc_csi_data_rx,
        }
    }

    /// Starts CSI Collection
    /// Everytime this method is called, the collection restarts with the current configuration/profile without the need to initialize again.
    /// `collection_time_secs` is an optional parameter to define the collection duration in seconds.
    /// If `None` is provided, the collection will continue indefinitely until the device is reset or powered off.
    pub fn start(&self, collection_time_secs: Option<u64>) {
        // In this method everytime the collection starts, the new configurations are loaded and propagated to global context.
        // All settings can be copied from new configuration and loaded to global, without needing to initalize again.
        // Initialization should be restriced to setting up hardware an spawning the network tasks.
        // Everything else should be done here.

        // Example use case, if any setting is updated (Ex. SSID or Password), I would be able to load new config on collection restart

        // Load configurations into global context
        COLLECTION_CONFIG.lock(|c| c.borrow_mut().replace(self.csi_config.clone()));
        NETWORK_CONFIG.lock(|c| *c.borrow_mut() = self.net_arch.clone());
        OPERATION_MODE.lock(|op| *op.borrow_mut() = self.op_mode.clone());

        match self.op_mode {
            WiFiMode::AccessPoint => {
                // Capture SSID and Password from provided configuration
                let ap_ssid = self.wifi_config.ap_ssid.clone();
                let ap_password = self.wifi_config.ap_password.clone();
                let channel = self.wifi_config.channel.clone();
                let max_connections = self.wifi_config.max_connections.clone();
                let hide_ssid = self.wifi_config.ssid_hidden.clone();

                // Access Point Configuration
                let ap_config = Configuration::AccessPoint(AccessPointConfiguration {
                    ssid: ap_ssid.as_str().try_into().unwrap(),
                    auth_method: esp_wifi::wifi::AuthMethod::WPA2Personal,
                    password: ap_password.as_str().try_into().unwrap(),
                    max_connections: max_connections,
                    channel: channel,
                    ssid_hidden: hide_ssid,
                    ..Default::default()
                });

                // Store AP Controller Configuration in Global Context
                CONTROLLER_CONFIG.lock(|c| c.borrow_mut().replace(ap_config));
            }
            WiFiMode::AccessPointStation => {
                // Capture Access Point SSID and Password from provided configuration
                let ap_ssid = self.wifi_config.ap_ssid.clone();
                let ap_password = self.wifi_config.ap_password.clone();
                let channel = self.wifi_config.channel.clone();
                let max_connections = self.wifi_config.max_connections.clone();
                let hide_ssid = self.wifi_config.ssid_hidden.clone();

                // Capture Station SSID and Password from provided configuration
                let ssid = self.wifi_config.ssid.clone();
                let password = self.wifi_config.password.clone();

                // AP/STA Configuration
                let config = Configuration::Mixed(
                    ClientConfiguration {
                        ssid: ssid.as_str().try_into().unwrap(),
                        password: password.as_str().try_into().unwrap(),
                        channel: Some(self.wifi_config.channel),
                        auth_method: esp_wifi::wifi::AuthMethod::WPA2Personal,
                        ..Default::default()
                    },
                    AccessPointConfiguration {
                        ssid: ap_ssid.as_str().try_into().unwrap(),
                        auth_method: esp_wifi::wifi::AuthMethod::WPA2Personal,
                        password: ap_password.as_str().try_into().unwrap(),
                        max_connections: max_connections,
                        channel: channel,
                        ssid_hidden: hide_ssid,
                        ..Default::default()
                    },
                );

                // Store Station Controller Configuration in Global Context
                CONTROLLER_CONFIG.lock(|c| c.borrow_mut().replace(config));
            }
            WiFiMode::Station => {
                // Capture SSID and Password from provided configuration
                let ssid = self.wifi_config.ssid.clone();
                let password = self.wifi_config.password.clone();

                // Station Configuration
                let config = Configuration::Client(ClientConfiguration {
                    ssid: ssid.as_str().try_into().unwrap(),
                    password: password.as_str().try_into().unwrap(),
                    channel: Some(self.wifi_config.channel),
                    auth_method: esp_wifi::wifi::AuthMethod::WPA2Personal,
                    ..Default::default()
                });

                // Store Station Controller Configuration in Global Context
                CONTROLLER_CONFIG.lock(|c| c.borrow_mut().replace(config));
            }
            _ => {}
        }
        // Reset DHCP complete signal to indicate new connection
        DHCP_COMPLETE.reset();
        // Send signal to connection to start collection for defined time
        START_COLLECTION.signal(collection_time_secs);
    }

    /// Retrieve the latest available CSI data packet
    pub async fn get_csi_data(&mut self) -> CSIDataPacket {
        // Wait for CSI data packet to update
        self.csi_data_rx.changed().await
    }

    /// Print the latest CSI data with metadata to console
    /// Optionally pass current time instant to calculate DateTimeCapture if available
    pub async fn print_csi_w_metadata(&mut self) {
        // Wait for CSI data packet to update
        let proc_csi_data = self.csi_data_rx.changed().await;

        // Print the CSI data to console
        proc_csi_data.print_csi_w_metadata();
    }

    /// Sets the WiFi configuration for CSI collection
    pub fn set_wifi_config(&mut self, wifi_config: WiFiConfig) {
        self.wifi_config = wifi_config;
    }

    /// Sets the WiFi operation mode for CSI collection (Sniffer, Station, Access Point, etc.)
    pub fn set_op_mode(&mut self, op_mode: WiFiMode) {
        self.op_mode = op_mode;
    }

    /// Sets the CSI configuration for CSI collection
    pub fn set_csi_config(&mut self, csi_config: CSIConfig) {
        self.csi_config = csi_config;
    }

    /// Sets the traffic configuration for CSI collection
    pub fn set_traffic_config(&mut self, traffic_config: TrafficConfig) {
        self.traffic_config = traffic_config;
    }

    /// Enables or disables traffic generation
    pub fn set_traffic_enabled(&mut self, traffic_enabled: bool) {
        self.traffic_enabled = traffic_enabled;
    }

    /// Sets the network architechture for CSI collection
    pub fn set_net_arch(&mut self, net_arch: NetworkArchitechture) {
        self.net_arch = net_arch;
    }

    /// Initializes the CSI Collection System. This method starts the WiFi connection and Spawns the required tasks.
    pub fn init(
        &self,
        controller: WifiController<'static>,
        interface: Interfaces<'static>,
        seed: u64,
        spawner: &Spawner,
    ) -> Result<()> {
        // Validate configuration
        self.validate()?;
        println!("Configuration Validated");

        // Spawn the CSI processing task
        spawner.spawn(process_csi_packet()).ok();

        // Instantiate Network Stack
        match self.op_mode {
            WiFiMode::AccessPoint => {
                // Create gateway IP address instance
                // This config doesnt get an IP address from router but runs DHCP server
                let gw_ip_addr_str = "192.168.2.1";
                let gw_ip_addr =
                    Ipv4Addr::from_str(gw_ip_addr_str).expect("failed to parse gateway ip");

                // Access Point IP Configuration
                let ap_ip_config = embassy_net::Config::ipv4_static(StaticConfigV4 {
                    address: Ipv4Cidr::new(gw_ip_addr, 24),
                    gateway: Some(gw_ip_addr),
                    dns_servers: Default::default(),
                });

                // Create Network Stack
                let (ap_stack, ap_runner) = embassy_net::new(
                    interface.ap,
                    // wifi_interface.ap,
                    ap_ip_config,
                    mk_static!(StackResources<6>, StackResources::<6>::new()),
                    seed,
                );

                // Spawn controller, runner, and DHCP server tasks
                spawner.spawn(net_task(ap_runner)).ok();
                spawner.spawn(run_dhcp(ap_stack, gw_ip_addr_str)).ok();
                spawner.spawn(connection(controller, self.mac_filter)).ok();
                // spawner.spawn(ap_stack_task(ap_stack)).ok();
            }

            // Station Mode
            WiFiMode::Station => {
                // Configure Station DHCP Client
                let sta_config = embassy_net::Config::dhcpv4(Default::default());

                // Create Network Stack
                let (sta_stack, sta_runner) = embassy_net::new(
                    // wifi_interface.sta,
                    interface.sta,
                    sta_config,
                    mk_static!(StackResources<6>, StackResources::<6>::new()),
                    seed,
                );

                // Spawn sequence number synchronization task if sequence number generation enabled
                if SEQ_NUM_EN.load(core::sync::atomic::Ordering::SeqCst) {
                    spawner.spawn(sequence_sync_task(interface.sniffer)).ok();
                }

                // Spawn Connection and Network Stack Polling Tasks
                spawner.spawn(connection(controller, self.mac_filter)).ok();
                spawner.spawn(net_task(sta_runner)).ok();
                spawner
                    .spawn(sta_stack_task(
                        sta_stack,
                        self.net_arch,
                        self.traffic_enabled,
                        self.traffic_config.traffic_interval_ms,
                        self.traffic_config.traffic_type.clone(),
                    ))
                    .ok();
            }
            // Access Point + Station Mode
            WiFiMode::AccessPointStation => {
                // Gets IP address from router and also runs DHCP server

                // IP Configuration

                // Create gateway IP address instance
                // This config doesnt get an IP address from router but runs DHCP server for AP
                let gw_ip_addr_str = "192.168.2.1";
                let gw_ip_addr =
                    Ipv4Addr::from_str(gw_ip_addr_str).expect("failed to parse gateway ip");

                // Access Point IP Configuration
                let ap_ip_config = embassy_net::Config::ipv4_static(StaticConfigV4 {
                    address: Ipv4Cidr::new(gw_ip_addr, 24),
                    gateway: Some(gw_ip_addr),
                    dns_servers: Default::default(),
                });

                // Station (DHCP) IP Configuration
                let sta_ip_config = embassy_net::Config::dhcpv4(Default::default());

                // Init network stacks
                let (ap_stack, ap_runner) = embassy_net::new(
                    interface.ap,
                    // wifi_interface.ap,
                    ap_ip_config,
                    mk_static!(StackResources<3>, StackResources::<3>::new()),
                    seed,
                );
                let (sta_stack, sta_runner) = embassy_net::new(
                    // wifi_interface.sta,
                    interface.sta,
                    sta_ip_config,
                    mk_static!(StackResources<6>, StackResources::<6>::new()),
                    seed,
                );

                // Spawn controller, runner, and DHCP server tasks
                spawner.spawn(connection(controller, self.mac_filter)).ok();
                spawner.spawn(net_task(ap_runner)).ok();
                spawner.spawn(net_task(sta_runner)).ok();
                spawner.spawn(run_dhcp(ap_stack, gw_ip_addr_str)).ok();
                spawner
                    .spawn(sta_stack_task(
                        sta_stack,
                        self.net_arch,
                        self.traffic_enabled,
                        self.traffic_config.traffic_interval_ms,
                        self.traffic_config.traffic_type.clone(),
                    ))
                    .ok();
            }
            // Sniffer Mode
            WiFiMode::Sniffer => {
                println!("Starting Sniffer");
                // Create sniffer instance
                let sniffer = interface.sniffer;
                sniffer.set_promiscuous_mode(true).unwrap();

                // Spawn controller to start sniffing and initiate CSI callback
                spawner.spawn(connection(controller, self.mac_filter)).ok();
            }
        }
        Ok(())
    }

    /// Validates the CSI Collection configuration/profile
    pub fn validate(&self) -> crate::Result<()> {
        // Validate WiFi configuration
        if self.wifi_config.channel < 1 || self.wifi_config.channel > 14 {
            return Err(crate::Error::ConfigError("Invalid WiFi channel"));
        }

        if self.wifi_config.max_retries > 10 {
            println!(
                "High number of WiFi retries configured: {}",
                self.wifi_config.max_retries
            );
        }

        if self.wifi_config.timeout_secs < 1 {
            return Err(crate::Error::ConfigError("WiFi timeout must be positive"));
        }

        if self.wifi_config.max_connections < 1 {
            return Err(crate::Error::ConfigError(
                "Maximum connections must be positive",
            ));
        }

        if self.traffic_config.traffic_interval_ms < 1 {
            return Err(crate::Error::ConfigError(
                "Traffic interval must be positive",
            ));
        }

        if self.traffic_enabled && self.sequence_no_en {
            return Err(crate::Error::ConfigError(
                "Cannot enable both traffic generation with sequence number capture",
            ));
        }

        if self.sequence_no_en && self.mac_filter.is_none() {
            println!("CAUTION: MAC Filter not Configured while Sequence Number Capture Enabled");
        }

        Ok(())
    }
}

// Embassy Tasks
#[embassy_executor::task]
async fn sequence_sync_task(mut sniffer: Sniffer) {
    sniffer.set_promiscuous_mode(true).unwrap();
    sniffer.set_receive_cb(|packet| {
        // Check if recieved packet is a data packet
        let _ = match_frames! {
            packet.data,
            data = DataFrame => {
                if let Some(payload) = &data.payload{
                    // Extract sequence number & save to global context
                    if let Some(seq_num) = extract_icmp_sequence(payload) {
                        // println!("Extracted seq_num: {} at timestamp: {}", seq_num, packet.rx_cntl.timestamp);
                        LAST_SEQ_NUM.store(seq_num, core::sync::atomic::Ordering::Relaxed);
                        LAST_TIMESTAMP.store(packet.rx_cntl.timestamp, core::sync::atomic::Ordering::Relaxed);
                    }
                }
            }
        };
    });
}

#[embassy_executor::task]
async fn connection(mut controller: WifiController<'static>, mac_filter: Option<[u8; 6]>) {
    loop {
        // Wait for the START_COLLECTION signal
        let run_duration_secs = START_COLLECTION.wait().await;

        // Get configurations from global context
        let csi_config = COLLECTION_CONFIG.lock(|c| c.borrow().as_ref().unwrap().clone());
        let net_arch = NETWORK_CONFIG.lock(|na| na.borrow().clone());
        let wifi_mode = OPERATION_MODE.lock(|om| om.borrow().clone());

        println!("Starting CSI Collection!");

        let ap_events = enum_set!(
            WifiEvent::ApStaconnected
                | WifiEvent::ApStadisconnected
                | WifiEvent::ApStop
                | WifiEvent::StaDisconnected
                | WifiEvent::StaStop
        );

        // Inner loop to handle WiFi state
        loop {
            let csi_cfg = build_csi_config(csi_config.clone());
            if matches!(wifi_mode, WiFiMode::Sniffer) {
                println!("Enabling CSI collection");
                controller
                    .set_csi(csi_cfg, |info: esp_wifi::wifi::wifi_csi_info_t| {
                        capture_csi_info(info, mac_filter);
                    })
                    .unwrap();

                // Wait for duration or until stopped
                if let Some(duration) = run_duration_secs {
                    Timer::after(Duration::from_secs(duration)).await;
                    println!("CSI Collection Concluded");
                    START_COLLECTION.reset();
                    break;
                }

                loop {
                    Timer::after(Duration::from_secs(1)).await;
                }
            } else {
                match esp_wifi::wifi::wifi_state() {
                    WifiState::ApStarted | WifiState::StaConnected => {
                        // For Station or AP+Station modes, ensure DHCP is complete before enabling CSI
                        if matches!(wifi_mode, WiFiMode::Station | WiFiMode::AccessPointStation) {
                            println!("Waiting for DHCP to complete...");
                            match with_timeout(Duration::from_secs(30), DHCP_COMPLETE.wait()).await
                            {
                                Ok(()) => println!("DHCP complete, enabling CSI collection."),
                                Err(_) => {
                                    println!("DHCP timed out, skipping CSI collection.");
                                    START_COLLECTION.reset();
                                    break;
                                }
                            }
                        }

                        // Set CSI Callback for Station or AP+Station modes
                        if !matches!(
                            wifi_mode,
                            WiFiMode::AccessPoint | WiFiMode::AccessPointStation
                        ) {
                            println!("Enabling CSI collection");
                            controller
                                .set_csi(csi_cfg, |info: esp_wifi::wifi::wifi_csi_info_t| {
                                    capture_csi_info(info, mac_filter);
                                })
                                .unwrap();
                        }

                        let run_loop = async {
                            loop {
                                let mut event = controller.wait_for_events(ap_events, true).await;
                                if event.contains(WifiEvent::ApStaconnected) {
                                    println!("New STA Connected");
                                }
                                if event.contains(WifiEvent::ApStadisconnected) {
                                    println!("STA Disconnected");
                                }
                                if event.contains(WifiEvent::ApStop) {
                                    println!("AP connection stopped. Collection halted.");
                                    Timer::after(Duration::from_millis(5000)).await;
                                    return Ok::<(), ()>(());
                                }
                                if event.contains(WifiEvent::StaDisconnected) {
                                    println!("STA disconnected");
                                    Timer::after(Duration::from_millis(5000)).await;
                                    return Ok(());
                                }
                                if event.contains(WifiEvent::StaStop) {
                                    println!("STA connection stopped. Collection halted.");
                                    Timer::after(Duration::from_millis(5000)).await;
                                    return Ok(());
                                }
                                event.clear();
                            }
                        };

                        let should_stop = match run_duration_secs {
                            Some(duration) => {
                                let conn_res =
                                    with_timeout(Duration::from_secs(duration), run_loop).await;
                                match conn_res {
                                    Ok(_) => {
                                        println!(
                                            "CSI Collection Concluded. Stopping Controller..."
                                        );
                                        true
                                    }
                                    Err(_) => {
                                        println!(
                                            "CSI Collection Timed Out. Stopping Controller..."
                                        );
                                        controller.disconnect_async().await.unwrap();
                                        controller.stop_async().await.unwrap();
                                        true
                                    }
                                }
                            }
                            None => {
                                let _ = run_loop.await;
                                println!("CSI Collection Stopped. Attempting to Restart...");
                                false
                            }
                        };

                        START_COLLECTION.reset();
                        if should_stop {
                            if controller.is_started().unwrap_or(false) {
                                controller.disconnect_async().await.unwrap();
                                controller.stop_async().await.unwrap();
                            }
                            break;
                        }
                    }
                    _ => {
                        if !matches!(wifi_mode, WiFiMode::Sniffer) {
                            let config =
                                CONTROLLER_CONFIG.lock(|c| c.borrow().as_ref().unwrap().clone());
                            match controller.set_configuration(&config) {
                                Ok(_) => println!("WiFi Configuration Set: {:?}", config),
                                Err(_) => {
                                    println!("WiFi Configuration Error");
                                    println!("Error Config: {:?}", config);
                                }
                            }
                        }

                        println!("Starting WiFi");
                        controller.start_async().await.unwrap();
                        println!("Wifi Started!");

                        if matches!(wifi_mode, WiFiMode::Station | WiFiMode::AccessPointStation) {
                            for attempt in 1..=3 {
                                println!("Connecting (attempt {}/{})...", attempt, 3);
                                match controller.connect_async().await {
                                    Ok(_) => {
                                        println!("Connected!");
                                        break;
                                    }
                                    Err(e) => {
                                        println!("Connection attempt {} failed: {:?}", attempt, e);
                                        if attempt < 3 {
                                            println!("Trying again...");
                                            Timer::after(Duration::from_millis(3000)).await;
                                        } else {
                                            println!("All connection attempts failed.");
                                        }
                                    }
                                }
                            }
                        }
                        if matches!(
                            net_arch,
                            NetworkArchitechture::RouterStation
                                | NetworkArchitechture::RouterAccessPointStation
                        ) {
                            let mut first_print = true;
                            while !DATE_TIME_VALID.load(core::sync::atomic::Ordering::Relaxed) {
                                if first_print {
                                    println!("Waiting for valid NTP time...");
                                    first_print = false;
                                } else {
                                    #[cfg(not(feature = "defmt"))]
                                    {
                                        print!(".");
                                    }
                                }
                                Timer::after(Duration::from_millis(500)).await;
                            }
                            if !first_print {
                                println!("");
                            }
                        } else {
                            #[cfg(not(feature = "defmt"))]
                            {
                                println!(
                                    "NTP time not supported for network architecture: {:?}",
                                    net_arch
                                );
                            }
                        }
                    }
                }
            }
        }
    }
}

// Placeholder for future use if an AP stack is required to process packets
// #[embassy_executor::task]
// async fn ap_stack_task(ap_stack: Stack<'static>) {
//     let mut rx_buffer = [0; 1024];
//     let mut tx_buffer = [0; 1024];
//     let mut rx_meta: [PacketMetadata; 128] = [PacketMetadata::EMPTY; 128];
//     let mut tx_meta: [PacketMetadata; 128] = [PacketMetadata::EMPTY; 128];

//     let mut socket = UdpSocket::new(
//         ap_stack,
//         &mut rx_meta,
//         &mut rx_buffer,
//         &mut tx_meta,
//         &mut tx_buffer,
//     );

//     println!("Binding");

//     socket.bind(10987).unwrap();

//     let mut buf = [0u8; 512];

//     loop {
//         // Wait to receive a packet
//         let (n, sender_endpoint) = socket.recv_from(&mut buf).await.unwrap();

//         println!("Received {} bytes from {:?}", n, sender_endpoint);
//         println!("Received array: {:?}", buf);
//         buf.fill(0);
//     }
// }

#[embassy_executor::task]
async fn sta_stack_task(
    sta_stack: Stack<'static>,
    net_arch: NetworkArchitechture,
    traffic_en: bool,
    traffic_interval: u64,
    traffic_type: TrafficType,
) {
    println!("STA Stack Task Running");

    // Acquire and store IP information for gateway and client after configuration is up

    // Check if link is up
    sta_stack.wait_link_up().await;
    println!("Link is up!");

    // Create instance to store acquired IP information
    let mut ip_info = IpInfo {
        local_address: Ipv4Cidr::new(Ipv4Addr::UNSPECIFIED, 24),
        gateway_address: Ipv4Address::UNSPECIFIED,
    };

    println!("Acquiring config...");
    sta_stack.wait_config_up().await;
    println!("Config Acquired");

    // Signal that DHCP is complete
    DHCP_COMPLETE.signal(());

    // Print out acquired IP configuration
    loop {
        if let Some(config) = sta_stack.config_v4() {
            ip_info.local_address = config.address;
            ip_info.gateway_address = config.gateway.unwrap();

            #[cfg(feature = "defmt")]
            {
                info!("Local IP: {:?}", ip_info.local_address);
                info!("Gateway IP: {:?}", ip_info.gateway_address);
            }

            #[cfg(not(feature = "defmt"))]
            {
                println!("Local IP: {:?}", ip_info.local_address);
                println!("Gateway IP: {:?}", ip_info.gateway_address);
            }

            break;
        }
        Timer::after(Duration::from_millis(500)).await;
    }

    // Check if selected network architechture supports NTP time stamp retrieval
    // Capturing time is supported only by architechtures that connect to a commercial router with an internet connection
    // These are RouterStation and RouterAccessPointStation
    match net_arch {
        NetworkArchitechture::RouterStation | NetworkArchitechture::RouterAccessPointStation => {
            // Get Current SNTP unix time values
            match get_sntp_time(sta_stack).await {
                Ok((seconds, milliseconds)) => {
                    // Convert captured time to date/time values
                    let time_capture = unix_to_date_time(seconds.into(), milliseconds);

                    // Print the time captured for validation
                    println!(
                        "Time: {:04}-{:02}-{:02} {:02}:{:02}:{:02}.{:03}",
                        time_capture.0,
                        time_capture.1,
                        time_capture.2,
                        time_capture.3,
                        time_capture.4,
                        time_capture.5,
                        time_capture.6
                    );

                    // Store the captured time instant values to DateTimeCapture struct
                    let time = DateTimeCapture {
                        captured_at: Instant::now(),
                        captured_secs: seconds as u64,
                        captured_millis: milliseconds,
                    };

                    // Move DateTimeCapture struct to Global Context
                    match DATE_TIME.init(time) {
                        Ok(_) => {
                            println!("Time Captured");
                            DATE_TIME_VALID.store(true, core::sync::atomic::Ordering::Relaxed);
                        }
                        Err(_) => {
                            println!("Failed to Capture Time");
                            DATE_TIME_VALID.store(false, core::sync::atomic::Ordering::Relaxed);
                        }
                    }
                }
                Err(_) => {
                    println!("Failed to get SNTP time, Proceeding with default.");
                    DATE_TIME_VALID.store(false, core::sync::atomic::Ordering::Relaxed);
                }
            }
        }
        NetworkArchitechture::AccessPointStation | NetworkArchitechture::Sniffer => {
            // Do Nothing, No Connection to Internet to Sync Time
            println!("NTP time not captured. Configured architechture does not support.");
        }
    }
    // ------------------ UDP Socket Setup ------------------
    let mut udp_rx_buffer = [0; 1024];
    let mut udp_tx_buffer = [0; 1024];
    let mut udp_rx_meta: [PacketMetadata; 8] = [PacketMetadata::EMPTY; 8];
    let mut udp_tx_meta: [PacketMetadata; 8] = [PacketMetadata::EMPTY; 8];

    let mut socket = UdpSocket::new(
        sta_stack,
        &mut udp_rx_meta,
        &mut udp_rx_buffer,
        &mut udp_tx_meta,
        &mut udp_tx_buffer,
    );

    println!("Binding");

    // Bind to a local port
    socket.bind(10987).unwrap();

    // Send to some unreachable port to trigger response
    // Using same as local port number
    let endpoint = IpEndpoint::new(embassy_net::IpAddress::Ipv4(ip_info.gateway_address), 10987);

    // ------------------ ICMP Socket Setup ------------------
    let mut rx_buffer = [0; 64];
    let mut tx_buffer = [0; 64];
    let mut rx_meta: [RawPacketMetadata; 1] = [RawPacketMetadata::EMPTY; 1];
    let mut tx_meta: [RawPacketMetadata; 1] = [RawPacketMetadata::EMPTY; 1];

    let raw_socket = RawSocket::new::<WifiDevice<'_>>(
        sta_stack,
        IpVersion::Ipv4,
        IpProtocol::Icmp,
        &mut rx_meta,
        &mut rx_buffer,
        &mut tx_meta,
        &mut tx_buffer,
    );

    // Buffer to hold ICMP Packet
    let mut icmp_buffer = [0u8; 12];

    // Create ICMP Packet
    let mut icmp_packet = Icmpv4Packet::new_unchecked(&mut icmp_buffer[..]);

    // Create an ICMPv4 Echo Request
    let icmp_repr = Icmpv4Repr::EchoRequest {
        ident: 0x22b,
        seq_no: 0,
        data: &[0xDE, 0xAD, 0xBE, 0xEF],
    };

    // Serialize the ICMP representation into the packet
    icmp_repr.emit(&mut icmp_packet, &ChecksumCapabilities::default());

    // Buffer for the full IPv4 packet
    let mut tx_ipv4_buffer = [0u8; 64];

    // Define the IPv4 representation
    let ipv4_repr = Ipv4Repr {
        src_addr: ip_info.local_address.address(),
        dst_addr: ip_info.gateway_address,
        payload_len: icmp_repr.buffer_len(),
        hop_limit: 64, // Time-to-live value
        next_header: IpProtocol::Icmp,
    };

    // Create the IPv4 packet
    let mut ipv4_packet = Ipv4Packet::new_unchecked(&mut tx_ipv4_buffer);

    // Serialize the IPv4 representation into the packet
    ipv4_repr.emit(&mut ipv4_packet, &ChecksumCapabilities::default());

    // Copy the ICMP packet into the IPv4 packet's payload
    ipv4_packet
        .payload_mut()
        .copy_from_slice(icmp_packet.into_inner());

    // IP Packet buffer that will be sent or recieved
    let ipv4_packet_buffer = ipv4_packet.into_inner();

    // Create a CSI_DATA Watch Reciever to monitor changes
    let mut proc_csi_data_rx = PROC_CSI_DATA.receiver().unwrap();

    // Check if traffic generation is enabled
    // If not enabled, then we assume that CSI data is not required at the Station
    // Instead will respond to any recieved traffic by echoing CSI data back to sender over UDP
    if traffic_en {
        println!("Starting Traffic Generation");
        // Match type of traffic
        match traffic_type {
            TrafficType::UDP => {
                loop {
                    // Send a random byte to gateway
                    println!("Pinging Gateway");
                    socket.send_to(&[13], endpoint).await.unwrap();
                    // Wait for user specified duration
                    Timer::after(Duration::from_millis(traffic_interval)).await;
                }
            }
            TrafficType::ICMPPing => {
                loop {
                    // Send raw packet
                    raw_socket.send(ipv4_packet_buffer).await;

                    // Wait for user specified duration
                    Timer::after(Duration::from_millis(traffic_interval)).await;
                }
            }
        }
    } else {
        loop {
            // Wait for CSI data packet to update
            let proc_csi_data = proc_csi_data_rx.changed().await;

            // Create a message buffer for the data to be sent back

            // Message format w/ seq_no:
            // [0..1]   : 2 bytes seq_no (u16) - big endian
            // [2]      : 1 byte for CSI data format (mapping below)
            // [2..7]   : 4 bytes timestamp (u32) - big endian
            // [7..n]   : n-6 bytes CSI data (i8)

            // Width of message (619) = 2 bytes for seq_no + 1 byte for format + 4 bytes for timestamp + 612 bytes for CSI data
            let mut message_u8: Vec<u8, 619> = Vec::new();

            // CSI is captured in a callback that does not have access to the ICMP sequence number
            // The CSI callback, however, does have access to the timestamp of the packet
            // So we use a global context to store the last captured ICMP sequence number and timestamp
            // We use timestamp to match the CSI to the ICMP sequence number

            // Append the sequence number to the message, if enabled
            if SEQ_NUM_EN.load(core::sync::atomic::Ordering::Relaxed) {
                message_u8
                    .extend_from_slice(&proc_csi_data.sequence_number.to_be_bytes())
                    .unwrap();
            } else {
                message_u8.extend_from_slice(&0_u16.to_be_bytes()).unwrap();
            }

            // Append the data format to the message
            message_u8.push(proc_csi_data.data_format as u8).unwrap();

            // Append the timestamp to the message
            message_u8
                .extend_from_slice(&proc_csi_data.timestamp.to_be_bytes())
                .unwrap();

            // Append the CSI data to the message
            for x in proc_csi_data.csi_data.iter() {
                message_u8.push(*x as u8).unwrap();
            }

            // Send back to sender if sequence number is not zero
            if proc_csi_data.sequence_number != 0 {
                socket.send_to(&message_u8, endpoint).await.unwrap();
            }
        }
    }
}

#[embassy_executor::task(pool_size = 2)]
pub async fn net_task(mut runner: Runner<'static, WifiDevice<'static>>) {
    println!("Network Task Running");
    runner.run().await
}

#[embassy_executor::task]
async fn run_dhcp(stack: Stack<'static>, gw_ip_addr: &'static str) {
    use core::net::{Ipv4Addr, SocketAddrV4};

    use edge_dhcp::{
        io::{self, DEFAULT_SERVER_PORT},
        server::{Server, ServerOptions},
    };
    use edge_nal::UdpBind;
    use edge_nal_embassy::{Udp, UdpBuffers};

    let ip = Ipv4Addr::from_str(gw_ip_addr).expect("DHCP task failed to parse gateway ip");

    let mut buf = [0u8; 1500];

    let mut gw_buf = [Ipv4Addr::UNSPECIFIED];

    let buffers = UdpBuffers::<3, 1024, 1024, 10>::new();
    let unbound_socket = Udp::new(stack, &buffers);
    let mut bound_socket = unbound_socket
        .bind(core::net::SocketAddr::V4(SocketAddrV4::new(
            Ipv4Addr::UNSPECIFIED,
            DEFAULT_SERVER_PORT,
        )))
        .await
        .unwrap();

    println!("DHCP Server Running");
    let mut server = Server::<_, 64>::new_with_et(ip);
    loop {
        _ = io::server::run(
            &mut server,
            &ServerOptions::new(ip, Some(&mut gw_buf)),
            &mut bound_socket,
            &mut buf,
        )
        .await
        .inspect_err(|_e| println!("DHCP Server Error"));
        println!("DHCP Buffer: {:?}", buf);
        Timer::after(Duration::from_millis(500)).await;
    }
}

// Function to process the CSI info
#[embassy_executor::task]
pub async fn process_csi_packet() {
    // Subscribe to CSI packet capture updates
    let mut csi_packet_sub = CSI_PACKET.subscriber().unwrap();
    let proc_csi_packet_sender = PROC_CSI_DATA.sender();

    // Loop that will process CSI data as soon as it arrives
    loop {
        // Get the unprocessed CSI data packet from the channel
        let mut csi_packet = csi_packet_sub.next_message_pure().await;

        // Update the CSI data format
        #[cfg(not(feature = "esp32c6"))]
        {
            csi_packet.csi_fmt_from_params();
        }

        // Process Date/Time if Date Time is valid/supported
        if DATE_TIME_VALID.load(core::sync::atomic::Ordering::Relaxed) {
            let dt_cap = DATE_TIME.get().await;
            let elapsed_time = Instant::now()
                .checked_duration_since(dt_cap.captured_at)
                .unwrap_or(Duration::from_secs(0));
            // Add seconds and adjust for overflow from milliseconds
            let total_time_secs = dt_cap.captured_secs + elapsed_time.as_secs();

            // Add milliseconds and adjust if they exceed 1000
            let total_millis = dt_cap.captured_millis + elapsed_time.as_millis();
            let extra_secs = total_millis / 1000; // 1000ms = 1 second
            let final_millis = total_millis % 1000; // Remainder in milliseconds

            // Add extra seconds from milliseconds overflow to total seconds
            let total_time_secs = total_time_secs + extra_secs;

            // Now call the date-time conversion function
            let (year, month, day, hour, minute, second, millisecond) =
                unix_to_date_time(total_time_secs, final_millis);

            let dt = DateTime {
                year,
                month,
                day,
                hour,
                minute,
                second,
                millisecond,
            };

            csi_packet.date_time = Some(dt);
        }
        // Capture Sequence Number if Enabled
        if SEQ_NUM_EN.load(core::sync::atomic::Ordering::SeqCst) {
            // Get the last captured ICMP sequence number and timestamp from global context
            let sequence_no = LAST_SEQ_NUM.load(core::sync::atomic::Ordering::Relaxed);
            let icmp_timestamp = LAST_TIMESTAMP.load(core::sync::atomic::Ordering::Relaxed);

            // println!("ICMP Timestamp: {:?}", icmp_timestamp);
            // println!("ICMP Sequence Number: {:?}", sequence_no);
            // println!("CSI Timestamp {:?}", csi_packet.timestamp);

            // Calculate the absolute difference between the timestamps.
            let timestamp_diff = icmp_timestamp.abs_diff(csi_packet.timestamp);

            // If timestamps are within a tolerance window of 1000us, then update the sequence number.
            if timestamp_diff <= 4000 {
                csi_packet.sequence_number = sequence_no;
            } else {
                // Print mistmatch (debiug)
                // println!("Timestamp mismatch! Diff: {} us. CSI: {}, ICMP: {}", timestamp_diff, csi_packet.timestamp, icmp_timestamp);
                csi_packet.sequence_number = 0;
            }
        }

        // Update the Watch with the processed CSI
        proc_csi_packet_sender.send(csi_packet);
    }

    // return the CSI as a &[u8]?
}

// Function to get timestamp from NTP
pub async fn get_sntp_time(stack: Stack<'_>) -> Result<(u32, u64)> {
    let mut rx_buffer = [0; 1024];
    let mut tx_buffer = [0; 1024];
    let mut rx_meta: [PacketMetadata; 128] = [PacketMetadata::EMPTY; 128];
    let mut tx_meta: [PacketMetadata; 128] = [PacketMetadata::EMPTY; 128];

    let mut sntp_socket = UdpSocket::new(
        stack,
        &mut rx_meta,
        &mut rx_buffer,
        &mut tx_meta,
        &mut tx_buffer,
    );

    sntp_socket.bind(12345).unwrap();

    let mut sntp_packet = [0u8; 48];
    sntp_packet[0] = 0x23; // LI = 0, VN = 4, Mode = 3

    let Ok(ntp_server_addrs) = stack
        .dns_query(NTP_SERVER, smoltcp::wire::DnsQueryType::A)
        .await
    else {
        return Err(error::Error::ConfigError(""));
    };

    // Find the first IPv4 address in the result
    let Some(ntp_server_addr) = ntp_server_addrs.iter().find_map(|addr| match addr {
        IpAddress::Ipv4(ip) => Some(*ip), // Return the first IPv4 address
        _ => None,
    }) else {
        return Err(error::Error::ConfigError(""));
    };

    let Ok(_) = sntp_socket
        .send_to(&sntp_packet, (ntp_server_addr, NTP_PORT))
        .await
    else {
        return Err(error::Error::ConfigError(""));
    };

    let mut ntp_response = [0_u8; 48];
    let Ok((len, _)) = sntp_socket.recv_from(&mut ntp_response).await else {
        println!("Receive failed");
        return Err(error::Error::ConfigError(""));
    };

    if len < 48 {
        return Err(crate::Error::SystemError("Incomplete NTP response"));
    }

    let transmit_secs = u32::from_be_bytes(ntp_response[40..44].try_into().unwrap());
    let transmit_frac = u32::from_be_bytes(ntp_response[44..48].try_into().unwrap());

    // Adjust for UNIX epoch
    let unix_seconds = transmit_secs - NTP_UNIX_OFFSET;

    // Convert fractional seconds to milliseconds
    let milliseconds = ((transmit_frac as u64) * 1000) >> 32;

    Ok((unix_seconds, milliseconds))
}

// Function to extract ICMP sequence number from a raw packet
fn extract_icmp_sequence(payload: &[u8]) -> Option<u16> {
    // Possible offsets
    let offsets = [
        (0, "Direct IP"),
        (2, "QoS header"),
        (8, "LLC/SNAP header"),
        (10, "QoS + LLC/SNAP header"),
        (16, "QoS + padding + LLC/SNAP"),
    ];

    for &(offset, _desc) in offsets.iter() {
        // Check for IP header (minimum 20 bytes)
        if payload.len() < offset + 20 {
            continue;
        }

        let ip_header = &payload[offset..offset + 20];

        // Verify IPv4 (version = 4)
        if ip_header[0] >> 4 != 4 {
            continue;
        }

        // Extract IP header length (IHL in 4-byte words)
        let ip_header_len = (ip_header[0] & 0x0F) as usize * 4;
        if ip_header_len < 20 {
            continue;
        }

        // Check for ICMP header (8 bytes)
        if payload.len() < offset + ip_header_len + 8 {
            continue;
        }

        // Verify ICMP protocol (byte 9 in IP header = 0x01)
        if ip_header[9] != 0x01 {
            continue;
        }

        let icmp_header = &payload[offset + ip_header_len..offset + ip_header_len + 8];

        // Verify ICMP Echo Request (8) or Reply (0)
        if icmp_header[0] != 0 && icmp_header[0] != 8 {
            continue;
        }

        // Extract sequence number (bytes 6-7, big endian)
        let sequence_no = u16::from_be_bytes([icmp_header[6], icmp_header[7]]);
        return Some(sequence_no);
    }
    None
}

// Function to capture CSI info from callback and publish to channel
fn capture_csi_info(info: esp_wifi::wifi::wifi_csi_info_t, mac_filter: Option<[u8; 6]>) {
    // If filter for MAC address is set, no need to proceed if address doesnt match
    if mac_filter.is_some() && info.mac != mac_filter.unwrap() {
        // println!("MAC Address Filtered Out");
        return;
    }

    let rssi = if info.rx_ctrl.rssi() > 127 {
        info.rx_ctrl.rssi() - 256
    } else {
        info.rx_ctrl.rssi()
    };

    let mut csi_data = Vec::<i8, 612>::new();
    let csi_buf = info.buf;
    let csi_buf_len = info.len;
    for data in 0..csi_buf_len {
        unsafe {
            let value = *csi_buf.add(data as usize);
            csi_data.push(value).expect("Exceeded maximum capacity");
        }
    }

    let csi_packet = CSIDataPacket {
        sequence_number: 0,
        data_format: RxCSIFmt::Undefined,
        date_time: None,
        mac: [
            info.mac[0],
            info.mac[1],
            info.mac[2],
            info.mac[3],
            info.mac[4],
            info.mac[5],
        ],
        rssi,
        bandwidth: info.rx_ctrl.cwb(),
        antenna: info.rx_ctrl.ant(),
        rate: info.rx_ctrl.rate(),
        sig_mode: info.rx_ctrl.sig_mode(),
        mcs: info.rx_ctrl.mcs(),
        smoothing: info.rx_ctrl.smoothing(),
        not_sounding: info.rx_ctrl.not_sounding(),
        aggregation: info.rx_ctrl.aggregation(),
        stbc: info.rx_ctrl.stbc(),
        fec_coding: info.rx_ctrl.fec_coding(),
        sgi: info.rx_ctrl.sgi(),
        noise_floor: info.rx_ctrl.noise_floor(),
        ampdu_cnt: info.rx_ctrl.ampdu_cnt(),
        channel: info.rx_ctrl.channel(),
        secondary_channel: info.rx_ctrl.secondary_channel(),
        timestamp: info.rx_ctrl.timestamp(),
        rx_state: info.rx_ctrl.rx_state(),
        sig_len: info.rx_ctrl.sig_len(),
        csi_data_len: csi_buf_len,
        csi_data: csi_data,
    };

    CSI_PACKET.publish_immediate(csi_packet);
}

#[cfg(feature = "esp32c6")]
fn build_csi_config(csi_config: CSIConfig) -> CsiConfig {
    CsiConfig {
        enable: csi_config.enable,
        acquire_csi_legacy: csi_config.acquire_csi_legacy,
        acquire_csi_ht20: csi_config.acquire_csi_ht20,
        acquire_csi_ht40: csi_config.acquire_csi_ht40,
        acquire_csi_su: csi_config.acquire_csi_su,
        acquire_csi_mu: csi_config.acquire_csi_mu,
        acquire_csi_dcm: csi_config.acquire_csi_dcm,
        acquire_csi_beamformed: csi_config.acquire_csi_beamformed,
        acquire_csi_he_stbc: csi_config.acquire_csi_he_stbc,
        val_scale_cfg: csi_config.val_scale_cfg,
        dump_ack_en: csi_config.dump_ack_en,
        reserved: csi_config.reserved,
    }
}

#[cfg(not(feature = "esp32c6"))]
fn build_csi_config(csi_config: CSIConfig) -> CsiConfig {
    CsiConfig {
        lltf_en: csi_config.lltf_enabled,
        htltf_en: csi_config.htltf_enabled,
        stbc_htltf2_en: csi_config.stbc_htltf2_enabled,
        ltf_merge_en: csi_config.ltf_merge_enabled,
        channel_filter_en: csi_config.channel_filter_enabled,
        manu_scale: csi_config.manu_scale,
        shift: csi_config.shift,
        dump_ack_en: csi_config.dump_ack_en,
    }
}
