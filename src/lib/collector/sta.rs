use embassy_executor::Spawner;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::watch::Receiver;

use smoltcp::phy::ChecksumCapabilities;
use smoltcp::wire::{Icmpv4Packet, Icmpv4Repr, Ipv4Packet, Ipv4Repr};

use embassy_futures::select::{select, Either};

use embassy_net::{
    raw::{IpProtocol, IpVersion, PacketMetadata as RawPacketMetadata, RawSocket},
    udp::{PacketMetadata, UdpSocket},
    IpEndpoint, Stack, StackResources,
};

use embassy_time::{Duration, Timer};

use enumset::enum_set;

use esp_alloc as _;
use esp_backtrace as _;
use esp_println::println;
use esp_wifi::wifi::WifiController;
use esp_wifi::wifi::{ClientConfiguration, Configuration, Interfaces, WifiDevice, WifiEvent};

use crate::error::Result;
use crate::run_dhcp_client;

use heapless::Vec;

use crate::{
    build_csi_config, capture_csi_info, configure_connection, connect_wifi, net_task,
    process_csi_packet, recapture_controller, run_ntp_sync, start_collection, start_wifi,
    stop_collection, ConnectionType,
};
use crate::{sequence_sync_task, CSIConfig};

use crate::{
    CSIDataPacket, CONTROLLER_CH, CONTROLLER_HALTED_SIGNAL, CSI_CONFIG_CH, DHCP_CLIENT_INFO,
    PROC_CSI_DATA, SEQ_NUM_EN, START_COLLECTION,
};

macro_rules! mk_static {
    ($t:ty,$val:expr) => {{
        static STATIC_CELL: static_cell::StaticCell<$t> = static_cell::StaticCell::new();
        #[deny(unused_attributes)]
        let x = STATIC_CELL.uninit().write(($val));
        x
    }};
}

/// Station Operation Modes
/// Trigger: Sends trigger packets (traffic) to stimulate CSI collection locally.
/// Monitor: Monitors incoming trigger packets stimulating CSI collection and sends back collected CSI to trigger source in UDP packet.
#[derive(PartialEq, Copy, Clone)]
pub enum StaOperationMode {
    Trigger(StaTriggerConfig),
    Monitor(StaMonitorConfig),
}

/// Configuration for Station Monitor Mode
#[derive(PartialEq, Copy, Clone)]
pub struct StaMonitorConfig {
    /// Source Port #
    local_port: u16,
    /// Destination Port #
    dest_port: u16,
}

/// Configuration for Station Trigger Mode
#[derive(PartialEq, Copy, Clone)]
pub struct StaTriggerConfig {
    /// Trigger Packet Frequency
    pub trigger_freq_hz: u32,
}

impl Default for StaMonitorConfig {
    fn default() -> Self {
        Self {
            local_port: 10789,
            dest_port: 10789,
        }
    }
}

impl Default for StaTriggerConfig {
    fn default() -> Self {
        Self {
            trigger_freq_hz: 100,
        }
    }
}

/// Driver Struct to Collect CSI as a Station
pub struct CSIStation {
    /// Station Configuration
    /// Note: This configuration is applied during init and can be updated via 'update_station_config()'
    pub sta_config: ClientConfiguration,
    /// Operation Mode: Trigger or Monitor
    pub op_mode: StaOperationMode,
    /// CSI Collection Parameters
    pub csi_config: CSIConfig,
    /// Synchronize NTP Time with Trigger Source
    /// Note: Requires internet connectivity at the Access Point
    pub sync_time: bool,
    /// MAC Address Filter for CSI Data
    mac_filter: Option<[u8; 6]>,
    /// Receiver for Processed CSI Data Packets
    csi_data_rx: Receiver<'static, CriticalSectionRawMutex, CSIDataPacket, 3>,
}

impl CSIStation {
    /// Creates a new `CSIStation` instance with a defined configuration/profile.
    /// 'CSIConfig' Defines the CSI Collection Parameters
    /// 'ClientConfiguration' Defines the WiFi Client/Station Connection Parameters
    /// 'StaOperationMode' Defines the Operation Mode: Trigger or Monitor
    /// 'mac_filter' is a' 'Option<[u8; 6]>' Optional MAC Address Filter for CSI Data
    /// 'sync_time' is to synchronize time with NTP server at Access Point (requirecs internet connectivity at AP)
    /// 'WifiController' Is a WiFi Controller instance
    pub async fn new(
        csi_config: CSIConfig,
        sta_config: ClientConfiguration,
        op_mode: StaOperationMode,
        sync_time: bool,
        wifi_controller: WifiController<'static>,
    ) -> Self {
        let csi_data_rx = PROC_CSI_DATA.receiver().unwrap();
        // Send shared data to global context
        CONTROLLER_CH.send(wifi_controller).await;
        Self {
            csi_config,
            sta_config,
            mac_filter: None,
            op_mode,
            csi_data_rx,
            sync_time,
        }
    }

    /// Creates a new `CSIStation` instance with defaults.
    /// 'CSIConfig' is set to 'default'
    /// 'ClientConfiguration' is set to 'default'
    /// 'StaOperationMode' is set to Trigger with default trigger configuration
    /// 'mac_filter' is set to 'None'
    /// 'sync_time' is set to 'false'
    pub async fn new_with_defaults(wifi_controller: WifiController<'static>) -> Self {
        let proc_csi_data_rx = PROC_CSI_DATA.receiver().unwrap();
        // let controller_rx = CONTROLLER_CH.receiver();
        CONTROLLER_CH.send(wifi_controller).await;
        // CLIENT_CONFIG_CH.send(ClientConfiguration::default()).await;
        Self {
            csi_config: CSIConfig::default(),
            sta_config: ClientConfiguration::default(),
            mac_filter: None,
            op_mode: StaOperationMode::Trigger(StaTriggerConfig::default()),
            csi_data_rx: proc_csi_data_rx,
            sync_time: false,
        }
    }

    /// Initialize WiFi and the CSI Collection System. This method starts the WiFi connection and spawns the required tasks.
    pub async fn init(&mut self, interface: Interfaces<'static>, spawner: &Spawner) -> Result<()> {
        println!("Initializing Station");

        // Station IP Configuration - DHCP
        let sta_ip_config = embassy_net::Config::dhcpv4(Default::default());
        let seed = 123456_u64;

        // Create STA Network Stack
        let (sta_stack, sta_runner) = embassy_net::new(
            interface.sta,
            sta_ip_config,
            mk_static!(StackResources<6>, StackResources::<6>::new()),
            seed,
        );

        // Spawn the network runner task
        spawner.spawn(net_task(sta_runner)).ok();
        println!("Network Task Running");

        // Configure WiFi Client/Station Connection
        let config = Configuration::Client(self.sta_config.clone());
        configure_connection(ConnectionType::Client, config).await;

        // Start & Connect WiFi
        // This needs to be done before DHCP and NTP
        start_wifi().await;
        connect_wifi().await;

        // Optionally retrieve MAC address from AP
        // let bssid = get_connected_ap_bssid();
        // esp_println::println!("Connected AP BSSID: {:02X?}", bssid);
        // self.mac_filter = Some(bssid);

        // Run DHCP Client to acquire IP
        run_dhcp_client(sta_stack).await;
        // Run NTP Sync to synchronize time
        if self.sync_time {
            run_ntp_sync(sta_stack).await;
        }

        // Spawn Remaining Tasks: CSI Processing, Connection Managment, and Network Operations
        spawner.spawn(process_csi_packet()).ok();
        spawner.spawn(sta_connection(self.op_mode)).ok();
        spawner.spawn(sta_network_ops(sta_stack, self.op_mode)).ok();

        // If in Monitor Mode, sniffer promiscuous mode is required to cross reference triggering ICMP or Beacon packets sequence number with extracted CSI
        match self.op_mode {
            StaOperationMode::Monitor(_config) => {
                SEQ_NUM_EN.store(true, core::sync::atomic::Ordering::Relaxed);
                // Spawn sequence number sync task
                spawner.spawn(sequence_sync_task(interface.sniffer)).ok();
            }
            _ => {}
        }

        // No need to wait on DHCP and NTP as they are awaited in init above
        println!("Station Initialized");

        Ok(())
    }

    /// Starts the Station & Loads Configuration
    /// To reconfigure Station settings, no need to reinit, only call start again with the updated configuration.
    pub async fn start_collection(&self) {
        let config = Configuration::Client(self.sta_config.clone());
        start_collection(crate::ConnectionType::Client, config).await;
    }

    /// Stops Collection
    pub async fn stop_collection(&self) {
        stop_collection().await;
    }

    /// Recaptures WiFi Controller Instance
    pub async fn recapture_controller(&self) -> WifiController<'static> {
        recapture_controller().await
    }

    /// Updates Client Configuration
    pub async fn update_sta_config(&mut self, sta_config: ClientConfiguration) {
        // update_client_config(sta_config).await;
        self.sta_config = sta_config;
    }

    /// Updates CSI Configuration
    pub async fn update_csi_config(&mut self, csi_config: CSIConfig) {
        self.csi_config = csi_config;
    }

    /// Updates MAC Address Filter
    pub async fn update_mac_filter(&mut self, mac_filter: Option<[u8; 6]>) {
        self.mac_filter = mac_filter;
    }

    /// Retrieve the latest available CSI data packet
    pub async fn get_csi_data(&mut self) -> Result<CSIDataPacket> {
        // Wait for CSI data packet to update
        let csi_data_pkt = self.csi_data_rx.changed().await;
        Ok(csi_data_pkt)
    }

    /// Print the latest CSI data with metadata to console
    /// Optionally pass current time instant to calculate DateTimeCapture if available
    pub async fn print_csi_w_metadata(&mut self) {
        // Wait for CSI data packet to update
        let proc_csi_data = self.csi_data_rx.changed().await;

        // Print the CSI data to console
        proc_csi_data.print_csi_w_metadata();
    }
}

// This task manages the connection and establisehes CSI collection for Trigger mode
#[embassy_executor::task]
pub async fn sta_connection(op_mode: StaOperationMode) {
    // Acquire Controller receiver
    let controller_rx = CONTROLLER_CH.receiver();
    // Get access to start collection watch
    let mut start_collection_watch = match START_COLLECTION.receiver() {
        Some(r) => r,
        None => panic!("Maximum number of recievers reached"),
    };
    // Define Events to Listen for
    let sta_events =
        enum_set!(WifiEvent::StaDisconnected | WifiEvent::StaStop | WifiEvent::StaConnected);

    loop {
        // Wait fot start signal
        while !start_collection_watch.changed().await {
            // If Start Collection is false, keep waiting
            Timer::after(Duration::from_millis(100)).await;
        }

        // Acquire the controller
        let mut controller = controller_rx.receive().await;

        // Trigger Logic
        match op_mode {
            StaOperationMode::Trigger(_) => {
                // Retrieved Updated Configuration
                let csi_config = CSI_CONFIG_CH.receive().await;
                // Build CSI Configuration
                let csi_cfg = build_csi_config(csi_config.clone());
                println!("Starting CSI Collection");
                controller
                    .set_csi(csi_cfg, |info: esp_wifi::wifi::wifi_csi_info_t| {
                        capture_csi_info(info);
                    })
                    .unwrap();
            }
            _ => {
                // Monitor mode is handled by sequence_sync_task,
                // Nothing needed here.
            }
        }

        // Monitoring/stop loop
        loop {
            // Events Future
            let wait_event_fut = controller.wait_for_events(sta_events, true);
            // Stop Collection Future
            let stop_coll_fut = start_collection_watch.changed();

            // If either future completes, handle accordingly
            match select(wait_event_fut, stop_coll_fut).await {
                // Wait event future cases
                Either::First(mut event) => {
                    if event.contains(WifiEvent::StaDisconnected) {
                        println!("STA Disconnected");
                    }
                    if event.contains(WifiEvent::StaStop) {
                        println!("STA Stopped");
                    }
                    event.clear();
                }
                // Stop collection future case
                Either::Second(sig) => {
                    // Stop Signal
                    if !sig {
                        println!("Halting CSI Collection...");
                        // Send the controller back before exiting loop
                        CONTROLLER_CH.send(controller).await;
                        CONTROLLER_HALTED_SIGNAL.signal(true);
                        break;
                    }
                }
            }
        }
    }
}

// This task manages network operations for the station
// This includes waiting for link up, DHCP, etc. followed by managing UDP connection with trigger source
#[embassy_executor::task]
pub async fn sta_network_ops(sta_stack: Stack<'static>, sta_config: StaOperationMode) {
    // Retrieve acquired IP information from DHCP
    let ip_info = DHCP_CLIENT_INFO.wait().await;

    let mut start_collection_watch = match START_COLLECTION.receiver() {
        Some(r) => r,
        None => panic!("Maximum number of recievers reached"),
    };

    match sta_config {
        StaOperationMode::Trigger(trigger_config) => {
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

            loop {
                // Wait for start signal
                while !start_collection_watch.changed().await {
                    Timer::after(Duration::from_millis(100)).await;
                }
                println!("Starting Trigger Traffic");
                // Station Trigger supports sending ICMP Echo Requests as trigger packets at defined frequency
                let trigger_interval =
                    Duration::from_millis((1000 / trigger_config.trigger_freq_hz).into());
                // Start sending trigger packets
                loop {
                    // Trigger Interval Future
                    let trigger_timer_fut = Timer::after(trigger_interval);
                    // Stop Trigger Future
                    let stop_coll_fut = start_collection_watch.changed();

                    match select(trigger_timer_fut, stop_coll_fut).await {
                        Either::First(_) => {
                            // Send raw packet
                            raw_socket.send(ipv4_packet_buffer).await;
                        }
                        Either::Second(sig) => {
                            if !sig {
                                println!("Stopping Trigger Traffic");
                                break;
                            }
                        }
                    };
                }
            }
        }
        StaOperationMode::Monitor(monitor_config) => {
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
            // Bind to specified source port
            socket.bind(monitor_config.local_port).unwrap();
            // Endpoint to send back collected CSI data
            let endpoint = IpEndpoint::new(
                embassy_net::IpAddress::Ipv4(ip_info.gateway_address),
                monitor_config.dest_port,
            );

            let mut proc_csi_data_rx = PROC_CSI_DATA.receiver().unwrap();

            // Create a message buffer for the data to be sent back

            // Message format w/ seq_no:
            // [0..1]   : 2 bytes seq_no (u16) - big endian
            // [2]      : 1 byte for CSI data format (mapping below)
            // [2..7]   : 4 bytes timestamp (u32) - big endian
            // [7..n]   : n-6 bytes CSI data (i8)

            // Width of message (619) = 2 bytes for seq_no + 1 byte for format + 4 bytes for timestamp + 612 bytes for CSI data
            let mut message_u8: Vec<u8, 619> = Vec::new();
            // let seq_num_en = SEQ_NUM_EN.load(core::sync::atomic::Ordering::SeqCst);
            loop {
                // Wait for start signal
                while !start_collection_watch.changed().await {
                    Timer::after(Duration::from_millis(100)).await;
                }
                loop {
                    // New CSI Data Future
                    let proc_csi_data_fut = proc_csi_data_rx.changed();
                    // Stop Trigger Future
                    let stop_coll_fut = start_collection_watch.changed();

                    match select(proc_csi_data_fut, stop_coll_fut).await {
                        Either::First(proc_csi_data) => {
                            // Clear the buffer for new message
                            message_u8.clear();

                            // Wait for CSI data packet to update
                            println!("Waiting on New CSI Data");
                            // let proc_csi_data = proc_csi_data_rx.changed().await;

                            println!(
                                "Sending CSI Data with Seq No: {}",
                                proc_csi_data.sequence_number
                            );

                            // CSI is captured in a callback that does not have access to the ICMP sequence number
                            // The CSI callback, however, does have access to the timestamp of the packet
                            // So we use a global context to store the last captured ICMP sequence number and timestamp
                            // We use timestamp to match the CSI to the ICMP sequence number

                            // Append the sequence number to the message, if enabled
                            // if seq_num_en {
                            message_u8
                                .extend_from_slice(&proc_csi_data.sequence_number.to_be_bytes())
                                .unwrap();
                            // } else {
                            //     message_u8.extend_from_slice(&0_u16.to_be_bytes()).unwrap();
                            // }

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
                        Either::Second(sig) => {
                            if !sig {
                                println!("Halting Messages to Trigger Source");
                                break;
                            }
                        }
                    }
                }
            }
        }
    }
}

// Get MAC from AP Code

// #[repr(C)]
// pub struct wifi_ap_record_t {
//     pub bssid: [u8; 6],
// }
// extern "C" {
//     pub fn esp_wifi_sta_get_ap_info(ap_info: *mut wifi_ap_record_t);
// }

// fn get_connected_ap_bssid() -> [u8; 6] {
//     let mut ap_record: wifi_ap_record_t = unsafe { core::mem::zeroed() };

//     unsafe { esp_wifi_sta_get_ap_info(&mut ap_record) };

//     ap_record.bssid
// }
