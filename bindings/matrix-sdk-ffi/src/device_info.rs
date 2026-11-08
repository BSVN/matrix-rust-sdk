use ruma::api::client::device::Device;

/// A device (session) of the current account, as reported by the homeserver.
///
/// It deliberately contains no access token or any key material.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct DeviceInfo {
    pub device_id: String,
    pub display_name: Option<String>,
    pub last_seen_ip: Option<String>,
    /// Last time the device was seen, in milliseconds since the Unix epoch.
    pub last_seen_ts: Option<u64>,
}

impl From<Device> for DeviceInfo {
    fn from(device: Device) -> Self {
        Self {
            device_id: device.device_id.to_string(),
            display_name: device.display_name,
            last_seen_ip: device.last_seen_ip,
            last_seen_ts: device.last_seen_ts.map(|ts| u64::from(ts.0)),
        }
    }
}

#[cfg(test)]
mod tests {
    use ruma::{MilliSecondsSinceUnixEpoch, UInt};

    use super::*;

    #[test]
    fn converts_device_with_all_fields() {
        let mut device = Device::new("DEVICE1".into());
        device.display_name = Some("Pixel 8".to_owned());
        device.last_seen_ip = Some("10.0.0.1".to_owned());
        device.last_seen_ts =
            Some(MilliSecondsSinceUnixEpoch(UInt::try_from(1_700_000_000_000_u64).unwrap()));

        let info = DeviceInfo::from(device);

        assert_eq!(info.device_id, "DEVICE1");
        assert_eq!(info.display_name.as_deref(), Some("Pixel 8"));
        assert_eq!(info.last_seen_ip.as_deref(), Some("10.0.0.1"));
        assert_eq!(info.last_seen_ts, Some(1_700_000_000_000));
    }

    #[test]
    fn converts_device_with_only_device_id() {
        let info = DeviceInfo::from(Device::new("DEVICE2".into()));

        assert_eq!(info.device_id, "DEVICE2");
        assert_eq!(info.display_name, None);
        assert_eq!(info.last_seen_ip, None);
        assert_eq!(info.last_seen_ts, None);
    }
}
