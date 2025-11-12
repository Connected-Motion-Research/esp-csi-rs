use embassy_executor::Spawner;

use heapless::Vec;
use smoltcp::phy::ChecksumCapabilities;
use smoltcp::wire::{Icmpv4Packet, Icmpv4Repr, Ipv4Packet, Ipv4Repr};

use embassy_futures::select::{select, Either};

use embassy_time::{Duration, Timer};

use embassy_net::{
    raw::{IpProtocol, IpVersion, PacketMetadata as RawPacketMetadata, RawSocket},
    udp::{PacketMetadata, UdpSocket},
    Ipv4Cidr, Stack, StackResources, StaticConfigV4,
};

use core::{net::Ipv4Addr, str::FromStr};

use esp_alloc as _;
use esp_backtrace as _;
use esp_println::println;
use esp_wifi::wifi::{AccessPointConfiguration, Configuration, Interfaces};
use esp_wifi::wifi::{WifiController, WifiDevice, WifiEvent};

use crate::error::Result;

use enumset::enum_set;

use crate::{
    configure_connection, net_task, recapture_controller, reconstruct_csi_from_udp,
    run_dhcp_server, start_collection, start_wifi, stop_collection, ConnectionType,
    CONTROLLER_HALTED_SIGNAL,
};

use crate::{CSIDataPacket, CONTROLLER_CH, CSI_UDP_RAW_CH, START_COLLECTION};

macro_rules! mk_static {
    ($t:ty,$val:expr) => {{
        static STATIC_CELL: static_cell::StaticCell<$t> = static_cell::StaticCell::new();
        #[deny(unused_attributes)]
        let x = STATIC_CELL.uninit().write(($val));
        x
    }};
}

/// Access Point Operation Modes
/// Trigger: Sends trigger packets to recieve CSI encapsulated in UDP packets from station.
/// Monitor: Monitors incoming trigger packets to facilitate CSI collection at the trigger source (station).
#[derive(PartialEq, Copy, Clone)]
pub enum ApOperationMode {
    Trigger(ApTriggerConfig),
    Monitor,
}

/// Configuration for Access Point Trigger Mode
#[derive(PartialEq, Copy, Clone)]
pub struct ApTriggerConfig {
    /// Trigger Packet Frequency
    trigger_freq_hz: u32,
    /// Local Port #
    local_port: u16,
    /// Trigger Type - Broadcast or Unicast
    trigger_type: TriggerType,
    /// Trigger Sequence Number Start
    trigger_seq_num: u16,
}

impl Default for ApTriggerConfig {
    fn default() -> Self {
        Self {
            trigger_freq_hz: 1,
            local_port: 10789,
            trigger_type: TriggerType::Broadcast,
            trigger_seq_num: 0,
        }
    }
}

#[derive(PartialEq, Copy, Clone)]
enum TriggerType {
    Broadcast,
    /// Unicast Trigger Type requires an IP Address
    #[allow(dead_code)]
    Unicast(&'static str),
}

/// Driver Struct to Enable CSI collection as an Access Point
pub struct CSIAccessPoint {
    /// Access Point Configuration
    pub ap_config: AccessPointConfiguration,
    /// Operation Mode: Trigger or Monitor
    pub op_mode: ApOperationMode,
    // controller_rx: ChannelReceiver<'static, CriticalSectionRawMutex, WifiController<'static>, 1>,
}

impl CSIAccessPoint {
    /// Creates a new `CSIAccessPoint` instance with a defined configuration/profile.
    /// 'AccessPointConfiguration' Defines the WiFi Access Point Connection Parameters
    /// 'ApOperationMode' Defines the Operation Mode: Trigger or Monitor
    /// 'WifiController' Is a WiFi Controller instance
    pub async fn new(
        ap_config: AccessPointConfiguration,
        op_mode: ApOperationMode,
        wifi_controller: WifiController<'static>,
    ) -> Self {
        // Send shared data to global context
        CONTROLLER_CH.send(wifi_controller).await;
        // ACCESSPOINT_CONFIG_CH.send(ap_config.clone()).await;
        Self { op_mode, ap_config }
    }

    /// Creates a new `CSIAccessPoint` instance with defaults.
    /// 'AccessPointConfiguration' is set to 'default'
    /// 'ApOperationMode' is set to Monitor
    pub async fn new_with_defaults(wifi_controller: WifiController<'static>) -> Self {
        CONTROLLER_CH.send(wifi_controller).await;
        Self {
            op_mode: ApOperationMode::Monitor,
            ap_config: AccessPointConfiguration::default(),
        }
    }

    /// Initialize WiFi and the CSI Collection System. This method spawns the connection, DHCP, and network tasks.
    pub async fn init(&self, interface: Interfaces<'static>, spawner: &Spawner) -> Result<()> {
        println!("Initializing Access Point");

        // Create gateway IP address instance
        // This config doesnt get an IP address from router but runs DHCP server
        let gw_ip_addr_str = "192.168.2.1";
        let gw_ip_addr = Ipv4Addr::from_str(gw_ip_addr_str).expect("failed to parse gateway ip");

        // Access Point IP Configuration
        let ap_ip_config = embassy_net::Config::ipv4_static(StaticConfigV4 {
            address: Ipv4Cidr::new(gw_ip_addr, 24),
            gateway: Some(gw_ip_addr),
            dns_servers: Default::default(),
        });

        let seed = 123456_u64;
        let ap_interface = interface.ap;

        // Print Device MAC address
        let bssid = ap_interface.mac_address();
        esp_println::println!("AP BSSID: {:02X?}", bssid);

        // Create AP Network Stack
        let (ap_stack, ap_runner) = embassy_net::new(
            ap_interface,
            ap_ip_config,
            mk_static!(StackResources<6>, StackResources::<6>::new()),
            seed,
        );

        // Spawn network task
        spawner.spawn(net_task(ap_runner)).ok();
        println!("Network Task Running");

        // Configure WiFi Client/Station Connection
        let config = Configuration::AccessPoint(self.ap_config.clone());
        configure_connection(ConnectionType::AccessPoint, config).await;

        // Start & Connect WiFi
        // This needs to be done before DHCP and NTP
        start_wifi().await;

        spawner
            .spawn(run_dhcp_server(ap_stack, gw_ip_addr_str))
            .ok();
        spawner.spawn(ap_connection()).ok();

        match self.op_mode {
            ApOperationMode::Trigger(config) => {
                spawner.spawn(ap_icmp_trigger(ap_stack, config)).ok();
                spawner.spawn(ap_udp_receiver(ap_stack, config)).ok();
            }
            _ => {}
        }

        // Initialization Finished
        println!("Access Point Initialized");
        Ok(())
    }

    /// Starts the Access Point & Loads Configuration
    /// To reconfigure AP settings, no need to reinit, only call start again with the updated configuration.
    pub async fn start_collection(&self) {
        let config = Configuration::AccessPoint(self.ap_config.clone());
        start_collection(crate::ConnectionType::AccessPoint, config).await;
    }

    /// Stops Collection
    pub async fn stop_collection(&self) {
        stop_collection().await;
    }

    /// Recaptures WiFi Controller Instance
    pub async fn recapture_controller(&self) -> WifiController<'static> {
        recapture_controller().await
    }

    /// Update Access Point Configuration
    pub async fn update_ap_config(&mut self, ap_config: AccessPointConfiguration) {
        // update_ap_config(ap_config).await;
        self.ap_config = ap_config;
    }

    /// Retrieve the latest available CSI data packet
    /// This method does not work if the Access Point is in Monitor Mode
    pub async fn get_csi_data(&mut self) -> Result<CSIDataPacket> {
        match self.op_mode {
            ApOperationMode::Monitor => Err(crate::error::Error::SystemError(
                "get_csi_data_raw() not supported in Monitor Mode",
            )),
            _ => match reconstruct_csi_from_udp().await {
                Ok(csi_pkt) => Ok(csi_pkt),
                Err(_e) => Err(crate::error::Error::SystemError(
                    "Error reconstructing recieved CSI message",
                )),
            },
        }
    }

    /// Retrieve the latest recieved CSI unprocessed raw data packet
    /// This method does not work if the Access Point is in Monitor Mode
    pub async fn get_csi_data_raw(&mut self) -> Result<Vec<u8, 625>> {
        match self.op_mode {
            ApOperationMode::Monitor => Err(crate::error::Error::SystemError(
                "get_csi_data_raw() not supported in Monitor Mode",
            )),
            _ => {
                // Wait for CSI data packet to update
                let csi_raw_pkt = CSI_UDP_RAW_CH.receive().await;
                Ok(csi_raw_pkt)
            }
        }
    }

    /// Print the latest CSI data with metadata to console
    pub async fn print_csi_w_metadata(&mut self) {
        // Wait for CSI data packet to update
        let proc_csi_data = reconstruct_csi_from_udp().await.unwrap();

        // Print the CSI data to console
        proc_csi_data.print_csi_w_metadata();
    }
}

#[embassy_executor::task]
pub async fn ap_connection() {
    // Capture Controller
    let controller_rx = CONTROLLER_CH.receiver();
    let mut start_collection_watch = match START_COLLECTION.receiver() {
        Some(r) => r,
        None => panic!("Maximum number of recievers reached"),
    };
    // Define Events to Listen For
    let ap_events =
        enum_set!(WifiEvent::ApStaconnected | WifiEvent::ApStadisconnected | WifiEvent::ApStop);
    // Connection Loop
    loop {
        // Wait for collection signal to start
        while !start_collection_watch.changed().await {
            Timer::after(Duration::from_millis(100)).await;
        }
        let mut controller = controller_rx.receive().await;
        // println!("AP Connection task active.");
        loop {
            // Events Future
            let wait_event_fut = controller.wait_for_events(ap_events, true);
            // Stop Collection Future
            let stop_coll_fut = start_collection_watch.changed();

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
                    CONTROLLER_HALTED_SIGNAL.signal(true);
                    break; // Break inner loop, and go wait again for start and new controller
                }
            }
        }
    }
}

// This task manages network operations for Access Point in Trigger mode
#[embassy_executor::task]
pub async fn ap_icmp_trigger(stack: Stack<'static>, config: ApTriggerConfig) {
    let mut start_collection_watch = match START_COLLECTION.receiver() {
        Some(r) => r,
        None => panic!("Maximum number of recievers reached"),
    };
    // Trigger Mode triggers CSI collection by sending ICMP packets at defined frequency
    let mut rx_buffer = [0; 64];
    let mut tx_buffer = [0; 64];
    let mut rx_meta: [RawPacketMetadata; 1] = [RawPacketMetadata::EMPTY; 1];
    let mut tx_meta: [RawPacketMetadata; 1] = [RawPacketMetadata::EMPTY; 1];

    let raw_socket = RawSocket::new::<WifiDevice<'_>>(
        stack,
        IpVersion::Ipv4,
        IpProtocol::Icmp,
        &mut rx_meta,
        &mut rx_buffer,
        &mut tx_meta,
        &mut tx_buffer,
    );

    loop {
        while !start_collection_watch.changed().await {
            Timer::after(Duration::from_millis(100)).await;
        }
        println!("ICMP Trigger Task: Collection Started");

        // Station Trigger supports sending ICMP Echo Requests as trigger packets at defined frequency
        let trigger_interval = Duration::from_millis((1000 / config.trigger_freq_hz).into());
        // Buffer to hold ICMP Packet
        let mut icmp_buffer = [0u8; 12];

        // Get Sequence Number
        let mut seq_num = config.trigger_seq_num;

        // Start sending trigger packets
        loop {
            // Trigger Interval Future
            let trigger_timer_fut = Timer::after(trigger_interval);
            // Stop Trigger Future
            let stop_coll_fut = start_collection_watch.changed();

            match select(trigger_timer_fut, stop_coll_fut).await {
                Either::First(_) => {
                    // Create ICMP Packet
                    let mut icmp_packet = Icmpv4Packet::new_unchecked(&mut icmp_buffer[..]);

                    // Create an ICMPv4 Echo Request
                    let icmp_repr = Icmpv4Repr::EchoRequest {
                        ident: 0x22b,
                        seq_no: seq_num,
                        data: &[0xDE, 0xAD, 0xBE, 0xEF],
                    };

                    // Serialize the ICMP representation into the packet
                    icmp_repr.emit(&mut icmp_packet, &ChecksumCapabilities::default());

                    // Buffer for the full IPv4 packet
                    let mut tx_ipv4_buffer = [0u8; 64];

                    // Access Point IP Information
                    let gw_ip_addr_str = "192.168.2.1";
                    let gw_ip_addr =
                        Ipv4Addr::from_str(gw_ip_addr_str).expect("failed to parse gateway ip");

                    // Destination IP Information
                    let dest_ip_addr = match config.trigger_type {
                        TriggerType::Broadcast => Ipv4Addr::from_str("192.168.2.255")
                            .expect("failed to parse destination ip"),
                        TriggerType::Unicast(ip_str) => {
                            // Parse IP Address
                            Ipv4Addr::from_str(ip_str).expect("failed to parse destination ip")
                        }
                    };

                    // Define the IPv4 representation
                    let ipv4_repr = Ipv4Repr {
                        src_addr: gw_ip_addr,
                        dst_addr: dest_ip_addr,
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

                    // Send raw packet
                    raw_socket.send(ipv4_packet_buffer).await;

                    // Increment sequence number for next packet
                    seq_num = seq_num.wrapping_add(1);

                    // Wait for user specified duration
                    Timer::after(trigger_interval).await;
                }
                Either::Second(sig) => {
                    if !sig {
                        println!("Halting Triggers to Connected Stations");
                        break;
                    }
                }
            }
        }
    }
}

// Task to Process the Recieved UDP Packets
#[embassy_executor::task]
pub async fn ap_udp_receiver(ap_stack: Stack<'static>, config: ApTriggerConfig) {
    // Flow in trigger mode to recieve and process incoming UDP packets that contain CSI data

    let mut udp_rx_buffer = [0; 1024];
    let mut udp_tx_buffer = [0; 1024];
    let mut udp_rx_meta: [PacketMetadata; 8] = [PacketMetadata::EMPTY; 8];
    let mut udp_tx_meta: [PacketMetadata; 8] = [PacketMetadata::EMPTY; 8];

    let mut socket = UdpSocket::new(
        ap_stack,
        &mut udp_rx_meta,
        &mut udp_rx_buffer,
        &mut udp_tx_meta,
        &mut udp_tx_buffer,
    );

    println!("Binding");
    // Bind to specified local port
    socket.bind(config.local_port).unwrap();

    // Width of message (625) = 2 bytes for seq_no + 1 byte for format + 4 bytes for timestamp + 6 bytes for MAC + 612 bytes for CSI data
    // Expected size is 625 but allocating a larger buffer to be safe
    let mut rx_buf = [0u8; 1024];

    loop {
        // Receive data into the fixed-size array
        match socket.recv_from(&mut rx_buf).await {
            Ok((len, _endpoint)) => {
                if len > 0 {
                    // Copy the received data
                    let data_slice = &rx_buf[..len];

                    match Vec::<u8, 625>::from_slice(data_slice) {
                        Ok(message_u8) => {
                            // Send the received raw CSI data to the global channel
                            CSI_UDP_RAW_CH.send(message_u8).await;
                        }
                        Err(_) => {
                            println!(
                                "Error: Received UDP packet larger than 625 bytes. Len: {}",
                                len
                            );
                        }
                    }
                }
            }
            Err(e) => {
                println!("UDP recv_from error: {:?}", e);
            }
        }
    }
}
