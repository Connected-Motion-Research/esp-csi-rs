use embassy_executor::Spawner;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Receiver as ChannelReceiver;
use embassy_sync::watch::Receiver;

use smoltcp::phy::ChecksumCapabilities;
use smoltcp::wire::{Icmpv4Packet, Icmpv4Repr, Ipv4Packet, Ipv4Repr};

use embassy_net::{
    raw::{IpProtocol, IpVersion, PacketMetadata as RawPacketMetadata, RawSocket},
    udp::{PacketMetadata, UdpSocket},
    IpAddress, IpEndpoint, Ipv4Address, Ipv4Cidr, Runner, Stack, StackResources, StaticConfigV4,
};

use embassy_time::{Duration, Instant, Timer};

use esp_alloc as _;
use esp_backtrace as _;
use esp_println::println;
use esp_wifi::wifi::WifiController;
use esp_wifi::wifi::{ClientConfiguration, Configuration, Interfaces, WifiDevice};

use crate::collector::run_dhcp_client;
use crate::error::Result;

use heapless::Vec;

use crate::{
    build_csi_config, capture_csi_info, get_sntp_time, net_task, process_csi_packet,
    unix_to_date_time,
};
use crate::{sequence_sync_task, CSIConfig};

use crate::{
    CSIDataPacket, DateTimeCapture, CONTROLLER_CH, CSI_CONFIG_CH, DATE_TIME, DATE_TIME_VALID,
    DHCP_CLIENT_INFO, MAC_FIL_CH, PROC_CSI_DATA, SEQ_NUM_EN, START_COLLECTION_,
};

macro_rules! mk_static {
    ($t:ty,$val:expr) => {{
        static STATIC_CELL: static_cell::StaticCell<$t> = static_cell::StaticCell::new();
        #[deny(unused_attributes)]
        let x = STATIC_CELL.uninit().write(($val));
        x
    }};
}

/// Access Point Operation Modes
/// Trigger: Sends trigger packets to stimulate CSI collection locally.
/// Monitor: Monitors incoming trigger Packets stimulating CSI collection and sends back collected CSI to trigger source.

#[derive(PartialEq, Copy, Clone)]
pub enum StaOperationMode {
    Trigger(StaTriggerConfig),
    Monitor(StaMonitorConfig),
}

/// Configuration for Access Point Monitor Mode
#[derive(PartialEq, Copy, Clone)]
pub struct StaMonitorConfig {
    /// Source Port #
    src_port: u16,
    /// Destination Port #
    dest_port: u16,
}

/// Configuration for Access Point Trigger Mode
#[derive(PartialEq, Copy, Clone)]
pub struct StaTriggerConfig {
    /// Trigger Packet Frequency
    trigger_freq_hz: u32,
}

impl Default for StaMonitorConfig {
    fn default() -> Self {
        Self {
            src_port: 10789,
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
    /// CSI Collection Parameters
    pub csi_config: CSIConfig,
    // Station/Client Configuration
    pub sta_config: ClientConfiguration,
    /// Operation Mode: Trigger or Monitor
    pub op_mode: StaOperationMode,
    /// Optional MAC Address Filter for CSI Data
    pub mac_filter: Option<[u8; 6]>,
    /// Synchronize NTP Time with Trigger Source
    /// Note: Requires internet connectivity at the Access Point
    pub sync_time: bool,
    csi_data_rx: Receiver<'static, CriticalSectionRawMutex, CSIDataPacket, 3>,
    controller_rx: ChannelReceiver<'static, CriticalSectionRawMutex, WifiController<'static>, 1>,
}

impl CSIStation {
    /// Creates a new `CSIStation` instance with a defined configuration/profile.
    pub fn new(
        csi_config: CSIConfig,
        sta_config: ClientConfiguration,
        op_mode: StaOperationMode,
        mac_filter: Option<[u8; 6]>,
        sync_time: bool,
    ) -> Self {
        let csi_data_rx = PROC_CSI_DATA.receiver().unwrap();
        let controller_rx = CONTROLLER_CH.receiver();
        Self {
            csi_config,
            sta_config,
            mac_filter,
            op_mode,
            csi_data_rx,
            controller_rx,
            sync_time,
        }
    }

    /// Creates a new `CSIStation` instance with defaults.
    pub fn new_with_defaults() -> Self {
        let proc_csi_data_rx = PROC_CSI_DATA.receiver().unwrap();
        let controller_rx = CONTROLLER_CH.receiver();
        Self {
            csi_config: CSIConfig::default(),
            sta_config: ClientConfiguration::default(),
            mac_filter: None,
            op_mode: StaOperationMode::Trigger(StaTriggerConfig::default()),
            csi_data_rx: proc_csi_data_rx,
            controller_rx: controller_rx,
            sync_time: false,
        }
    }

    /// Updates `CSIStation` Client Configuration
    pub fn update_config(&mut self, sta_config: ClientConfiguration) {
        self.sta_config = sta_config;
    }

    /// Initialize WiFi and the CSI Collection System. This method starts the WiFi connection and spawns the required tasks.
    pub async fn init(&self, interface: Interfaces<'static>, spawner: &Spawner) -> Result<()> {
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

        // Spawn the network runner task to run DHCP and NTP
        spawner.spawn(net_task(sta_runner)).ok();

        // Run DHCP Client to acquire IP
        run_dhcp_client(sta_stack).await;
        // Run NTP Sync to synchronize time
        if self.sync_time {
            println!("Running NTP Sync");
            run_ntp_sync(sta_stack).await;
        }

        // Spawn Remaining Tasks: CSI Processing, Connection Managment, and Network Operations
        spawner.spawn(process_csi_packet()).ok();
        spawner.spawn(sta_connection()).ok();
        spawner.spawn(sta_network_ops(sta_stack, self.op_mode)).ok();

        // If in Monitor Mode, sniffer promiscuous mode is required to cross reference triggering ICMP or Beacon packets sequence number with extracted CSI
        match self.op_mode {
            StaOperationMode::Monitor(_config) => {
                // Spawn sequence number sync task
                spawner.spawn(sequence_sync_task(interface.sniffer)).ok();
            }
            _ => {}
        }

        // Make sure network task is running
        // NET_TASK_COMPLETE.wait().await;
        // No need to wait on DHCP and NTP as they are awaited in init above
        println!("Access Point Initialized");

        Ok(())
    }

    /// Starts CSI Collection
    pub async fn start(&self, mut controller: WifiController<'static>) {
        // Send Updated CSI & MAC Filter Configs
        CSI_CONFIG_CH.send(self.csi_config.clone()).await;
        MAC_FIL_CH.send(self.mac_filter).await;
        // Send Station Configuration
        let config = Configuration::Client(self.sta_config.clone());
        match controller.set_configuration(&config) {
            Ok(_) => println!("WiFi Configuration Set: {:?}", config),
            Err(_) => {
                println!("WiFi Configuration Error");
                println!("Error Config: {:?}", config);
            }
        }

        // In case controller isnt started already, start it
        if !matches!(controller.is_started(), Ok(true)) {
            controller.start_async().await.unwrap();
            println!("Wifi started!");
        }

        // Establish Connection with Access Point
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

        // Signal Collection Start
        START_COLLECTION_.signal(true);

        // Share WiFi Controller to Global Context
        // This is such that the controller can be returned if the Collection needs to be recaptured
        CONTROLLER_CH.send(controller).await;
    }

    /// Stops Collection
    pub async fn stop(&self) -> WifiController<'static> {
        START_COLLECTION_.signal(false);
        self.controller_rx.receive().await
    }

    /// Recaptures WiFi Controller Instance
    pub async fn recapture_controller(&self) -> WifiController<'static> {
        self.controller_rx.receive().await
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
}

// This task manages the connection and establisehes CSI collection
#[embassy_executor::task]
pub async fn sta_connection() {
    // Acquire Controller
    let mut controller_rx = CONTROLLER_CH.receiver();
    // Define Events to Listen for
    let sta_events =
        enum_set!(WifiEvent::StaDisconnected | WifiEvent::StaStop | WifiEvent::StaConnected);
    loop {
        // Wait for Start Signal
        while !START_COLLECTION_.wait().await {
            // If Start Collection is false, keep waiting
            Timer::after(Duration::from_millis(100)).await;
        }
        // Retrieved Updated Configuration
        let mac_filter = MAC_FIL_CH.receive().await;
        let csi_config = CSI_CONFIG_CH.receive().await;
        // Build CSI Configuration
        let csi_cfg = build_csi_config(csi_config.clone());
        println!("Starting CSI Collection");
        controller
            .set_csi(csi_cfg, |info: esp_wifi::wifi::wifi_csi_info_t| {
                capture_csi_info(info, mac_filter);
            })
            .unwrap();
        let mut controller = controller_rx.receive().await;
        loop {
            // Events Future
            let wait_event_fut = controller.wait_for_events(ap_events, true);
            // Stop Collection Future
            let stop_coll_fut = START_COLLECTION_.wait();

            // If either future completes, handle accordingly
            match select(wait_event_fut, stop_coll_fut).await {
                // Wait event future cases
                Either::First(mut event) => {
                    if event.contains(WifiEvent::ApStaconnected) {
                        println!("New STA Connected");
                    }
                    if event.contains(WifiEvent::ApStadisconnected) {
                        println!("STA Disconnected");
                    }
                    event.clear();
                }
                // Stop collection future case
                // Return Controller and break inner loop
                Either::Second(_sig) => {
                    println!("Halting CSI Collection...");
                    // Send the controller back before we exit the inner loop
                    CONTROLLER_CH.send(controller).await;
                    break; // Break inner loop, and go wait again for start and new controller
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

    // ICMP needed for both trigger and monitor modes
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

    match sta_config {
        StaOperationMode::Trigger(trigger_config) => {
            // Station Trigger supports sending ICMP Echo Requests as trigger packets at defined frequency
            let trigger_interval =
                Duration::from_millis((1000 / trigger_config.trigger_freq_hz).into());
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

            // Start sending trigger packets
            loop {
                // Send raw packet
                raw_socket.send(ipv4_packet_buffer).await;

                // Wait for user specified duration
                Timer::after(trigger_interval).await;
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
            socket.bind(monitor_config.src_port).unwrap();
            // Endpoint to send back collected CSI data
            let endpoint = IpEndpoint::new(
                embassy_net::IpAddress::Ipv4(ip_info.gateway_address),
                monitor_config.dest_port,
            );

            let mut proc_csi_data_rx = PROC_CSI_DATA.receiver().unwrap();

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
}

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
    // NTP_SYNC_COMPLETE.signal(());
}
