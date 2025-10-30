use embassy_executor::Spawner;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Receiver as ChannelReceiver;
use embassy_sync::watch::Receiver;

use esp_alloc as _;
use esp_backtrace as _;
use esp_println::println;
use esp_wifi::wifi::Interfaces;
use esp_wifi::wifi::WifiController;

use crate::error::Result;

use crate::CSIConfig;
use crate::{build_csi_config, capture_csi_info, process_csi_packet};

use crate::{
    CSIDataPacket, CONTROLLER_CH, CSI_CONFIG_CH, MAC_FIL_CH, PROC_CSI_DATA, START_COLLECTION_,
};

/// Driver Struct to Collect CSI as a Sniffer
pub struct CSISniffer {
    /// CSI Collection Parameters
    pub csi_config: CSIConfig,
    /// MAC Address Filter for CSI Data
    pub mac_filter: Option<[u8; 6]>,
    csi_data_rx: Receiver<'static, CriticalSectionRawMutex, CSIDataPacket, 3>,
    controller_rx: ChannelReceiver<'static, CriticalSectionRawMutex, WifiController<'static>, 1>,
}

impl CSISniffer {
    /// Creates a new `CSISniffer` instance with a defined configuration/profile.
    pub fn new(csi_config: CSIConfig, mac_filter: Option<[u8; 6]>) -> Self {
        let csi_data_rx = PROC_CSI_DATA.receiver().unwrap();
        let controller_rx = CONTROLLER_CH.receiver();
        Self {
            csi_config,
            mac_filter,
            csi_data_rx,
            controller_rx,
        }
    }

    /// Creates a new `CSISniffer` instance with defaults.
    pub fn new_with_defaults() -> Self {
        let proc_csi_data_rx = PROC_CSI_DATA.receiver().unwrap();
        let controller_rx = CONTROLLER_CH.receiver();
        Self {
            csi_config: CSIConfig::default(),
            mac_filter: None,
            csi_data_rx: proc_csi_data_rx,
            controller_rx: controller_rx,
        }
    }

    /// Initialize WiFi and the CSI Collection System. This method starts the WiFi connection and spawns the required tasks.
    pub async fn init(&self, interface: Interfaces<'static>, spawner: &Spawner) -> Result<()> {
        println!("Initializing Sniffer");
        // Create sniffer instance
        let sniffer = &interface.sniffer;
        sniffer.set_promiscuous_mode(true).unwrap();
        // Spawn the CSI processing task
        spawner.spawn(process_csi_packet()).ok();
        // Spawn controller
        spawner.spawn(sniffer_task()).ok();
        Ok(())
    }

    /// Starts CSI Collection
    pub async fn start(&self, controller: WifiController<'static>) {
        // Send Updated Configs
        CSI_CONFIG_CH.send(self.csi_config.clone()).await;
        MAC_FIL_CH.send(self.mac_filter).await;
        // Share WiFi Controller to Global Context
        // This is such that the controller can be returned if the Collection is deinitalized
        CONTROLLER_CH.send(controller).await;
        // Signal start collection
        START_COLLECTION_.signal(true);
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
    pub async fn print_csi_w_metadata(&mut self) {
        // Wait for CSI data packet to update
        let proc_csi_data = self.csi_data_rx.changed().await;

        // Print the CSI data to console
        proc_csi_data.print_csi_w_metadata();
    }
}

#[embassy_executor::task]
async fn sniffer_task() {
    loop {
        // Wait for Start Signal
        START_COLLECTION_.wait().await;
        let mut controller = CONTROLLER_CH.receive().await;
        loop {
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
            // If Start Collection becomes false then collection needs to stop
            let stop_collection = START_COLLECTION_.wait().await;
            if !stop_collection {
                println!("Halting CSI Collection");
                CONTROLLER_CH.send(controller).await;
                break;
            }
        }
    }
}
