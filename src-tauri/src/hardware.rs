//! What processors this computer has, as the operating system reports them:
//! the CPU, the graphics adapters and any neural processor (NPU), with their
//! driver versions. This is independent of which llama.cpp builds are
//! installed, so Loft can say "there is an NPU here, but the engine that uses
//! it is not installed".
//!
//! Windows only for now; elsewhere just the CPU count is known and the list
//! of devices comes from the engine builds themselves.

use crate::inference::{vendor_of, DeviceKind, Hardware, Vendor};

/// The processor's marketing name ("12th Gen Intel(R) Core(TM) i7-12700H").
pub fn cpu_name() -> String {
    imp::cpu_name().unwrap_or_else(|| {
        let cores = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
        format!("CPU ({cores} threads)")
    })
}

/// Graphics adapters and NPUs present, software adapters left out.
pub fn devices() -> Vec<Hardware> {
    imp::devices()
}

/// True when a driver file that a backend needs is present.
pub fn has_library(windows_dll: &str, unix_lib: &str) -> bool {
    #[cfg(target_os = "windows")]
    {
        let _ = unix_lib;
        let root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
        std::path::PathBuf::from(root).join("System32").join(windows_dll).exists()
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = windows_dll;
        ["/usr/lib/x86_64-linux-gnu", "/usr/lib/aarch64-linux-gnu", "/usr/lib64", "/usr/lib"]
            .iter()
            .any(|dir| std::path::PathBuf::from(dir).join(unix_lib).exists())
    }
}

/// Classify an adapter name; used for adapters and for NPUs.
fn hardware(name: String, kind: DeviceKind, driver: Option<String>) -> Hardware {
    let vendor = match (kind, vendor_of(&name)) {
        // "Intel(R) AI Boost" has the vendor in it; other NPU names may not.
        (DeviceKind::Npu, Vendor::Other) => Vendor::Intel,
        (_, vendor) => vendor,
    };
    Hardware {
        name,
        kind,
        vendor,
        driver,
    }
}

#[cfg(windows)]
mod imp {
    use super::hardware;
    use crate::inference::{DeviceKind, Hardware};
    use windows::core::{GUID, HSTRING, PCWSTR};
    use windows::Win32::Devices::DeviceAndDriverInstallation::{
        SetupDiDestroyDeviceInfoList, SetupDiEnumDeviceInfo, SetupDiGetClassDevsW, SetupDiGetDeviceRegistryPropertyW,
        DIGCF_PRESENT, SETUP_DI_REGISTRY_PROPERTY, SPDRP_DEVICEDESC, SPDRP_DRIVER, SPDRP_FRIENDLYNAME, SP_DEVINFO_DATA,
    };
    use windows::Win32::System::Registry::{RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ};

    // Device setup classes. (The graphics adapters are read from here, not
    // from DXGI: loading DXGI alongside the Windows OCR and PDF components
    // keeps the process from exiting.)
    const DISPLAY: GUID = GUID::from_u128(0x4d36e968_e325_11ce_bfc1_08002be10318);
    /// "Neural processors": Intel AI Boost and friends.
    const COMPUTE_ACCELERATOR: GUID = GUID::from_u128(0xf01a9d53_3ff6_48d2_9f97_c8a7004be10c);

    fn utf16(buffer: &[u16]) -> String {
        let end = buffer.iter().position(|c| *c == 0).unwrap_or(buffer.len());
        String::from_utf16_lossy(&buffer[..end]).trim().to_string()
    }

    /// A string value from the registry under HKEY_LOCAL_MACHINE.
    fn registry_string(key: &str, value: &str) -> Option<String> {
        let mut buffer = [0u16; 256];
        let mut size = (buffer.len() * 2) as u32;
        // SAFETY: the buffer is `size` bytes long; the names are NUL-terminated HSTRINGs.
        let result = unsafe {
            RegGetValueW(
                HKEY_LOCAL_MACHINE,
                &HSTRING::from(key),
                &HSTRING::from(value),
                RRF_RT_REG_SZ,
                None,
                Some(buffer.as_mut_ptr().cast()),
                Some(&mut size),
            )
        };
        result.ok().ok()?;
        Some(utf16(&buffer)).filter(|text| !text.is_empty())
    }

    pub fn cpu_name() -> Option<String> {
        registry_string(r"HARDWARE\DESCRIPTION\System\CentralProcessor\0", "ProcessorNameString")
    }

    /// The present devices of one setup class: (name, driver version).
    fn class_devices(class: &GUID) -> Vec<(String, Option<String>)> {
        let mut found = Vec::new();
        // SAFETY: the device-info set is destroyed before returning, and every
        // buffer passed is as long as stated.
        unsafe {
            let Ok(set) = SetupDiGetClassDevsW(Some(class), PCWSTR::null(), None, DIGCF_PRESENT) else {
                return found;
            };
            let property = |info: &SP_DEVINFO_DATA, which: SETUP_DI_REGISTRY_PROPERTY| -> Option<String> {
                let mut bytes = [0u8; 512];
                SetupDiGetDeviceRegistryPropertyW(set, info, which, None, Some(&mut bytes), None).ok()?;
                let wide: Vec<u16> = bytes.chunks_exact(2).map(|b| u16::from_le_bytes([b[0], b[1]])).collect();
                Some(utf16(&wide)).filter(|text| !text.is_empty())
            };
            let mut index = 0;
            loop {
                let mut info = SP_DEVINFO_DATA {
                    cbSize: std::mem::size_of::<SP_DEVINFO_DATA>() as u32,
                    ..Default::default()
                };
                if SetupDiEnumDeviceInfo(set, index, &mut info).is_err() {
                    break;
                }
                index += 1;
                let Some(name) = property(&info, SPDRP_FRIENDLYNAME).or_else(|| property(&info, SPDRP_DEVICEDESC)) else {
                    continue;
                };
                // The driver's own key holds its version: "{class guid}\0001".
                let driver = property(&info, SPDRP_DRIVER)
                    .and_then(|key| registry_string(&format!(r"SYSTEM\CurrentControlSet\Control\Class\{key}"), "DriverVersion"));
                found.push((name, driver));
            }
            let _ = SetupDiDestroyDeviceInfoList(set);
        }
        found
    }

    pub fn devices() -> Vec<Hardware> {
        let mut all: Vec<Hardware> = Vec::new();
        for (name, driver) in class_devices(&DISPLAY) {
            // "Microsoft Basic Display Adapter", remote and virtual displays.
            let software = name.starts_with("Microsoft ") || name.contains("Virtual") || name.contains("Remote Display");
            if software || all.iter().any(|h| h.name == name) {
                continue;
            }
            all.push(hardware(name, DeviceKind::Gpu, driver));
        }
        for (name, driver) in class_devices(&COMPUTE_ACCELERATOR) {
            all.push(hardware(name, DeviceKind::Npu, driver));
        }
        all
    }
}

#[cfg(not(windows))]
mod imp {
    use crate::inference::Hardware;

    pub fn cpu_name() -> Option<String> {
        // Linux keeps it in /proc/cpuinfo; elsewhere there is no cheap source.
        let info = std::fs::read_to_string("/proc/cpuinfo").ok()?;
        info.lines()
            .find(|line| line.starts_with("model name"))
            .and_then(|line| line.split_once(':'))
            .map(|(_, name)| name.trim().to_string())
            .filter(|name| !name.is_empty())
    }

    pub fn devices() -> Vec<Hardware> {
        Vec::new() // the engine builds report the devices they can use
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Prints what this computer reports. Always passes on a machine with a CPU.
    #[test]
    fn the_cpu_is_named() {
        let cpu = cpu_name();
        eprintln!("cpu: {cpu}");
        assert!(!cpu.is_empty());
    }

    #[test]
    fn graphics_and_neural_devices_are_listed() {
        let found = devices();
        for device in &found {
            eprintln!("{:?} {:?}: {} (driver {:?})", device.kind, device.vendor, device.name, device.driver);
        }
        // Software adapters are left out, and nothing is listed twice.
        assert!(found.iter().all(|d| !d.name.starts_with("Microsoft ")));
        let mut names: Vec<&str> = found.iter().map(|d| d.name.as_str()).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), found.len());
    }

    #[test]
    fn neural_processors_are_intel_unless_named_otherwise() {
        assert_eq!(hardware("Intel(R) AI Boost".into(), DeviceKind::Npu, None).vendor, Vendor::Intel);
        assert_eq!(hardware("Neural Processing Unit".into(), DeviceKind::Npu, None).vendor, Vendor::Intel);
        assert_eq!(hardware("NVIDIA GeForce RTX 4050 Laptop GPU".into(), DeviceKind::Gpu, None).vendor, Vendor::Nvidia);
    }
}
