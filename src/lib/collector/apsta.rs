use embassy_executor::Spawner;

use embassy_net::{Ipv4Cidr, StackResources, StaticConfigV4};

use core::{net::Ipv4Addr, str::FromStr};

use esp_alloc as _;
use esp_backtrace as _;
use esp_println::println;
use esp_wifi::wifi::WifiController;
use esp_wifi::wifi::{AccessPointConfiguration, ClientConfiguration, Configuration, Interfaces};

use crate::error::Result;

use crate::collector::ap::{ap_connection, ApOperationMode};
use crate::{
    configure_connection, connect_wifi, net_task, recapture_controller, reconstruct_csi_from_udp,
    run_dhcp_client, run_dhcp_server, start_collection, start_wifi, stop_collection, CSIDataPacket,
    ConnectionType, CSI_UDP_RAW_CH,
};

use crate::collector::{ap_icmp_trigger, ap_udp_receiver};

use heapless::Vec;

use crate::CONTROLLER_CH;

macro_rules! mk_static {
    ($t:ty,$val:expr) => {{
        static STATIC_CELL: static_cell::StaticCell<$t> = static_cell::StaticCell::new();
        #[deny(unused_attributes)]
        let x = STATIC_CELL.uninit().write(($val));
        x
    }};
}

/// Driver Struct to Collect CSI as a Access Point + Station
/// Note: The station purpose in this mode is to connect to a router/access point for internet access
pub struct CSIAccessPointStation {
    /// Operation Mode: Trigger or Monitor
    pub op_mode: ApOperationMode,
    /// Access Point Configuration
    pub ap_config: AccessPointConfiguration,
    /// Station/Client Configuration
    pub sta_config: ClientConfiguration,
}

impl CSIAccessPointStation {
    /// Creates a new `CSIAccessPointStation` instance with a defined configuration/profile.
    pub async fn new(
        ap_config: AccessPointConfiguration,
        sta_config: ClientConfiguration,
        op_mode: ApOperationMode,
        wifi_controller: WifiController<'static>,
    ) -> Self {
        CONTROLLER_CH.send(wifi_controller).await;
        // ACCESSPOINT_CONFIG_CH.send(ap_config.clone()).await;
        // CLIENT_CONFIG_CH.send(sta_config.clone()).await;
        Self {
            op_mode,
            ap_config,
            sta_config,
        }
    }

    /// Creates a new `CSIAccessPointStation` instance with defaults.
    /// 'AccessPointConfiguration' is set to 'default'
    /// 'StationConfiguration' is set to 'default'
    /// 'ApOperationMode' is set to Monitor
    pub async fn new_with_defaults(wifi_controller: WifiController<'static>) -> Self {
        CONTROLLER_CH.send(wifi_controller).await;
        Self {
            op_mode: ApOperationMode::Monitor,
            ap_config: AccessPointConfiguration::default(),
            sta_config: ClientConfiguration::default(),
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

        // Configure WiFi Access Point + Client/Station Connection
        let config = Configuration::Mixed(self.sta_config.clone(), self.ap_config.clone());
        configure_connection(ConnectionType::Mixed, config).await;

        // Start & Connect WiFi
        // This needs to be done before DHCP and NTP
        start_wifi().await;
        connect_wifi().await;

        // Run DHCP Client for STA to acquire IP
        run_dhcp_client(sta_stack).await;

        // Spawn the CSI processing task and AP Connection Management task
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
        println!("Access Point + Station Initialized");

        Ok(())
    }

    /// Starts the Access Point + Station & Loads Configuration
    /// To reconfigure AP/STA settings, no need to reinit, only call start again with the updated configuration.
    pub async fn start_collection(&self) {
        let config = Configuration::Mixed(self.sta_config.clone(), self.ap_config.clone());
        start_collection(crate::ConnectionType::Mixed, config).await;
    }

    /// Stops Collection & Returns WiFi Controller Instance
    pub async fn stop_collection(&self) {
        stop_collection().await;
    }

    /// Recaptures WiFi Controller Instance
    pub async fn recapture_controller(&self) -> WifiController<'static> {
        recapture_controller().await
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

    pub async fn update_ap_config(&mut self, ap_config: AccessPointConfiguration) {
        // update_ap_config(ap_config).await;
        self.ap_config = ap_config;
    }

    pub async fn update_sta_config(&mut self, sta_config: ClientConfiguration) {
        // update_client_config(sta_config).await;
        self.sta_config = sta_config;
    }
}
