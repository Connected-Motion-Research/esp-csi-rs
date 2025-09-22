use embassy_executor::Spawner;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::{Channel, Receiver as ChannelReceiver};
use embassy_sync::watch::Receiver;

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
use esp_wifi::wifi::{WifiController, WifiEvent, WifiState};

use crate::error::Result;

use enumset::enum_set;

use crate::CSIConfig;
use crate::{build_csi_config, capture_csi_info, net_task, process_csi_packet};

use crate::{CSIDataPacket, PROC_CSI_DATA, START_COLLECTION_};

macro_rules! mk_static {
    ($t:ty,$val:expr) => {{
        static STATIC_CELL: static_cell::StaticCell<$t> = static_cell::StaticCell::new();
        #[deny(unused_attributes)]
        let x = STATIC_CELL.uninit().write(($val));
        x
    }};
}

static CNTRLR_CHAN: Channel<CriticalSectionRawMutex, WifiController<'static>, 1> = Channel::new();

/// Driver Struct to Enable CSI collection as an Access Point
pub struct CSIAccessPoint {
    /// Access Point Configuration
    pub ap_config: AccessPointConfiguration,
    controller_rx: ChannelReceiver<'static, CriticalSectionRawMutex, WifiController<'static>, 1>,
}

impl CSIAccessPoint {
    /// Creates a new `CSIAccessPoint` instance with a defined configuration/profile.
    pub fn new(ap_config: AccessPointConfiguration) -> Self {
        let controller_rx = CNTRLR_CHAN.receiver();
        Self {
            ap_config,
            controller_rx,
        }
    }

    /// Creates a new `CSIAccessPoint` instance with defaults.
    pub fn new_with_defaults() -> Self {
        let controller_rx = CNTRLR_CHAN.receiver();
        Self {
            ap_config: AccessPointConfiguration::default(),
            controller_rx: controller_rx,
        }
    }

    /// Initialize WiFi and the CSI Collection System. This method starts the WiFi connection and spawns the required tasks.
    pub async fn init(
        &self,
        mut controller: WifiController<'static>,
        interface: Interfaces<'static>,
        spawner: &Spawner,
    ) -> Result<()> {
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

        let config = Configuration::AccessPoint(self.ap_config.clone());
        match controller.set_configuration(&config) {
            Ok(_) => println!("WiFi Configuration Set: {:?}", config),
            Err(_) => {
                println!("WiFi Configuration Error");
                println!("Error Config: {:?}", config);
            }
        }

        // Spawn connection, runner, and DHCP server tasks
        spawner.spawn(net_task(ap_runner)).ok();
        spawner.spawn(run_dhcp(ap_stack, gw_ip_addr_str)).ok();
        spawner.spawn(ap_connection()).ok();

        if !matches!(controller.is_started(), Ok(true)) {
            let config = Configuration::AccessPoint(self.ap_config.clone());
            controller.set_configuration(&config).unwrap();
            println!("Starting wifi");
            controller.start_async().await.unwrap();
            println!("Wifi started!");
        }

        // Share WiFi Controller to Global Context
        // This is such that the controller can be returned if the Collection is stopped
        CNTRLR_CHAN.send(controller).await;
        println!("Access Point Initialized");

        Ok(())
    }

    /// Starts Access Point
    pub fn start(&self) {
        START_COLLECTION_.signal(true);
    }

    /// Recaptures WiFi Controller Instance
    pub async fn recapture_controller(&self) -> WifiController<'static> {
        self.controller_rx.receive().await
    }
}

// #[embassy_executor::task]
// async fn sniffer_task(mac_filter: Option<[u8; 6]>, csi_config: CSIConfig) {
//     let mut controller = CNTRLR_CHAN.receive().await;
//     loop {
//         // Build CSI Configuration
//         let csi_cfg = build_csi_config(csi_config.clone());
//         // Wait for Start Signal
//         let start_collection = START_COLLECTION_.wait().await;
//         // If Start Collection is false then collection needs to stop
//         if !start_collection {
//             println!("Halting CSI Collection");
//             CNTRLR_CHAN.send(controller).await;
//             break;
//         }
//         println!("Starting CSI Collection");
//         controller
//             .set_csi(csi_cfg, |info: esp_wifi::wifi::wifi_csi_info_t| {
//                 capture_csi_info(info, mac_filter);
//             })
//             .unwrap();
//     }
// }

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

#[embassy_executor::task]
async fn ap_connection() {
    loop {
        let ap_events =
            enum_set!(WifiEvent::ApStaconnected | WifiEvent::ApStadisconnected | WifiEvent::ApStop);

        let mut controller = CNTRLR_CHAN.receive().await;

        while !START_COLLECTION_.wait().await {}

        // Inner loop to handle WiFi state
        loop {
            match esp_wifi::wifi::wifi_state() {
                WifiState::ApStarted => {
                    println!("AP Started, Waiting for Station to Connect...");
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
                                println!(
                                    "AP connection stopped. Attempting to bring up AP again..."
                                );
                                Timer::after(Duration::from_millis(5000)).await;
                                return Ok::<(), ()>(());
                            }
                            event.clear();
                        }
                    };

                    // Run AP Until Event Happens
                    let _ = run_loop.await;
                }
                _ => {
                    println!("Starting WiFi");
                    controller.start_async().await.unwrap();
                    println!("Wifi Started!");
                }
            }
        }
    }
}
