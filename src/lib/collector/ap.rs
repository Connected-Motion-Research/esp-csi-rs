use embassy_executor::Spawner;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::{Channel, Receiver as ChannelReceiver};

use embassy_futures::select::{select, Either};

use embassy_time::{Duration, Timer};

use embassy_net::{Ipv4Cidr, Stack, StackResources, StaticConfigV4};

use core::{net::Ipv4Addr, str::FromStr};

use esp_alloc as _;
use esp_backtrace as _;
use esp_println::println;
use esp_wifi::wifi::{AccessPointConfiguration, Configuration, Interfaces};
use esp_wifi::wifi::{WifiController, WifiEvent};

use crate::error::Result;

use enumset::enum_set;

use crate::net_task;

use crate::{DHCP_COMPLETE, NET_TASK_COMPLETE, START_COLLECTION_};

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
pub enum ApOperationMode {
    Trigger(ApTriggerConfig),
    Monitor,
}

/// Configuration for Access Point Trigger Mode
pub struct ApTriggerConfig {
    /// Trigger Packet Frequency
    trigger_freq_hz: u32,
    /// Source Port #
    src_port: u16,
    /// Destination Port #
    dest_port: u16,
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
            dest_port: 10789,
            channel: 1,
            trigger_type: TriggerType::Broadcast,
            trigger_seq_num: 0,
        }
    }
}

enum TriggerType {
    Broadcast,
    /// Unicast Trigger Type requires a MAC Address
    Unicast([u8; 6]),
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
            .spawn(run_dhcp_server_server(ap_stack, gw_ip_addr_str))
            .ok();
        spawner.spawn(ap_connection()).ok();

        // Wait for Net Task to Complete
        // NET_TASK_COMPLETE.wait().await;
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
}

#[embassy_executor::task]
async fn run_dhcp_server_server(stack: Stack<'static>, gw_ip_addr: &'static str) {
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
