use ruma::api::client::device::Device;

/// The verification state of a device, as known by the local crypto store.
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum DeviceVerificationState {
    /// The device is verified.
    Verified,
    /// The device is known but not verified.
    NotVerified,
    /// The crypto store does not know this device, or it could not be read.
    Unknown,
}

impl DeviceVerificationState {
    /// Map the result of a crypto store lookup.
    ///
    /// `Some(is_verified)` means the device was found, `None` means it was not
    /// found or the lookup failed.
    pub fn from_lookup(is_verified: Option<bool>) -> Self {
        match is_verified {
            Some(true) => Self::Verified,
            Some(false) => Self::NotVerified,
            None => Self::Unknown,
        }
    }
}

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
    /// Verification state of the device. `Unknown` until it is looked up in
    /// the crypto store.
    pub verification_state: DeviceVerificationState,
}

impl DeviceInfo {
    /// Return the same info with the given verification state.
    pub fn with_verification_state(mut self, state: DeviceVerificationState) -> Self {
        self.verification_state = state;
        self
    }
}

impl From<Device> for DeviceInfo {
    fn from(device: Device) -> Self {
        Self {
            device_id: device.device_id.to_string(),
            display_name: device.display_name,
            last_seen_ip: device.last_seen_ip,
            last_seen_ts: device.last_seen_ts.map(|ts| u64::from(ts.0)),
            verification_state: DeviceVerificationState::Unknown,
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
        assert_eq!(info.verification_state, DeviceVerificationState::Unknown);
    }

    #[test]
    fn converts_device_with_only_device_id() {
        let info = DeviceInfo::from(Device::new("DEVICE2".into()));

        assert_eq!(info.device_id, "DEVICE2");
        assert_eq!(info.display_name, None);
        assert_eq!(info.last_seen_ip, None);
        assert_eq!(info.last_seen_ts, None);
        assert_eq!(info.verification_state, DeviceVerificationState::Unknown);
    }

    #[test]
    fn maps_a_verified_device() {
        assert_eq!(
            DeviceVerificationState::from_lookup(Some(true)),
            DeviceVerificationState::Verified
        );
    }

    #[test]
    fn maps_a_known_but_unverified_device() {
        assert_eq!(
            DeviceVerificationState::from_lookup(Some(false)),
            DeviceVerificationState::NotVerified
        );
    }

    #[test]
    fn maps_an_unknown_device() {
        assert_eq!(DeviceVerificationState::from_lookup(None), DeviceVerificationState::Unknown);
    }

    #[test]
    fn with_verification_state_only_changes_the_state() {
        let info = DeviceInfo::from(Device::new("DEVICE3".into()))
            .with_verification_state(DeviceVerificationState::Verified);

        assert_eq!(info.device_id, "DEVICE3");
        assert_eq!(info.verification_state, DeviceVerificationState::Verified);
    }
}
