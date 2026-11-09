//! Real hardware detection (no fakes).
//!  * RAM: GetPhysicallyInstalledSystemMemory (installed) + GlobalMemoryStatusEx (free).
//!  * GPU: a *hardware* DXGI adapter (software/"Basic Render" adapters are rejected)
//!    with >= 2 GiB dedicated VRAM AND a loadable vulkan-1.dll (the GPU backend
//!    we ship is llama.cpp's Vulkan build, which runs on NVIDIA, AMD and Intel Arc).
use crate::GIB;
use serde::Serialize;

#[derive(Debug, Clone, Serialize, Default)]
pub struct Hardware {
    pub total_ram: u64, // bytes physically installed
    pub free_ram: u64,  // bytes currently available
    pub gpu_ok: bool,
    pub gpu_name: String,
    pub vram: u64, // dedicated VRAM bytes of the best adapter
    pub vulkan_loader: bool,
}

pub const MIN_VRAM: u64 = 2 * GIB;

#[cfg(windows)]
pub fn detect() -> Hardware {
    use windows::core::w;
    use windows::Win32::Graphics::Dxgi::*;
    use windows::Win32::System::LibraryLoader::{FreeLibrary, LoadLibraryW};
    use windows::Win32::System::SystemInformation::*;

    let mut hw = Hardware::default();

    unsafe {
        let mut ms = MEMORYSTATUSEX {
            dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32,
            ..Default::default()
        };
        if GlobalMemoryStatusEx(&mut ms).is_ok() {
            hw.free_ram = ms.ullAvailPhys;
            hw.total_ram = ms.ullTotalPhys;
        }
        // Installed RAM (includes memory reserved by firmware, so a "4 GB" stick
        // is not reported as 3.7 GiB). Falls back to ullTotalPhys on failure.
        let mut kb: u64 = 0;
        if GetPhysicallyInstalledSystemMemory(&mut kb).is_ok() && kb > 0 {
            hw.total_ram = kb * 1024;
        }

        if let Ok(h) = LoadLibraryW(w!("vulkan-1.dll")) {
            hw.vulkan_loader = true;
            let _ = FreeLibrary(h);
        }

        if let Ok(factory) = CreateDXGIFactory1::<IDXGIFactory1>() {
            let mut i = 0;
            while let Ok(adapter) = factory.EnumAdapters1(i) {
                i += 1;
                if let Ok(d) = adapter.GetDesc1() {
                    let software = (d.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32) != 0
                        || d.VendorId == 0x1414; // Microsoft Basic Render Driver
                    if software {
                        continue;
                    }
                    let vram = d.DedicatedVideoMemory as u64;
                    if vram > hw.vram {
                        hw.vram = vram;
                        let n = d.Description.iter().take_while(|c| **c != 0).copied().collect::<Vec<u16>>();
                        hw.gpu_name = String::from_utf16_lossy(&n);
                    }
                }
            }
        }
    }
    hw.gpu_ok = hw.vulkan_loader && hw.vram >= MIN_VRAM;
    hw
}

/// Non-Windows stub so the selection logic can be unit-tested on Linux CI/dev boxes.
#[cfg(not(windows))]
pub fn detect() -> Hardware {
    let mut hw = Hardware::default();
    if let Ok(s) = std::fs::read_to_string("/proc/meminfo") {
        for l in s.lines() {
            let kb = |l: &str| l.split_whitespace().nth(1).and_then(|v| v.parse::<u64>().ok()).unwrap_or(0) * 1024;
            if l.starts_with("MemTotal:") { hw.total_ram = kb(l); }
            if l.starts_with("MemAvailable:") { hw.free_ram = kb(l); }
        }
    }
    hw
}
