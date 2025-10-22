use embassy_executor::Spawner;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::{Channel, Receiver as ChannelReceiver};

use embassy_time::{Duration, Timer};

use embassy_net::{Ipv4Address, Ipv4Cidr, Stack, StackResources, StaticConfigV4};

use core::{net::Ipv4Addr, str::FromStr};

use esp_alloc as _;
use esp_backtrace as _;
use esp_println::println;
use esp_wifi::wifi::WifiController;
use esp_wifi::wifi::{AccessPointConfiguration, ClientConfiguration, Configuration, Interfaces};

use crate::error::Result;

use crate::collector::ap::{ap_connection, ApOperationMode};
use crate::{net_task, run_dhcp_server, IpInfo};

use crate::{DHCP_CLIENT_INFO, DHCP_COMPLETE, NET_TASK_COMPLETE, PROC_CSI_DATA, START_COLLECTION_};

macro_rules! mk_static {
    ($t:ty,$val:expr) => {{
        static STATIC_CELL: static_cell::StaticCell<$t> = static_cell::StaticCell::new();
        #[deny(unused_attributes)]
        let x = STATIC_CELL.uninit().write(($val));
        x
    }};
}

static CONTROLLER_CH: Channel<CriticalSectionRawMutex, WifiController<'static>, 1> = Channel::new();

/// Driver Struct to Collect CSI as a Access Point + Station
pub struct CSIAccessPointStation {
    /// Access Point Configuration
    pub ap_config: AccessPointConfiguration,
    /// Station/Client Configuration
    pub sta_config: ClientConfiguration,
    /// Operation Mode: Trigger or Monitor
    pub op_mode: ApOperationMode,
    controller_rx: ChannelReceiver<'static, CriticalSectionRawMutex, WifiController<'static>, 1>,
}

impl CSIAccessPointStation {
    /// Creates a new `CSIAccessPointStation` instance with a defined configuration/profile.
    pub fn new(
        ap_config: AccessPointConfiguration,
        sta_config: ClientConfiguration,
        op_mode: ApOperationMode,
    ) -> Self {
        let controller_rx = CONTROLLER_CH.receiver();
        Self {
            ap_config,
            sta_config,
            op_mode,
            controller_rx,
        }
    }

    /// Creates a new `CSISniffer` instance with defaults.
    pub fn new_with_defaults() -> Self {
        let _proc_csi_data_rx = PROC_CSI_DATA.receiver().unwrap();
        let controller_rx = CONTROLLER_CH.receiver();
        Self {
            ap_config: AccessPointConfiguration::default(),
            sta_config: ClientConfiguration::default(),
            op_mode: ApOperationMode::Monitor,
            controller_rx: controller_rx,
        }
    }

    /// Updates `CSIAccessPointStation` AP Configuration
    pub fn update_ap_config(&mut self, ap_config: AccessPointConfiguration) {
        self.ap_config = ap_config;
    }

    /// Updates `CSIAccessPointStation` STA Configuration
    pub fn update_sta_config(&mut self, sta_config: ClientConfiguration) {
        self.sta_config = sta_config;
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

        // Create AP Network Stack
        let (ap_stack, ap_runner) = embassy_net::new(
            ap_interface,
            ap_ip_config,
            mk_static!(StackResources<3>, StackResources::<3>::new()),
            seed,
        );

        // Station IP Configuration - DHCP
        let sta_ip_config = embassy_net::Config::dhcpv4(Default::default());
        let sta_interface = interface.sta;

        // Create STA Network Stack
        let (sta_stack, sta_runner) = embassy_net::new(
            sta_interface,
            sta_ip_config,
            mk_static!(StackResources<6>, StackResources::<6>::new()),
            seed,
        );

        // Spawn the network runner tasks to run DHCP
        spawner.spawn(net_task(ap_runner)).ok();
        spawner.spawn(net_task(sta_runner)).ok();

        // Run DHCP Client for STA to acquire IP
        run_dhcp_client(sta_stack).await;

        // Spawn the CSI processing task and AP Connection Management task
        spawner
            .spawn(run_dhcp_server(ap_stack, gw_ip_addr_str))
            .ok();
        spawner.spawn(ap_connection()).ok();

        // Wait for Net Task to Complete
        // NET_TASK_COMPLETE.wait().await;
        // Wait for DHCP Server to Run before finishing init
        DHCP_COMPLETE.wait().await;
        // No need to wait on DHCP since its awaited in init above
        // Initialization Finished
        println!("Access Point Initialized");

        Ok(())
    }

    /// Starts the Access Point + Station & Loads Configuration
    /// To reconfigure AP/STA settings, no need to reinit, only call start again with the updated configuration.
    pub async fn start(&self, mut controller: WifiController<'static>) {
        let config = Configuration::Mixed(self.sta_config.clone(), self.ap_config.clone());
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

// #[embassy_executor::task]
pub async fn run_dhcp_client(sta_stack: Stack<'static>) {
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
