//! Tier planner (the part of Colibri's philosophy that survives for a DENSE model).
//!
//! Colibri keeps a mixture-of-experts model on disk and streams only the few experts a token routes to.
//! MedGemma is dense: EVERY weight is read for EVERY token, so a model that does not fit in fast memory
//! must be streamed in full on each token and speed collapses to (disk bandwidth / model size).
//! This module therefore does not pretend to skip weights; it (1) places them across VRAM -> RAM -> disk,
//! (2) estimates the resulting speed from MEASURED bandwidth, and (3) lets the selector refuse variants
//! that would be unusably slow. The estimate is bandwidth-bound arithmetic, not a benchmark: the app
//! also shows the real tokens/second reported by llama-server after every answer.
use crate::GIB;
use serde::Serialize;

#[derive(Debug, Clone, Copy, Serialize)]
pub struct Bandwidth {
    /// Multi-threaded system-RAM read bandwidth, GB/s.
    pub ram_gbps: f64,
    /// Sequential read speed of the drive holding the model (HDD/SSD/USB), MB/s.
    pub disk_mbps: f64,
}
impl Bandwidth {
    /// Used only when measurement is impossible; deliberately pessimistic.
    pub fn conservative() -> Self { Bandwidth { ram_gbps: 15.0, disk_mbps: 150.0 } }
}

/// Minimum estimated decode speed (tokens/s) for a variant to be chosen automatically.
pub const MIN_TPS: f64 = 0.5;
const VRAM_GBPS: f64 = 200.0; // conservative bandwidth of an 8 GB-class GPU

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Verdict {
    /// >= 2 tok/s : comfortable chat
    Interactive,
    /// 0.5 - 2 tok/s : usable, you wait for long answers
    Slow,
    /// 0.05 - 0.5 tok/s : a 1,000-token report takes roughly 5 - 60 minutes
    BatchOnly,
    /// < 0.05 tok/s
    Impractical,
}
impl Verdict {
    pub fn from_tps(t: f64) -> Verdict {
        if t >= 2.0 { Verdict::Interactive } else if t >= MIN_TPS { Verdict::Slow } else if t >= 0.05 { Verdict::BatchOnly } else { Verdict::Impractical }
    }
    pub fn label(self) -> &'static str {
        match self { Verdict::Interactive => "interactive", Verdict::Slow => "slow", Verdict::BatchOnly => "batch only (very slow)", Verdict::Impractical => "impractical" }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Input {
    pub model_bytes: u64,
    pub mmproj_bytes: u64,
    /// KV cache + compute buffers for this context size (host share when running on CPU).
    pub overhead: u64,
    /// RAM we may use = free RAM - 1 GiB headroom.
    pub ram_budget: u64,
    /// Dedicated VRAM of the GPU (0 = CPU run).
    pub vram: u64,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct Placement {
    /// Fraction of the weights placed in VRAM (0..1).
    pub gpu_fraction: f64,
    pub gpu_bytes: u64,
    /// Weights that stay resident in RAM.
    pub ram_bytes: u64,
    /// Weights that must be re-read from the drive for every token (best case: proportional cache hits).
    pub streamed_bytes: u64,
    pub tps_best: f64,
    /// Worst case: strict-LRU page cache thrash when the CPU-side weights exceed RAM (nothing stays cached).
    pub tps_worst: f64,
    pub verdict: Verdict,
}

pub fn estimate(i: &Input, bw: &Bandwidth) -> Placement {
    let w = i.model_bytes as f64;
    let total = (i.model_bytes + i.mmproj_bytes) as f64;
    let (f, host_overhead) = if i.vram > 0 {
        let usable = (i.vram as f64 * 0.85) - (i.overhead as f64 / 2.0);
        ((usable.max(0.0) / total).min(1.0), i.overhead as f64 / 2.0 + GIB as f64 / 2.0)
    } else { (0.0, i.overhead as f64) };
    let gpu_w = f * w;
    let host_w = w - gpu_w;
    let host_mm = if i.vram > 0 { 0.0 } else { i.mmproj_bytes as f64 };
    let ram_for_weights = (i.ram_budget as f64 - host_overhead - host_mm).max(0.0);
    let resident = host_w.min(ram_for_weights);
    let streamed = host_w - resident;

    let ram_bps = bw.ram_gbps * 1e9; let disk_bps = (bw.disk_mbps * 1e6).max(1.0);
    let t_best = gpu_w / (VRAM_GBPS * 1e9) + resident / ram_bps + streamed / disk_bps;
    let t_worst = gpu_w / (VRAM_GBPS * 1e9) + if host_w <= ram_for_weights { host_w / ram_bps } else { host_w / disk_bps };
    let tps_best = if t_best > 0.0 { 1.0 / t_best } else { f64::INFINITY };
    let tps_worst = if t_worst > 0.0 { 1.0 / t_worst } else { f64::INFINITY };
    Placement {
        gpu_fraction: f, gpu_bytes: gpu_w as u64, ram_bytes: resident as u64, streamed_bytes: streamed as u64,
        tps_best, tps_worst, verdict: Verdict::from_tps(tps_best),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const NVME: Bandwidth = Bandwidth { ram_gbps: 40.0, disk_mbps: 3000.0 };
    const USB: Bandwidth = Bandwidth { ram_gbps: 40.0, disk_mbps: 200.0 };
    fn q8_27b(budget_gb: f64, vram_gb: f64) -> Input {
        Input { model_bytes: (28.7 * GIB as f64) as u64, mmproj_bytes: (0.9 * GIB as f64) as u64, overhead: GIB * 3 / 2,
                ram_budget: (budget_gb * GIB as f64) as u64, vram: (vram_gb * GIB as f64) as u64 }
    }
    #[test] fn resident_27b_q8_on_big_ram_is_usable() {
        let p = estimate(&q8_27b(40.0, 0.0), &NVME);
        assert_eq!(p.streamed_bytes, 0); assert!(p.tps_best > 0.9 && p.tps_best < 2.0, "{}", p.tps_best); assert_eq!(p.verdict, Verdict::Slow);
    }
    #[test] fn the_requested_8gb_ram_8gb_vram_target_is_not_fast_for_a_dense_27b_q8() {
        // 7 GB usable RAM + 8 GB VRAM: ~21% in VRAM, ~6 GB resident, the rest re-read from disk EVERY token
        let p = estimate(&q8_27b(7.0, 8.0), &NVME);
        assert!(p.streamed_bytes > 15 * GIB && p.gpu_fraction > 0.15 && p.gpu_fraction < 0.3);
        assert!(p.tps_best < 0.5 && p.tps_best > 0.1, "NVMe: {}", p.tps_best); assert_eq!(p.verdict, Verdict::BatchOnly);
        let u = estimate(&q8_27b(7.0, 8.0), &USB);
        assert_eq!(u.verdict, Verdict::Impractical, "USB: {}", u.tps_best);
    }
    #[test] fn worst_case_is_never_better_than_best_case() {
        for (r, v) in [(7.0, 0.0), (20.0, 0.0), (25.0, 8.0), (40.0, 12.0)] {
            let p = estimate(&q8_27b(r, v), &NVME); assert!(p.tps_worst <= p.tps_best + 1e-9, "{r} {v}");
        }
    }
    #[test] fn a_4b_q8_fits_the_8gb_target_and_is_interactive() {
        let i = Input { model_bytes: (4.1 * GIB as f64) as u64, mmproj_bytes: (0.85 * GIB as f64) as u64, overhead: GIB * 3 / 4, ram_budget: 7 * GIB, vram: 0 };
        let p = estimate(&i, &NVME); assert_eq!(p.streamed_bytes, 0); assert_eq!(p.verdict, Verdict::Interactive, "{}", p.tps_best);
    }
}
