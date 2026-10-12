//! Power-mode detection for macOS battery-aware operation.
//!
//! Detects whether the system is on battery or AC power and publishes
//! the current mode via a `tokio::sync::watch` channel so that all
//! background tasks can adapt their polling intervals.
//!
//! On non-macOS systems the mode is always `Unknown` (treated as AC).

use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

/// Power mode published to background tasks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum PowerMode {
    Unknown = 0,
    Battery = 1,
    AcPower = 2,
}

impl PowerMode {
    /// Detect the current power mode.
    ///
    /// On macOS this shells out to `pmset -g batt` which is fast and
    /// requires no special entitlements. Falls back to `system_profiler`
    /// if `pmset` isn't available.
    ///
    /// On non-macOS always returns `Unknown`.
    pub fn detect() -> Self {
        #[cfg(target_os = "macos")]
        {
            // pmset -g batt outputs lines like:
            //   Now drawing from 'Battery Power'
            //   Now drawing from 'AC Power'
            if let Ok(output) = std::process::Command::new("pmset")
                .args(["-g", "batt"])
                .output()
            {
                let stdout = String::from_utf8_lossy(&output.stdout);
                if stdout.contains("Battery Power") {
                    debug!("Power mode: Battery");
                    return Self::Battery;
                }
                if stdout.contains("AC Power") {
                    debug!("Power mode: AC Power");
                    return Self::AcPower;
                }
            }

            // Fallback: system_profiler SPPowerDataType
            if let Ok(output) = std::process::Command::new("system_profiler")
                .args(["SPPowerDataType"])
                .output()
            {
                let stdout = String::from_utf8_lossy(&output.stdout);
                if stdout.contains("Battery") && !stdout.contains("Connected: Yes") {
                    debug!("Power mode (fallback): Battery");
                    return Self::Battery;
                }
                if stdout.contains("AC Charger") {
                    debug!("Power mode (fallback): AC Power");
                    return Self::AcPower;
                }
            }

            // Default to AC on macOS if we can't determine (safer —
            // prevents unnecessary throttling)
            warn!("Power mode: could not detect, defaulting to AC Power");
            Self::AcPower
        }

        #[cfg(not(target_os = "macos"))]
        {
            debug!("Power mode: Unknown (non-macOS)");
            Self::Unknown
        }
    }

    /// Returns `true` when the system is on battery power.
    pub fn is_battery(self) -> bool {
        matches!(self, Self::Battery)
    }

    /// Returns `true` when the system is on AC power or unknown.
    pub fn is_ac(self) -> bool {
        matches!(self, Self::AcPower | Self::Unknown)
    }
}

impl From<u8> for PowerMode {
    fn from(v: u8) -> Self {
        match v {
            1 => Self::Battery,
            2 => Self::AcPower,
            _ => Self::Unknown,
        }
    }
}

/// Start a background power-mode monitor.
///
/// Checks the power source every `check_interval` seconds and publishes
/// changes via the provided `AtomicU8`. Background tasks should read
/// this value to determine their polling interval.
///
/// The value is encoded as: 0=Unknown, 1=Battery, 2=AcPower.
pub async fn start_power_monitor(
    current_mode: Arc<AtomicU8>,
    check_interval_secs: u64,
    cancel_token: CancellationToken,
) {
    // Initial detection
    let mode = PowerMode::detect();
    current_mode.store(mode as u8, Ordering::Relaxed);
    info!("Power monitor started: mode={:?}", mode);

    loop {
        tokio::select! {
            _ = tokio::time::sleep(std::time::Duration::from_secs(check_interval_secs)) => {}
            _ = cancel_token.cancelled() => {
                info!("Power monitor: cancellation requested, shutting down");
                break;
            }
        }

        if cancel_token.is_cancelled() {
            break;
        }

        let new_mode = PowerMode::detect();
        let old_mode = PowerMode::from(current_mode.load(Ordering::Relaxed));

        if new_mode != old_mode {
            info!(
                "Power mode changed: {:?} → {:?}",
                old_mode, new_mode
            );
            current_mode.store(new_mode as u8, Ordering::Relaxed);
        }
    }

    info!("Power monitor: stopped");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_power_mode_from_u8() {
        assert_eq!(PowerMode::from(0), PowerMode::Unknown);
        assert_eq!(PowerMode::from(1), PowerMode::Battery);
        assert_eq!(PowerMode::from(2), PowerMode::AcPower);
        assert_eq!(PowerMode::from(99), PowerMode::Unknown);
    }

    #[test]
    fn test_is_battery() {
        assert!(PowerMode::Battery.is_battery());
        assert!(!PowerMode::AcPower.is_battery());
        assert!(!PowerMode::Unknown.is_battery());
    }

    #[test]
    fn test_is_ac() {
        assert!(PowerMode::AcPower.is_ac());
        assert!(PowerMode::Unknown.is_ac());
        assert!(!PowerMode::Battery.is_ac());
    }

    #[test]
    fn test_power_mode_clone_copy() {
        let m = PowerMode::Battery;
        let m2 = m; // Copy
        assert_eq!(m, m2);
        let m3 = m.clone(); // Clone
        assert_eq!(m, m3);
    }

    #[test]
    fn test_power_mode_debug() {
        assert_eq!(format!("{:?}", PowerMode::Battery), "Battery");
        assert_eq!(format!("{:?}", PowerMode::AcPower), "AcPower");
        assert_eq!(format!("{:?}", PowerMode::Unknown), "Unknown");
    }
}
