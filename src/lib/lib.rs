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
//! ### Types of CSI Collection
//! AP, AP/STA, STA, & Sniffer
//!
//!
//!
//! AP and AP/STA modes do not collect CSI locally, they are merely CSI collection enablers for stations. They rely on connected stations to capture CSI data.
//! Explain trigger mode vs listener mode
//!
//! They can operate as external traffic trigger providers for connected stations. The CSI collected at the stations is then propagated back through a UDP message to the trigger source (AP or AP/STA).
//! Alternatively, the  
//!
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

use core::sync::atomic::{AtomicBool, AtomicU16, AtomicU32};
use embassy_sync::pubsub::{PubSubBehavior, PubSubChannel};
use embassy_sync::watch::Watch;
use embassy_time::with_timeout;
use embassy_time::{Duration, Instant, Timer};

#[cfg(feature = "println")]
use esp_println as _;
#[cfg(feature = "println")]
use esp_println::println;

#[cfg(feature = "defmt")]
use defmt::info;
#[cfg(feature = "defmt")]
use defmt::println;

use esp_wifi::wifi::Sniffer;

use embassy_net::{
    udp::{PacketMetadata, UdpSocket},
    IpAddress, Ipv4Address, Ipv4Cidr, Runner, Stack,
};
use esp_alloc as _;
use esp_backtrace as _;
use esp_wifi::wifi::{Configuration, CsiConfig, WifiController, WifiDevice};
use ieee80211::ssid;

use core::{net::Ipv4Addr, str::FromStr};
use embassy_sync::{
    blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel, once_lock::OnceLock,
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

const NTP_UNIX_OFFSET: u32 = 2_208_988_800; // 1900 to 1970 offset in seconds
const NTP_SERVER: &str = "pool.ntp.org";
const NTP_PORT: u16 = 123;

// Global Static Variables

// OnceLocks
static DATE_TIME: OnceLock<DateTimeCapture> = OnceLock::new();

// Atomics
static DATE_TIME_VALID: AtomicBool = AtomicBool::new(false);
static LAST_SEQ_NUM: AtomicU16 = AtomicU16::new(0);
static LAST_TIMESTAMP: AtomicU32 = AtomicU32::new(0);
static SEQ_NUM_EN: AtomicBool = AtomicBool::new(false);

// Mutexes
// static CONTROLLER_CONFIG: Mutex<CriticalSectionRawMutex, RefCell<Option<Configuration>>> =
//     Mutex::new(RefCell::new(None));
// static NETWORK_CONFIG: Mutex<CriticalSectionRawMutex, RefCell<NetworkArchitechture>> =
//     Mutex::new(RefCell::new(NetworkArchitechture::Sniffer));
// static COLLECTION_CONFIG: Mutex<CriticalSectionRawMutex, RefCell<Option<CSIConfig>>> =
//     Mutex::new(RefCell::new(None));
// static OPERATION_MODE: Mutex<CriticalSectionRawMutex, RefCell<WiFiMode>> =
//     Mutex::new(RefCell::new(WiFiMode::Sniffer));

// Watches
static PROC_CSI_DATA: Watch<CriticalSectionRawMutex, CSIDataPacket, 3> = Watch::new();

// Signals
static DHCP_CLIENT_INFO: Signal<CriticalSectionRawMutex, IpInfo> = Signal::new();
static START_COLLECTION: Signal<CriticalSectionRawMutex, bool> = Signal::new();
static CONTROLLER_HALTED_SIGNAL: Signal<CriticalSectionRawMutex, bool> = Signal::new();

// Channels
static CONTROLLER_CH: Channel<CriticalSectionRawMutex, WifiController<'static>, 1> = Channel::new();
static CSI_CONFIG_CH: Channel<CriticalSectionRawMutex, CSIConfig, 1> = Channel::new();
static MAC_FIL_CH: Channel<CriticalSectionRawMutex, Option<[u8; 6]>, 1> = Channel::new();
static CSI_UDP_RAW_CH: Channel<CriticalSectionRawMutex, Vec<u8, 619>, 2> = Channel::new();
// static ICMP_INFO_CH: Channel<CriticalSectionRawMutex, IcmpInfo, 4> = Channel::new();
// static CLIENT_CONFIG_CH: Channel<CriticalSectionRawMutex, ClientConfiguration, 1> = Channel::new();
// static ACCESSPOINT_CONFIG_CH: Channel<CriticalSectionRawMutex, AccessPointConfiguration, 1> =
// Channel::new();

// OnceLocks
static AP_MAC_BSSID: OnceLock<[u8; 6]> = OnceLock::new();

// CSI PubSub Channels
static CSI_PACKET: PubSubChannel<CriticalSectionRawMutex, CSIDataPacket, 4, 1, 1> =
    PubSubChannel::new();

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

#[derive(Debug, Clone)]
enum ConnectionType {
    Client,
    AccessPoint,
    Mixed,
}

// Date Time Struct
#[derive(Debug, Clone)]
struct DateTimeCapture {
    captured_at: Instant,
    captured_secs: u64,
    captured_millis: u64,
}

#[derive(Debug, Clone)]
pub struct DateTime {
    year: u64,
    month: u64,
    day: u64,
    hour: u64,
    minute: u64,
    second: u64,
    millisecond: u64,
}

#[derive(Debug, Clone)]
struct IpInfo {
    pub local_address: Ipv4Cidr,
    pub gateway_address: Ipv4Address,
}

#[derive(Clone, Copy, Debug)]
struct IcmpInfo {
    seq_num: u16,
    timestamp: u32,
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
                    if let Some((seq_num, _src_ip)) = extract_icmp_info(payload) {
                    // if let Some(seq_num) = extract_icmp_sequence(payload) {
                        // println!("Extracted seq_num: {} at timestamp: {}", seq_num, packet.rx_cntl.timestamp);
                        LAST_SEQ_NUM.store(seq_num, core::sync::atomic::Ordering::Relaxed);
                        LAST_TIMESTAMP.store(packet.rx_cntl.timestamp, core::sync::atomic::Ordering::Relaxed);
                        // Create the info packet
                        // let info = IcmpInfo {
                        //     seq_num,
                        //     timestamp: packet.rx_cntl.timestamp,
                        // };

                        // Send to channel, non-blocking
                        // This sends the info to the `process_csi_packet` task
                        // let _ = ICMP_INFO_CH.try_send(info);
                    }
                }
            }
        };
    });
}

#[embassy_executor::task(pool_size = 2)]
pub async fn net_task(mut runner: Runner<'static, WifiDevice<'static>>) {
    // println!("Network Task Running");
    runner.run().await
}

#[embassy_executor::task]
async fn run_dhcp_server(stack: Stack<'static>, gw_ip_addr: &'static str) {
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
    let seq_num_en = SEQ_NUM_EN.load(core::sync::atomic::Ordering::SeqCst);

    // Receiver for the ICMP info from the sniffer
    // let icmp_receiver = ICMP_INFO_CH.receiver();

    // Buffer for ICMP info that arrives *before* its matching CSI packet
    let mut icmp_buffer: heapless::Vec<IcmpInfo, 4> = heapless::Vec::new();

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
        if seq_num_en {
            // Get the last captured ICMP sequence number and timestamp from global context
            let sequence_no = LAST_SEQ_NUM.load(core::sync::atomic::Ordering::Relaxed);
            let icmp_timestamp = LAST_TIMESTAMP.load(core::sync::atomic::Ordering::Relaxed);

            // Debug Output
            println!("ICMP Timestamp: {:?}", icmp_timestamp);
            println!("ICMP Sequence Number: {:?}", sequence_no);
            println!("CSI Timestamp {:?}", csi_packet.timestamp);

            // Calculate the absolute difference between the timestamps.
            let timestamp_diff = icmp_timestamp.abs_diff(csi_packet.timestamp);

            // If timestamps are within a tolerance window of 1000us, then update the sequence number.
            if timestamp_diff <= 5000 {
                csi_packet.sequence_number = sequence_no;
            } else {
                // Print mistmatch (for debug)
                println!(
                    "Timestamp mismatch! Diff: {} us. CSI: {}, ICMP: {}",
                    timestamp_diff, csi_packet.timestamp, icmp_timestamp
                );
                csi_packet.sequence_number = 0;
            }
        }

        // if seq_num_en {
        //     let mut found_match = false;
        //     let csi_ts = csi_packet.timestamp;

        //     // 2. Check our buffer of ICMP packets that arrived *before* this CSI packet
        //     if !icmp_buffer.is_empty() {
        //         let mut best_match_index: Option<usize> = None;
        //         let mut smallest_diff = u32::MAX;

        //         // Find the ICMP info with the closest timestamp
        //         for (i, icmp_info) in icmp_buffer.iter().enumerate() {
        //             let diff = csi_ts.abs_diff(icmp_info.timestamp);
        //             if diff <= 4000 && diff < smallest_diff {
        //                 smallest_diff = diff;
        //                 best_match_index = Some(i);
        //             }
        //         }

        //         // If we found a match, use it and remove it from the buffer
        //         if let Some(index) = best_match_index {
        //             let icmp_info = icmp_buffer.remove(index);
        //             csi_packet.sequence_number = icmp_info.seq_num;
        //             found_match = true;
        //         }
        //     }

        //     // 3. If no match in buffer, the ICMP info hasn't arrived yet.
        //     //    This handles the original race condition (CSI-then-Sniffer).
        //     if !found_match {
        //         // We'll wait a *very short, bounded time* for the ICMP packet
        //         // that *must* exist. This is an event-driven wait, not a blind poll.
        //         let deadline = Instant::now() + Duration::from_millis(5);

        //         while let Ok(icmp_info) = with_timeout(
        //             deadline.saturating_duration_since(Instant::now()),
        //             icmp_receiver.receive(),
        //         )
        //         .await
        //         {
        //             let diff = csi_ts.abs_diff(icmp_info.timestamp);
        //             if diff <= 4000 {
        //                 // We found our match!
        //                 csi_packet.sequence_number = icmp_info.seq_num;
        //                 found_match = true;
        //                 break; // Exit the while loop
        //             } else {
        //                 // Not a match. This is for a *future* CSI packet. Buffer it.
        //                 if icmp_buffer.push(icmp_info).is_err() {
        //                     // Buffer is full, discard oldest to make room
        //                     icmp_buffer.remove(0);
        //                     let _ = icmp_buffer.push(icmp_info);
        //                 }
        //             }
        //         }
        //     }

        //     // 4. If still no match after checking buffer and waiting, set to 0
        //     if !found_match {
        //         csi_packet.sequence_number = 0;
        //     }
        // } else {
        //     csi_packet.sequence_number = 0;
        // }

        // Update the Watch with the processed CSI
        proc_csi_packet_sender.send(csi_packet);
    }
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

// Extracts Sequence No, source IP
fn extract_icmp_info(payload: &[u8]) -> Option<(u16, Ipv4Address)> {
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

        // Extract source IP (bytes 12-15)
        let src_ip = Ipv4Address::new(ip_header[12], ip_header[13], ip_header[14], ip_header[15]);

        return Some((sequence_no, src_ip));
    }
    None
}

// Function to extract ICMP sequence number from a raw packet
// fn extract_icmp_sequence(payload: &[u8]) -> Option<u16> {
//     // Possible offsets
//     let offsets = [
//         (0, "Direct IP"),
//         (2, "QoS header"),
//         (8, "LLC/SNAP header"),
//         (10, "QoS + LLC/SNAP header"),
//         (16, "QoS + padding + LLC/SNAP"),
//     ];

//     for &(offset, _desc) in offsets.iter() {
//         // Check for IP header (minimum 20 bytes)
//         if payload.len() < offset + 20 {
//             continue;
//         }

//         let ip_header = &payload[offset..offset + 20];

//         // Verify IPv4 (version = 4)
//         if ip_header[0] >> 4 != 4 {
//             continue;
//         }

//         // Extract IP header length (IHL in 4-byte words)
//         let ip_header_len = (ip_header[0] & 0x0F) as usize * 4;
//         if ip_header_len < 20 {
//             continue;
//         }

//         // Check for ICMP header (8 bytes)
//         if payload.len() < offset + ip_header_len + 8 {
//             continue;
//         }

//         // Verify ICMP protocol (byte 9 in IP header = 0x01)
//         if ip_header[9] != 0x01 {
//             continue;
//         }

//         let icmp_header = &payload[offset + ip_header_len..offset + ip_header_len + 8];

//         // Verify ICMP Echo Request (8) or Reply (0)
//         if icmp_header[0] != 0 && icmp_header[0] != 8 {
//             continue;
//         }

//         // Extract sequence number (bytes 6-7, big endian)
//         let sequence_no = u16::from_be_bytes([icmp_header[6], icmp_header[7]]);
//         return Some(sequence_no);
//     }
//     None
// }

// Function to capture CSI info from callback and publish to channel
fn capture_csi_info(info: esp_wifi::wifi::wifi_csi_info_t, mac_filter: Option<[u8; 6]>) {
    // If filter for MAC address is set, no need to proceed if address doesnt match
    // if mac_filter.is_some() && info.mac != mac_filter.unwrap() {
    //     println!("MAC Address {:02X?} Filtered Out", info.mac);
    //     return;
    // }

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

    csi_packet.print_csi_w_metadata();

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

async fn configure_connection(conn_type: ConnectionType, config: Configuration) {
    // Capture Controller & Configuration from Global Context
    let mut controller = CONTROLLER_CH.receive().await;
    match conn_type {
        ConnectionType::AccessPoint => {
            // Set the Configuration
            match controller.set_configuration(&config) {
                Ok(_) => println!("WiFi Configuration Set: {:?}", config),
                Err(_) => {
                    println!("WiFi Configuration Error");
                    println!("Error Config: {:?}", config);
                }
            }
        }
        ConnectionType::Client => {
            // Set the Configuration
            match controller.set_configuration(&config) {
                Ok(_) => println!("WiFi Configuration Set: {:?}", config),
                Err(_) => {
                    println!("WiFi Configuration Error");
                    println!("Error Config: {:?}", config);
                }
            }
        }
        ConnectionType::Mixed => {
            // Set the Configuration
            match controller.set_configuration(&config) {
                Ok(_) => println!("WiFi Configuration Set: {:?}", config),
                Err(_) => {
                    println!("WiFi Configuration Error");
                    println!("Error Config: {:?}", config);
                }
            }
        }
    };

    // Return Controller & Configuration to Global Context
    CONTROLLER_CH.send(controller).await;
}

async fn start_wifi() {
    // Capture Controller & Configuration from Global Context
    let mut controller = CONTROLLER_CH.receive().await;

    match controller.start_async().await {
        Ok(_) => println!("WiFi Started"),
        Err(e) => {
            panic!("Failed to start WiFi: {:?}", e);
        }
    }

    // Return Controller & Configuration to Global Context
    CONTROLLER_CH.send(controller).await;
}

async fn stop_wifi() {
    // Capture Controller & Configuration from Global Context
    let mut controller = CONTROLLER_CH.receive().await;

    match controller.stop_async().await {
        Ok(_) => println!("WiFi Stopped"),
        Err(e) => {
            panic!("Failed to stop WiFi: {:?}", e);
        }
    }

    // Return Controller & Configuration to Global Context
    CONTROLLER_CH.send(controller).await;
}

async fn connect_wifi() {
    // Capture Controller & Configuration from Global Context
    let mut controller = CONTROLLER_CH.receive().await;

    match controller.connect_async().await {
        Ok(_) => println!("WiFi Connected"),
        Err(e) => {
            panic!("Failed to connect WiFi: {:?}", e);
        }
    }

    // Return Controller & Configuration to Global Context
    CONTROLLER_CH.send(controller).await;
}

// pub async fn get_bssid(ssid: &str) -> [u8; 6] {
//     let mut controller = CONTROLLER_CH.receive().await;
//     match controller.start_async().await {
//         Ok(_) => println!("WiFi Started"),
//         Err(e) => {
//             panic!("Failed to start WiFi: {:?}", e);
//         }
//     }
//     let mut scan_config = esp_wifi::wifi::ScanConfig::default();
//     scan_config.ssid = Some(ssid);

//     let ap_info = match controller.scan_with_config_async(scan_config).await {
//         Ok(aps) => {
//             for ap in &aps {
//                 if ap.ssid == ssid {
//                     println!("Found AP - SSID: {}, BSSID: {:02X?}", ap.ssid, ap.bssid);
//                     return ap.bssid;
//                 } else {
//                     continue;
//                 }
//             }
//             println!("Failed to finded matching SSID. MAC filtering disabled.");
//             [0u8; 6]
//         }
//         Err(_e) => {
//             println!("Failed to scan for Access Points. MAC filtering disabled.");
//             [0u8; 6]
//         }
//     };
//     controller.stop_async().await;
//     controller.disconnect_async().await;
//     CONTROLLER_CH.send(controller).await;
//     ap_info
// }

async fn run_ntp_sync(sta_stack: Stack<'static>) {
    println!("Running NTP Sync");
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
    // Signal that NTP Sync is complete
}

/// Updates Client Configuration
// async fn update_client_config(sta_config: ClientConfiguration) {
//     CLIENT_CONFIG_CH.send(sta_config).await;
// }

/// Updates Access Point Configuration
// async fn update_ap_config(ap_config: AccessPointConfiguration) {
//     ACCESSPOINT_CONFIG_CH.send(ap_config).await;
// }

/// Stops Collection
async fn stop_collection() {
    START_COLLECTION.signal(false);
    CONTROLLER_HALTED_SIGNAL.wait().await;
}

/// Recaptures WiFi Controller Instance
async fn recapture_controller() -> WifiController<'static> {
    CONTROLLER_CH.receive().await
}
/// Starts CSI Collection
async fn start_collection(conn_type: ConnectionType, config: Configuration) {
    // Configure Connection
    configure_connection(conn_type.clone(), config).await;

    // In case controller isnt started already, start it
    let controller = CONTROLLER_CH.receive().await;
    if !matches!(controller.is_started(), Ok(true)) {
        start_wifi().await;
    }
    CONTROLLER_CH.send(controller).await;

    // In case controller isnt connected, establish a connection
    let controller = CONTROLLER_CH.receive().await;
    if !matches!(controller.is_connected(), Ok(true)) {
        match conn_type {
            ConnectionType::AccessPoint => {
                // No need to connect if only AP mode
            }
            ConnectionType::Client | ConnectionType::Mixed => {
                // Connect WiFi
                connect_wifi().await;
            }
        }
    }

    CONTROLLER_CH.send(controller).await;

    // Signal Collection Start
    START_COLLECTION.signal(true);
}

async fn run_dhcp_client(sta_stack: Stack<'static>) {
    println!("Running DHCP Client");

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
    // Signal that DHCP is complete
    DHCP_CLIENT_INFO.signal(ip_info);
}

/// Reconstructs a `CSIDataPacket` from a UDP message buffer received in Monitor mode.
///
/// The expected format of `raw_csi_data` is:
/// - Bytes 0-1: u16 sequence_number (big-endian)
/// - Byte 2: u8 data_format (as repr of RxCSIFmt)
/// - Bytes 3-6: u32 timestamp (big-endian)
/// - Bytes 7..end: CSI data (u8 cast from original i8, up to 612 bytes)
///
/// Fields not transmitted (e.g., MAC, RSSI, rate, etc.) are set to default values:
/// - u32/i32 fields: 0
/// - mac: [0; 6]
/// - date_time: None
/// - sig_len: 0 (cannot be reliably reconstructed without additional data)
/// - rx_state: 0 (assumes no error)
///
/// Returns an error if the buffer length is invalid (<7 bytes or CSI data >612 bytes).
pub async fn reconstruct_csi_from_udp() -> Result<CSIDataPacket> {
    // Retrive the new CSI raw data from UDP channel
    let raw_csi_data = CSI_UDP_RAW_CH.receive().await;

    if raw_csi_data.len() < 7 {
        return Err(crate::error::Error::SystemError(
            "Buffer too short: must be at least 7 bytes",
        ));
    }

    let csi_data_start = 7;
    let csi_len = (raw_csi_data.len() - csi_data_start) as u16;
    if csi_len > 612 {
        return Err(crate::error::Error::SystemError(
            "CSI data too long: max 612 bytes",
        ));
    }

    // Extract sequence_number (u16 Big Endian)
    let sequence_number = u16::from_be_bytes([raw_csi_data[0], raw_csi_data[1]]);

    // Extract data_format (u8 -> RxCSIFmt)
    let fmt_u8 = raw_csi_data[2];
    let (data_format, bandwidth, sig_mode, stbc, secondary_channel) = match fmt_u8 {
        0 => (RxCSIFmt::Bw20, 0, 0, 0, 0),
        1 => (RxCSIFmt::HtBw20, 0, 1, 0, 0),
        2 => (RxCSIFmt::HtBw20Stbc, 0, 1, 1, 0),
        3 => (RxCSIFmt::SecbBw20, 0, 0, 0, 2),
        4 => (RxCSIFmt::SecbHtBw20, 0, 1, 0, 2),
        5 => (RxCSIFmt::SecbHtBw20Stbc, 0, 1, 1, 2),
        6 => (RxCSIFmt::SecbHtBw40, 1, 1, 0, 2),
        7 => (RxCSIFmt::SecbHtBw40Stbc, 1, 1, 1, 2),
        8 => (RxCSIFmt::SecaBw20, 0, 0, 0, 1),
        9 => (RxCSIFmt::SecaHtBw20, 0, 1, 0, 1),
        10 => (RxCSIFmt::SecaHtBw20Stbc, 0, 1, 1, 1),
        11 => (RxCSIFmt::SecaHtBw40, 1, 1, 0, 1),
        12 => (RxCSIFmt::SecaHtBw40Stbc, 1, 1, 1, 1),
        _ => (RxCSIFmt::Undefined, 0, 0, 0, 0),
    };

    // Extract timestamp (u32 BE)
    let timestamp = u32::from_be_bytes([
        raw_csi_data[3],
        raw_csi_data[4],
        raw_csi_data[5],
        raw_csi_data[6],
    ]);

    // Reconstruct CSI data (u8 -> i8, preserving sign via bit reinterpretation)
    let mut csi_data = Vec::new();
    for &b in &raw_csi_data[csi_data_start..] {
        csi_data
            .push(b as i8)
            .map_err(|_| "Failed to push to Vec (capacity exceeded)")
            .unwrap();
    }

    // Build CSIDataPacket with defaults for missing fields
    Ok(CSIDataPacket {
        mac: [0u8; 6],
        rssi: 0,
        timestamp,
        rate: 0,
        sgi: 0,
        secondary_channel: secondary_channel,
        channel: 0,
        bandwidth: bandwidth,
        antenna: 0,
        sig_mode: sig_mode,
        mcs: 0,
        smoothing: 0,
        not_sounding: 0,
        aggregation: 0,
        stbc: stbc,
        fec_coding: 0,
        ampdu_cnt: 0,
        noise_floor: 0,
        rx_state: 0,
        sig_len: 0,
        date_time: None,
        sequence_number,
        data_format,
        csi_data_len: csi_len,
        csi_data,
    })
}
