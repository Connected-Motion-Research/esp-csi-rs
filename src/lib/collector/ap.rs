use embassy_executor::Spawner;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::{Channel, Receiver as ChannelReceiver};

use heapless::Vec;
use smoltcp::phy::ChecksumCapabilities;
use smoltcp::wire::{Icmpv4Packet, Icmpv4Repr, Ipv4Packet, Ipv4Repr};

use embassy_futures::select::{select, Either};

use embassy_time::{Duration, Timer};

use embassy_net::{
    raw::{IpProtocol, IpVersion, PacketMetadata as RawPacketMetadata, RawSocket},
    udp::{PacketMetadata, UdpSocket},
    IpAddress, IpEndpoint, Ipv4Address, Ipv4Cidr, Runner, Stack, StackResources, StaticConfigV4,
};

use core::{net::Ipv4Addr, str::FromStr};

use esp_alloc as _;
use esp_backtrace as _;
use esp_println::println;
use esp_wifi::wifi::{AccessPointConfiguration, Configuration, Interfaces};
use esp_wifi::wifi::{WifiController, WifiDevice, WifiEvent};

use crate::error::Result;

use enumset::enum_set;

use crate::net_task;

use crate::{CSIDataPacket, CSI_UDP_RAW_CH, DHCP_COMPLETE, START_COLLECTION_};

macro_rules! mk_static {
    ($t:ty,$val:expr) => {{
        static STATIC_CELL: static_cell::StaticCell<$t> = static_cell::StaticCell::new();
        #[deny(unused_attributes)]
        let x = STATIC_CELL.uninit().write(($val));
        x
    }};
}

static CONTROLLER_CH: Channel<CriticalSectionRawMutex, WifiController<'static>, 1> = Channel::new();

/// Access Point Operation Modes
/// Trigger: Sends trigger packets to stimulate CSI collection.
/// Monitor: Monitors incoming trigger Packets to stimulate CSI collection at the trigger source.
/// IMPORTANT NOTE: Trigger Mode not yet implemented
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
    /// Source Port #
    src_port: u16,
    /// Trigger Channel
    channel: u8,
    /// Trigger Type - Broadcast or Unicast
    trigger_type: TriggerType,
    /// Trigger Sequence Number Start
    trigger_seq_num: u16,
}

impl Default for ApTriggerConfig {
    fn default() -> Self {
        Self {
            trigger_freq_hz: 10,
            src_port: 10789,
            channel: 1,
            trigger_type: TriggerType::Broadcast,
            trigger_seq_num: 0,
        }
    }
}

#[derive(PartialEq, Copy, Clone)]
enum TriggerType {
    Broadcast,
    /// Unicast Trigger Type requires an IP Address
    Unicast(&'static str),
}

/// Driver Struct to Enable CSI collection as an Access Point
pub struct CSIAccessPoint {
    /// Access Point Configuration
    pub ap_config: AccessPointConfiguration,
    /// Operation Mode: Trigger or Monitor
    pub op_mode: ApOperationMode,
    controller_rx: ChannelReceiver<'static, CriticalSectionRawMutex, WifiController<'static>, 1>,
}

impl CSIAccessPoint {
    /// Creates a new `CSIAccessPoint` instance with a defined configuration/profile.
    pub fn new(ap_config: AccessPointConfiguration, op_mode: ApOperationMode) -> Self {
        let controller_rx = CONTROLLER_CH.receiver();
        Self {
            ap_config,
            op_mode,
            controller_rx,
        }
    }

    /// Creates a new `CSIAccessPoint` instance with defaults.
    pub fn new_with_defaults() -> Self {
        let controller_rx = CONTROLLER_CH.receiver();
        Self {
            ap_config: AccessPointConfiguration::default(),
            op_mode: ApOperationMode::Monitor,
            controller_rx: controller_rx,
        }
    }

    /// Updates `CSIAccessPoint` AP Configuration
    pub fn update_config(&mut self, ap_config: AccessPointConfiguration) {
        self.ap_config = ap_config;
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

        // Create Network Stack
        let (ap_stack, ap_runner) = embassy_net::new(
            ap_interface,
            ap_ip_config,
            mk_static!(StackResources<6>, StackResources::<6>::new()),
            seed,
        );

        // Spawn connection, runner, and DHCP server tasks
        spawner.spawn(net_task(ap_runner)).ok();
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

        // Wait for DHCP to complete
        DHCP_COMPLETE.wait().await;
        // Initialization Finished
        println!("Access Point Initialized");
        Ok(())
    }

    /// Starts the Access Point & Loads Configuration
    /// To reconfigure AP settings, no need to reinit, only call start again with the updated configuration.
    pub async fn start(&self, mut controller: WifiController<'static>) {
        let config = Configuration::AccessPoint(self.ap_config.clone());
        match controller.set_configuration(&config) {
            Ok(_) => println!("WiFi Configuration Set: {:?}", config),
            Err(_) => {
                println!("WiFi Configuration Error");
                println!("Error Config: {:?}", config);
            }
        }

        if !matches!(controller.is_started(), Ok(true)) {
            let config = Configuration::AccessPoint(self.ap_config.clone());
            controller.set_configuration(&config).unwrap();
            println!("Starting wifi");
            controller.start_async().await.unwrap();
            println!("Wifi started!");
        }

        // Signal Collection Start
        START_COLLECTION_.signal(true);
        // Share WiFi Controller to Global Context
        CONTROLLER_CH.send(controller).await;
    }

    /// Stops Collection & Returns WiFi Controller Instance
    pub async fn stop(&self) -> WifiController<'static> {
        START_COLLECTION_.signal(false);
        self.controller_rx.receive().await
    }

    /// Recaptures WiFi Controller Instance
    pub async fn recapture_controller(&self) -> WifiController<'static> {
        self.controller_rx.receive().await
    }

    /// Retrieve the latest available CSI data packet
    /// This method does not work if the Access Point is in Monitor Mode
    pub async fn get_csi_data(&mut self) -> Result<CSIDataPacket> {
        match self.op_mode {
            ApOperationMode::Monitor => Err(crate::error::Error::SystemError(
                "get_csi_data_raw() not supported in Monitor Mode",
            )),
            _ => {
                // Wait for CSI data packet to update
                todo!();
            }
        }
    }

    /// Retrieve the latest recieved CSI unprocessed raw data packet
    /// This method does not work if the Access Point is in Monitor Mode
    pub async fn get_csi_data_raw(&mut self) -> Result<Vec<u8, 619>> {
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
    DHCP_COMPLETE.signal(());
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

#[embassy_executor::task]
pub async fn ap_connection() {
    // Capture Controller
    let controller_rx = CONTROLLER_CH.receiver();
    // Define Events to Listen For
    let ap_events =
        enum_set!(WifiEvent::ApStaconnected | WifiEvent::ApStadisconnected | WifiEvent::ApStop);
    // Connection Loop
    loop {
        // Wait for collection signal to start
        while !START_COLLECTION_.wait().await {
            Timer::after(Duration::from_millis(100)).await;
        }
        let mut controller = controller_rx.receive().await;
        // println!("AP Connection task active.");
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
                    // println!("AP Connection task stopping...");
                    // Send the controller back before we exit the inner loop
                    CONTROLLER_CH.send(controller).await;
                    break; // Break inner loop, and go wait again for start and new controller
                }
            }
        }
    }
}

// This task manages network operations for Access Point in Trigger mode
#[embassy_executor::task]
async fn ap_icmp_trigger(stack: Stack<'static>, config: ApTriggerConfig) {
    // Trigger Mode triggers CSI collection by sending ICMP packets at defined frequency
    // ------------------ ICMP Socket Setup ------------------
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
    // Station Trigger supports sending ICMP Echo Requests as trigger packets at defined frequency
    let trigger_interval = Duration::from_millis((1000 / config.trigger_freq_hz).into());
    // Buffer to hold ICMP Packet
    let mut icmp_buffer = [0u8; 12];

    // Create ICMP Packet
    let mut icmp_packet = Icmpv4Packet::new_unchecked(&mut icmp_buffer[..]);

    // Create an ICMPv4 Echo Request
    let icmp_repr = Icmpv4Repr::EchoRequest {
        ident: 0x22b,
        seq_no: config.trigger_seq_num,
        data: &[0xDE, 0xAD, 0xBE, 0xEF],
    };

    // Serialize the ICMP representation into the packet
    icmp_repr.emit(&mut icmp_packet, &ChecksumCapabilities::default());

    // Buffer for the full IPv4 packet
    let mut tx_ipv4_buffer = [0u8; 64];

    // Access Point IP Information
    let gw_ip_addr_str = "192.168.2.1";
    let gw_ip_addr = Ipv4Addr::from_str(gw_ip_addr_str).expect("failed to parse gateway ip");

    // Destination IP Information
    let dest_ip_addr = match config.trigger_type {
        TriggerType::Broadcast => {
            Ipv4Addr::from_str("192.168.2.255").expect("failed to parse destination ip")
        }
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

    // Start sending trigger packets
    loop {
        // Send raw packet
        raw_socket.send(ipv4_packet_buffer).await;

        // Wait for user specified duration
        Timer::after(trigger_interval).await;
    }
}

// Task to Process the Recieved UDP Packets
#[embassy_executor::task]
async fn ap_udp_receiver(ap_stack: Stack<'static>, config: ApTriggerConfig) {
    // Flow in trigger mode to recieve and process incoming UDP packets that contain CSI data

    // ------------------ UDP Socket Setup ------------------
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
    // Bind to specified source port
    socket.bind(config.src_port).unwrap();

    // Width of message (619) = 2 bytes for seq_no + 1 byte for format + 4 bytes for timestamp + 612 bytes for CSI data
    let mut message_u8: Vec<u8, 619> = Vec::new();

    loop {
        socket.recv_from(&mut message_u8).await.unwrap();
        // Semd the recieved raw CSI data to the global channel for processing
        CSI_UDP_RAW_CH.send(message_u8.clone()).await;
        // Clear the message buffer for next recieve
        message_u8.clear();
    }
}

async fn reconstruct_csi_from_udp() {
    // Retrive the new CSI raw data from UDP channel
    let _raw_csi_data = CSI_UDP_RAW_CH.receive().await;
    // Process the raw CSI data as needed
    todo!()
}
