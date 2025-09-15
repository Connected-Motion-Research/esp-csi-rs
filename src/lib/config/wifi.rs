use heapless::String;

/// WiFi Configuration
#[derive(Debug, Clone)]
pub struct WiFiConfig {
    /// Network SSID
    pub ssid: String<32>,
    /// Network password
    pub password: String<64>,
    /// Access point SSID
    pub ap_ssid: String<32>,
    /// Access point password
    pub ap_password: String<64>,
    /// Access point max connections
    pub max_connections: u16,
    /// Hide SSID in AP Mode
    pub ssid_hidden: bool,
    /// WiFi channel (1-14)
    pub channel: u8,
    /// Maximum number of retries for connection
    pub max_retries: u32,
    /// Connection timeout in seconds
    pub timeout_secs: u32,
}

impl Default for WiFiConfig {
    /// Default implementation for WiFi Configuration:
    /// - Sniffer Mode
    /// - All ssids and passwords are empty
    /// - Maximum allowed connections (if AP) is 1
    /// - SSID is not hidden (if AP)
    /// - Preffered channel number 1
    /// - Maxiumum number of connection retries is 5
    /// - Timeout period before a retry is 30 secs
    fn default() -> Self {
        Self {
            ssid: String::new(),
            password: String::new(),
            ap_ssid: String::new(),
            ap_password: String::new(),
            max_connections: 1,
            ssid_hidden: false,
            channel: 1,
            max_retries: 5,
            timeout_secs: 30,
        }
    }
}
