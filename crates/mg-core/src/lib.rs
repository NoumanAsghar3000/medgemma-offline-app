//! Shared logic used by BOTH the installer helper (mg-probe.exe) and the
//! desktop app, so the installer and the runtime can never disagree about
//! hardware detection or which model variant fits.
pub mod backend;
pub mod bench;
pub mod config;
pub mod gguf;
pub mod http;
pub mod plan;
pub mod hw;
pub mod select;

pub const GIB: u64 = 1024 * 1024 * 1024;
/// Hard rule: refuse to run/install below this much *installed* RAM.
#[cfg(not(feature = "relax-ram-for-tests"))]
pub const MIN_TOTAL_RAM: u64 = 4 * GIB;
/// TEST BUILDS ONLY (never enabled by CI release builds): lets small CI/dev boxes exercise the install path.
#[cfg(feature = "relax-ram-for-tests")]
pub const MIN_TOTAL_RAM: u64 = GIB;
/// Hard rule: always leave this much RAM free for Windows and other apps.
pub const HEADROOM: u64 = GIB;
