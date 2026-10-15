//! Power-source awareness for the background loop (issue #368, plan phase P5).
//!
//! On macOS kellner asks `pmset -g batt` whether the machine currently draws
//! from the battery or from AC; everywhere else the mode is [`PowerMode::Unknown`]
//! and treated like AC. The detected mode feeds [`interval_for`], which stretches
//! the poll base interval on battery (and further while idle) so the tray/service
//! stays battery-friendly (cf. `plans/proposed/battery-friendly-tray.md`).
//!
//! The parser is a pure function over the `pmset` text, so it is unit-testable
//! without macOS; only [`PowerMode::detect`] touches the OS, and only there.

use std::time::Duration;

/// Where the machine gets its power from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)] // Battery/AcPower are only constructed on macOS.
pub enum PowerMode {
    /// Running off the internal battery.
    Battery,
    /// Connected to mains power.
    AcPower,
    /// Not determinable (every non-macOS host, or an unparseable `pmset`).
    Unknown,
}

/// Hard floor for any poll interval — never spin faster than this.
pub const MIN_INTERVAL: Duration = Duration::from_secs(30);

impl PowerMode {
    /// Detect the current power source.
    ///
    /// Only macOS shells out (`pmset -g batt`); every other platform returns
    /// [`PowerMode::Unknown`] without spawning a process.
    pub fn detect() -> Self {
        #[cfg(target_os = "macos")]
        {
            macos_detect()
        }
        #[cfg(not(target_os = "macos"))]
        {
            PowerMode::Unknown
        }
    }
}

/// Parse the `pmset -g batt` output.
///
/// The authoritative line is `Now drawing from 'Battery Power'` /
/// `Now drawing from 'AC Power'`. Anything else (empty, garbage, a locale we do
/// not know) yields [`PowerMode::Unknown`] instead of a panic.
#[allow(dead_code)] // only invoked by the macOS detector and by tests.
pub fn parse_pmset(output: &str) -> PowerMode {
    for line in output.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("Now drawing from") else {
            continue;
        };
        if rest.contains("Battery Power") {
            return PowerMode::Battery;
        }
        if rest.contains("AC Power") {
            return PowerMode::AcPower;
        }
    }
    // Some pmset builds omit the "Now drawing from" prefix; fall back to a
    // relaxed scan so a slightly different wording still classifies.
    if output.contains("Battery Power") {
        PowerMode::Battery
    } else if output.contains("AC Power") {
        PowerMode::AcPower
    } else {
        PowerMode::Unknown
    }
}

#[cfg(target_os = "macos")]
fn macos_detect() -> PowerMode {
    match std::process::Command::new("pmset")
        .args(["-g", "batt"])
        .output()
    {
        Ok(out) if out.status.success() => {
            parse_pmset(&String::from_utf8_lossy(&out.stdout))
        }
        _ => PowerMode::Unknown,
    }
}

/// The poll interval to use for a `base` interval under `mode`.
///
/// Multipliers: AC+active `1x`, AC+idle `2x`, battery+active `3x`,
/// battery+idle `6x`; `Unknown` behaves like AC. The result is never smaller
/// than `base` nor than [`MIN_INTERVAL`], so the mapping is monotone:
/// `battery+idle >= battery+active >= AC+active`.
pub fn interval_for(mode: PowerMode, base: Duration, idle: bool) -> Duration {
    let multiplier: u32 = match (mode, idle) {
        (PowerMode::Battery, true) => 6,
        (PowerMode::Battery, false) => 3,
        (PowerMode::AcPower, true) => 2,
        (PowerMode::AcPower, false) => 1,
        (PowerMode::Unknown, true) => 2,
        (PowerMode::Unknown, false) => 1,
    };
    let scaled = base * multiplier;
    if scaled < MIN_INTERVAL {
        MIN_INTERVAL
    } else {
        scaled
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_battery_line() {
        let out = "Now drawing from 'Battery Power'\n -InternalBattery-0 (id=1)\t72%; discharging; 3:20 remaining\n";
        assert_eq!(parse_pmset(out), PowerMode::Battery);
    }

    #[test]
    fn parses_ac_line() {
        let out = "Now drawing from 'AC Power'\n -InternalBattery-0 (id=1)\t100%; charged; 0:00 remaining\n";
        assert_eq!(parse_pmset(out), PowerMode::AcPower);
    }

    #[test]
    fn empty_and_garbage_are_unknown() {
        assert_eq!(parse_pmset(""), PowerMode::Unknown);
        assert_eq!(parse_pmset("hello world\nno juice here"), PowerMode::Unknown);
    }

    #[test]
    fn detect_never_panics() {
        // Non-macOS hosts must return Unknown without shelling out.
        let _ = PowerMode::detect();
    }

    #[test]
    fn interval_scales_monotonically() {
        let base = Duration::from_secs(300);
        let ac_active = interval_for(PowerMode::AcPower, base, false);
        let ac_idle = interval_for(PowerMode::AcPower, base, true);
        let bat_active = interval_for(PowerMode::Battery, base, false);
        let bat_idle = interval_for(PowerMode::Battery, base, true);
        assert_eq!(ac_active, base);
        assert!(ac_idle >= ac_active);
        assert!(bat_active >= ac_idle);
        assert!(bat_idle >= bat_active);
        assert_eq!(bat_idle, base * 6);
    }

    #[test]
    fn unknown_behaves_like_ac() {
        let base = Duration::from_secs(300);
        assert_eq!(
            interval_for(PowerMode::Unknown, base, true),
            interval_for(PowerMode::AcPower, base, true)
        );
        assert_eq!(
            interval_for(PowerMode::Unknown, base, false),
            interval_for(PowerMode::AcPower, base, false)
        );
    }

    #[test]
    fn floor_applies_and_is_never_below_base() {
        let tiny = Duration::from_secs(1);
        assert_eq!(interval_for(PowerMode::AcPower, tiny, false), MIN_INTERVAL);
        let base = Duration::from_secs(60);
        assert!(interval_for(PowerMode::AcPower, base, false) >= base);
    }
}
