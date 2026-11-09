//! Shared logic used by BOTH the installer helper (mg-probe.exe) and the
//! desktop app, so the installer and the runtime can never disagree about
//! hardware detection or which model variant fits.
pub mod backend;
pub mod config;
pub mod hw;
pub mod select;

pub const GIB: u64 = 1024 * 1024 * 1024;
/// Hard rule: refuse to run/install below this much *installed* RAM.
pub const MIN_TOTAL_RAM: u64 = 4 * GIB;
/// Hard rule: always leave this much RAM free for Windows and other apps.
pub const HEADROOM: u64 = GIB;
