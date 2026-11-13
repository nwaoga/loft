//! Is the computer on mains power or on battery, and when does that change?
//!
//! * **Windows**: the current state comes from `GetSystemPowerStatus`, and
//!   changes arrive as notifications from the power manager (no polling, and
//!   no window needed, so it works while the notch is hidden).
//! * **Linux**: `/sys/class/power_supply` is read every 30 seconds.
//! * **Elsewhere**: unknown, which Loft treats as mains power.
//!
//! This module only reports raw readings. Debouncing them into settled
//! changes is `inference::PowerDebounce`; acting on them is `engine.rs`.

use crate::inference::PowerSource;

/// The power source right now.
pub fn current() -> PowerSource {
    imp::current()
}

/// Call `on_change` with every reading that arrives from now on (duplicates
/// included). The watch lasts for the life of the process.
pub fn watch(on_change: impl Fn(PowerSource) + Send + Sync + 'static) {
    imp::watch(Box::new(on_change));
}

/// How fast the battery is draining, in watts, when the system can tell.
/// Only meaningful on battery: it is the whole computer's draw (screen and
/// all), not that of one processor.
pub fn battery_drain_watts() -> Option<f32> {
    imp::battery_drain_watts()
}

type Callback = Box<dyn Fn(PowerSource) + Send + Sync>;

#[cfg(windows)]
mod imp {
    use super::Callback;
    use crate::inference::PowerSource;
    use std::ffi::c_void;
    use std::sync::OnceLock;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::Power::{
        CallNtPowerInformation, GetSystemPowerStatus, PowerSettingRegisterNotification, SystemBatteryState,
        DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS, POWERBROADCAST_SETTING, SYSTEM_BATTERY_STATE, SYSTEM_POWER_STATUS,
    };
    use windows::Win32::System::SystemServices::GUID_ACDC_POWER_SOURCE;
    use windows::Win32::UI::WindowsAndMessaging::{DEVICE_NOTIFY_CALLBACK, PBT_POWERSETTINGCHANGE};

    pub fn current() -> PowerSource {
        let mut status = SYSTEM_POWER_STATUS::default();
        // SAFETY: `status` is a valid, writable SYSTEM_POWER_STATUS.
        if unsafe { GetSystemPowerStatus(&mut status) }.is_err() {
            return PowerSource::Unknown;
        }
        match status.ACLineStatus {
            0 => PowerSource::Battery,
            1 => PowerSource::Ac,
            _ => PowerSource::Unknown,
        }
    }

    static CALLBACK: OnceLock<Callback> = OnceLock::new();

    /// Called by the power manager on one of its own threads.
    unsafe extern "system" fn notified(_context: *const c_void, kind: u32, setting: *const c_void) -> u32 {
        if kind == PBT_POWERSETTINGCHANGE && !setting.is_null() {
            // SAFETY: for this notification `setting` points at a
            // POWERBROADCAST_SETTING whose data is a 4-byte power condition.
            let setting = unsafe { &*(setting as *const POWERBROADCAST_SETTING) };
            if setting.PowerSetting == GUID_ACDC_POWER_SOURCE && setting.DataLength >= 4 {
                let condition = unsafe { std::ptr::read_unaligned(setting.Data.as_ptr() as *const u32) };
                let source = match condition {
                    0 => PowerSource::Ac,      // PoAc
                    1 => PowerSource::Battery, // PoDc
                    _ => current(),            // PoHot (a UPS): ask again
                };
                if let Some(callback) = CALLBACK.get() {
                    callback(source);
                }
            }
        }
        0
    }

    pub fn watch(on_change: Callback) {
        if CALLBACK.set(on_change).is_err() {
            return; // already watching
        }
        let parameters = DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS {
            Callback: Some(notified),
            Context: std::ptr::null_mut(),
        };
        let mut registration: *mut c_void = std::ptr::null_mut();
        // SAFETY: with DEVICE_NOTIFY_CALLBACK the "recipient" is a pointer to
        // the parameters, which the call copies; `notified` lives for ever.
        let result = unsafe {
            PowerSettingRegisterNotification(
                &GUID_ACDC_POWER_SOURCE,
                DEVICE_NOTIFY_CALLBACK,
                HANDLE(&parameters as *const _ as *mut c_void),
                &mut registration,
            )
        };
        if result.is_err() {
            eprintln!("Loft: power notifications are unavailable ({result:?}); checking once a minute instead");
            std::thread::spawn(|| loop {
                std::thread::sleep(std::time::Duration::from_secs(60));
                if let Some(callback) = CALLBACK.get() {
                    callback(current());
                }
            });
        }
        // The registration is deliberately never released: it lasts as long as Loft.
    }

    pub fn battery_drain_watts() -> Option<f32> {
        let mut state = SYSTEM_BATTERY_STATE::default();
        // SAFETY: the output buffer is a SYSTEM_BATTERY_STATE of the stated size.
        let status = unsafe {
            CallNtPowerInformation(
                SystemBatteryState,
                None,
                0,
                Some(&mut state as *mut _ as *mut c_void),
                std::mem::size_of::<SYSTEM_BATTERY_STATE>() as u32,
            )
        };
        if status.is_err() || !state.BatteryPresent || !state.Discharging {
            return None;
        }
        // `Rate` is in milliwatts, as a signed value in an unsigned field
        // (negative while discharging); 0x80000000 means "unknown".
        let rate = state.Rate as i32;
        if rate == i32::MIN || rate == 0 {
            return None;
        }
        Some(rate.unsigned_abs() as f32 / 1000.0)
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use super::Callback;
    use crate::inference::PowerSource;

    pub fn current() -> PowerSource {
        let Ok(supplies) = std::fs::read_dir("/sys/class/power_supply") else {
            return PowerSource::Unknown;
        };
        let mut mains = None;
        for supply in supplies.flatten() {
            let path = supply.path();
            let kind = std::fs::read_to_string(path.join("type")).unwrap_or_default();
            if kind.trim() != "Mains" && !kind.trim().starts_with("USB") {
                continue;
            }
            match std::fs::read_to_string(path.join("online")).unwrap_or_default().trim() {
                "1" => return PowerSource::Ac,
                "0" => mains = Some(false),
                _ => {}
            }
        }
        match mains {
            Some(false) => PowerSource::Battery,
            _ => PowerSource::Unknown,
        }
    }

    pub fn watch(on_change: Callback) {
        std::thread::spawn(move || loop {
            std::thread::sleep(std::time::Duration::from_secs(30));
            on_change(current());
        });
    }

    pub fn battery_drain_watts() -> Option<f32> {
        let supplies = std::fs::read_dir("/sys/class/power_supply").ok()?;
        for supply in supplies.flatten() {
            let path = supply.path();
            if std::fs::read_to_string(path.join("status")).unwrap_or_default().trim() != "Discharging" {
                continue;
            }
            if let Ok(microwatts) = std::fs::read_to_string(path.join("power_now")).unwrap_or_default().trim().parse::<f32>() {
                if microwatts > 0.0 {
                    return Some(microwatts / 1_000_000.0);
                }
            }
        }
        None
    }
}

#[cfg(not(any(windows, target_os = "linux")))]
mod imp {
    use super::Callback;
    use crate::inference::PowerSource;

    pub fn current() -> PowerSource {
        PowerSource::Unknown
    }
    pub fn watch(_on_change: Callback) {}
    pub fn battery_drain_watts() -> Option<f32> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Prints the reading on this computer; it must not hang or crash.
    #[test]
    fn the_power_source_can_be_read() {
        let source = current();
        eprintln!("power source: {source:?}, battery drain: {:?} W", battery_drain_watts());
        // Reading twice gives the same answer (nobody unplugs mid-test).
        assert_eq!(current(), source);
        if source == PowerSource::Ac {
            assert_eq!(battery_drain_watts(), None, "no drain is reported on mains");
        }
    }
}
