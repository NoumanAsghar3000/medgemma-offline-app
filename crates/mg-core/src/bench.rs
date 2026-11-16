//! Real bandwidth measurements that feed the tier planner (std only, so they work everywhere).
use crate::plan::Bandwidth;
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::Path,
    time::Instant,
};

/// Multi-threaded memory copy throughput (GB/s of data read). Inference is multi-core and bandwidth-bound.
pub fn measure_ram_gbps() -> f64 {
    const CHUNK: usize = 48 * 1024 * 1024;
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).clamp(2, 8);
    let t0 = Instant::now();
    let hs: Vec<_> = (0..threads).map(|_| std::thread::spawn(|| {
        let src = vec![1u8; CHUNK]; let mut dst = vec![0u8; CHUNK]; let mut acc = 0u8;
        for _ in 0..3 { dst.copy_from_slice(&src); acc = acc.wrapping_add(dst[CHUNK / 2]); }
        acc
    })).collect();
    for h in hs { let _ = h.join(); }
    let secs = t0.elapsed().as_secs_f64().max(1e-6);
    ((threads * CHUNK * 3) as f64 / secs) / 1e9
}

/// Sequential read speed (MB/s) of the drive that holds `path`: reads up to `bytes` from the MIDDLE of the file
/// (the start is more likely to be in the OS cache). A cached region inflates the result, so the caller
/// should measure a file that has not just been read; the planner treats the number as a best case.
pub fn measure_disk_mbps(path: &Path, bytes: u64) -> Option<f64> {
    let mut f = File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    if len < 8 * 1024 * 1024 { return None; }
    let want = bytes.min(len / 2);
    f.seek(SeekFrom::Start(len / 2 - want / 2)).ok()?;
    let mut buf = vec![0u8; 4 * 1024 * 1024]; let mut done = 0u64; let t0 = Instant::now();
    while done < want { let n = f.read(&mut buf).ok()?; if n == 0 { break; } done += n as u64; }
    let secs = t0.elapsed().as_secs_f64().max(1e-6);
    if done == 0 { None } else { Some((done as f64 / 1e6) / secs) }
}

pub fn measure(model_file: Option<&Path>) -> Bandwidth {
    let c = Bandwidth::conservative();
    Bandwidth {
        ram_gbps: measure_ram_gbps().max(1.0),
        disk_mbps: model_file.and_then(|p| measure_disk_mbps(p, 192 * 1024 * 1024)).unwrap_or(c.disk_mbps),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn ram_measurement_is_sane() { let g = measure_ram_gbps(); assert!(g > 0.5 && g < 2000.0, "{g}"); }
    #[test] fn disk_measurement_reads_the_middle_of_a_file() {
        let p = std::env::temp_dir().join("mg_bench_file.bin");
        std::fs::write(&p, vec![7u8; 64 * 1024 * 1024]).unwrap();
        let m = measure_disk_mbps(&p, 32 * 1024 * 1024).unwrap(); assert!(m > 1.0, "{m}");
        assert!(measure_disk_mbps(Path::new("/nonexistent/x.gguf"), 1 << 20).is_none());
    }
}
