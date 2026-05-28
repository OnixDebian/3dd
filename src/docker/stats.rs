//! PURE stats normalizer — raw Docker counters in, a normalized
//! [`StatSample`] out. Independent of bollard: 03-03 will map bollard's
//! `ContainerStatsResponse` onto [`RawCpu`] / [`RawMem`] before calling
//! [`normalize`], so this code stays daemon-free and unit-testable on
//! synthetic samples.
//!
//! See PITFALLS Pitfall 1 — the CPU% delta is the highest-correctness risk in
//! the whole phase, and every guard documented there will be implemented (and
//! pinned by the tests) here in the GREEN pass.

// Surface consumed by 03-03/03-04 and Phase 4 — public before its first call
// site, so dead_code while only the tests use it.
#![allow(dead_code)]

/// Raw CPU counters from one stats sample.
///
/// All counters are cumulative nanoseconds from the daemon (NEVER a
/// percentage). Two samples are required to compute a percentage — see
/// [`normalize`].
#[derive(Debug, Clone, Copy, Default)]
pub struct RawCpu {
    /// Cumulative container CPU usage (`cpu_stats.cpu_usage.total_usage`).
    pub total_usage: u64,
    /// Cumulative host system CPU usage (`cpu_stats.system_cpu_usage`).
    /// `None` on Windows daemons / certain cgroup configurations.
    pub system_usage: Option<u64>,
    /// Online CPU count reported by the daemon
    /// (`cpu_stats.online_cpus`). `None` or 0 on some daemons — falls back
    /// to `percpu_len` in [`normalize`].
    pub online_cpus: Option<u64>,
    /// Length of `cpu_stats.cpu_usage.percpu_usage` — the per-core slice,
    /// used as a fallback for `online_cpus` when it is missing/zero.
    pub percpu_len: usize,
}

/// Raw memory counters from one stats sample.
#[derive(Debug, Clone, Copy, Default)]
pub struct RawMem {
    /// Total memory usage from the daemon (`memory_stats.usage`). Includes
    /// page cache — see [`RawMem::cache`].
    pub usage: u64,
    /// Cache to subtract from `usage` to get the working-set: cgroup v2
    /// `memory_stats.stats["inactive_file"]`, cgroup v1
    /// `memory_stats.stats["cache"]`. 03-03 picks the right one per daemon.
    pub cache: u64,
    /// Memory limit (`memory_stats.limit`). 0 when unlimited / unset.
    pub limit: u64,
}

/// One normalized stats sample — the domain stat type the rest of the app
/// speaks (NOT bollard's type). All fields are finite and bounded once
/// [`normalize`] is implemented in the GREEN pass.
#[derive(Debug, Clone, Copy, Default)]
pub struct StatSample {
    /// CPU percentage in `[0, online_cpus * 100]` (so a 4-core 100%-busy
    /// container reads as `400.0`). `0.0` on the warming-up sample and on
    /// every guarded zero-divisor case — NEVER NaN/Inf.
    pub cpu_pct: f32,
    /// Working-set memory in bytes (`usage - cache`, clamped to
    /// `[0, mem_limit]`).
    pub mem_used: u64,
    /// Memory limit in bytes (mirrors [`RawMem::limit`], passed through so
    /// downstream code does not need both structs).
    pub mem_limit: u64,
    /// Working-set memory as a fraction of the limit, in `[0, 1]`. `0.0`
    /// when limit is 0 — NEVER NaN/Inf.
    pub mem_fraction: f32,
    /// Normalized load that drives box size: the MAX of the CPU% normalized
    /// to `[0, 1]` (divided by `online_cpus * 100`) and `mem_fraction`. So
    /// a box grows for whichever resource it is hot on. Feeds the existing
    /// [`crate::world::entity::load_to_half_extent`] unchanged. `0.0` on
    /// the warming-up sample.
    pub load: f32,
    /// First sample of a stream (no previous CPU counters) — the box is
    /// NOT sized from this sample; `load` is forced to `0.0`. Per PITFALLS
    /// Pitfall 1: the first sample is garbage and must be skipped.
    pub warming_up: bool,
}

/// Normalize one stats sample.
///
/// `prev_cpu` is the previous sample's CPU counters (i.e. bollard's
/// `precpu_stats`). When it is `None` — or its counters are all zero — the
/// sample is marked [`StatSample::warming_up`] and `load` is `0.0`: the box
/// is not sized from it (PITFALLS Pitfall 1).
///
/// Formula (the CPU%-delta gotcha):
/// ```text
/// cpu_delta    = cur.total_usage - prev.total_usage
/// system_delta = cur.system_usage - prev.system_usage
/// online_cpus  = cur.online_cpus, fallback to cur.percpu_len if missing/0
/// cpu_pct      = (cpu_delta / system_delta) * online_cpus * 100
/// ```
///
/// Guards (all enforced):
/// - First sample → `warming_up=true`, `load=0.0`.
/// - `system_delta <= 0` → `cpu_pct = 0.0`.
/// - `online_cpus == 0` → `cpu_pct = 0.0`.
/// - Final `cpu_pct` clamped to `[0, online_cpus * 100]`.
/// - Any non-finite intermediate coerced to `0.0` before returning.
///
/// Memory: `mem_used = usage.saturating_sub(cache)`, clamped to `limit`;
/// `mem_fraction = mem_used / limit` in `[0,1]` (or `0.0` when `limit == 0`).
pub fn normalize(cur_cpu: &RawCpu, prev_cpu: Option<&RawCpu>, mem: &RawMem) -> StatSample {
    // --- Memory: pure, no division-by-zero risk on `saturating_sub`. ----------
    let mem_used_raw = mem.usage.saturating_sub(mem.cache);
    let mem_used = if mem.limit > 0 {
        mem_used_raw.min(mem.limit)
    } else {
        mem_used_raw
    };
    let mem_fraction: f32 = if mem.limit > 0 {
        (mem_used as f64 / mem.limit as f64).clamp(0.0, 1.0) as f32
    } else {
        0.0
    };

    // --- online_cpus with the percpu_len fallback ------------------------------
    let online_cpus: u64 = match cur_cpu.online_cpus {
        Some(n) if n > 0 => n,
        _ => cur_cpu.percpu_len as u64,
    };

    // --- First-sample / warming-up detection -----------------------------------
    // Treat as warming up when there is no prior sample at all, or when the
    // prior sample's counters look uninitialized (both zero — the precpu_stats
    // shape Docker emits on the very first frame of a stream).
    let warming_up = match prev_cpu {
        None => true,
        Some(p) => p.total_usage == 0 && p.system_usage.unwrap_or(0) == 0,
    };

    // --- CPU%: only meaningful with a real previous sample AND a real online ---
    let cpu_pct: f32 = if warming_up || online_cpus == 0 {
        0.0
    } else {
        // `prev_cpu` is Some here (warming_up handled None).
        let prev = prev_cpu.expect("prev_cpu is Some when not warming up");
        let cur_sys = cur_cpu.system_usage.unwrap_or(0);
        let prev_sys = prev.system_usage.unwrap_or(0);

        // Use f64 for the delta math: counters are u64 and large.
        let cpu_delta = cur_cpu.total_usage as f64 - prev.total_usage as f64;
        let system_delta = cur_sys as f64 - prev_sys as f64;

        if system_delta <= 0.0 || cpu_delta < 0.0 {
            // Zero/negative system_delta -> never divide.
            // Negative cpu_delta (counter regression) -> treat as idle.
            0.0
        } else {
            let raw = (cpu_delta / system_delta) * online_cpus as f64 * 100.0;
            let ceiling = online_cpus as f64 * 100.0;
            let clamped = raw.clamp(0.0, ceiling);
            if clamped.is_finite() {
                clamped as f32
            } else {
                0.0
            }
        }
    };

    // --- Load: max of cpu_norm and mem_fraction, in [0,1] ----------------------
    let load: f32 = if warming_up {
        0.0
    } else {
        let cpu_norm: f32 = if online_cpus > 0 {
            (cpu_pct / (online_cpus as f32 * 100.0)).clamp(0.0, 1.0)
        } else {
            0.0
        };
        cpu_norm.max(mem_fraction).clamp(0.0, 1.0)
    };

    // --- Final scrub: coerce any non-finite slip to 0.0 ------------------------
    let cpu_pct = if cpu_pct.is_finite() { cpu_pct } else { 0.0 };
    let mem_fraction = if mem_fraction.is_finite() {
        mem_fraction
    } else {
        0.0
    };
    let load = if load.is_finite() { load } else { 0.0 };

    StatSample {
        cpu_pct,
        mem_used,
        mem_limit: mem.limit,
        mem_fraction,
        load,
        warming_up,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::entity::{load_to_half_extent, MAX_HALF, MIN_HALF};

    /// Helper: a "prior" CPU sample with non-zero counters (so the next sample
    /// is NOT flagged warming-up).
    fn prev(total: u64, sys: u64) -> RawCpu {
        RawCpu {
            total_usage: total,
            system_usage: Some(sys),
            online_cpus: Some(4),
            percpu_len: 4,
        }
    }

    #[test]
    fn busy_container_matches_formula() {
        // cpu_delta=100, system_delta=1000, online_cpus=4
        //   => (100/1000) * 4 * 100 = 40.0
        let p = prev(1_000, 10_000);
        let c = RawCpu {
            total_usage: 1_100,
            system_usage: Some(11_000),
            online_cpus: Some(4),
            percpu_len: 4,
        };
        let m = RawMem {
            usage: 0,
            cache: 0,
            limit: 1,
        };
        let s = normalize(&c, Some(&p), &m);
        assert!((s.cpu_pct - 40.0).abs() < 1e-3, "cpu_pct = {}", s.cpu_pct);
        assert!(!s.warming_up);
        assert!(s.cpu_pct.is_finite());
    }

    #[test]
    fn first_sample_is_warming_up() {
        let c = RawCpu {
            total_usage: 5_000,
            system_usage: Some(50_000),
            online_cpus: Some(4),
            percpu_len: 4,
        };
        let m = RawMem {
            usage: 500,
            cache: 100,
            limit: 1_000,
        };
        let s = normalize(&c, None, &m);
        assert!(s.warming_up);
        assert_eq!(s.load, 0.0);
        assert!(s.cpu_pct.is_finite() && s.load.is_finite());
    }

    #[test]
    fn zero_precpu_counters_count_as_warming_up() {
        // bollard hands us precpu_stats with all zeros on the first frame.
        let p = RawCpu {
            total_usage: 0,
            system_usage: Some(0),
            online_cpus: Some(4),
            percpu_len: 4,
        };
        let c = RawCpu {
            total_usage: 5_000,
            system_usage: Some(50_000),
            online_cpus: Some(4),
            percpu_len: 4,
        };
        let m = RawMem::default();
        let s = normalize(&c, Some(&p), &m);
        assert!(s.warming_up);
        assert_eq!(s.load, 0.0);
    }

    #[test]
    fn zero_system_delta_yields_zero() {
        // cur.system == prev.system -> system_delta == 0 -> never divide.
        let p = prev(1_000, 10_000);
        let c = RawCpu {
            total_usage: 2_000,
            system_usage: Some(10_000),
            online_cpus: Some(4),
            percpu_len: 4,
        };
        let m = RawMem::default();
        let s = normalize(&c, Some(&p), &m);
        assert_eq!(s.cpu_pct, 0.0);
        assert!(s.cpu_pct.is_finite());
    }

    #[test]
    fn none_online_cpus_falls_back_to_percpu_len() {
        // online_cpus = None, percpu_len = 2 -> ceiling = 200, formula uses 2.
        let p = RawCpu {
            total_usage: 1_000,
            system_usage: Some(10_000),
            online_cpus: None,
            percpu_len: 2,
        };
        let c = RawCpu {
            total_usage: 1_100,
            system_usage: Some(11_000),
            online_cpus: None,
            percpu_len: 2,
        };
        let m = RawMem::default();
        let s = normalize(&c, Some(&p), &m);
        // (100/1000) * 2 * 100 = 20.0
        assert!((s.cpu_pct - 20.0).abs() < 1e-3, "cpu_pct = {}", s.cpu_pct);
    }

    #[test]
    fn zero_online_cpus_yields_zero() {
        let p = RawCpu {
            total_usage: 1_000,
            system_usage: Some(10_000),
            online_cpus: Some(0),
            percpu_len: 0,
        };
        let c = RawCpu {
            total_usage: 1_100,
            system_usage: Some(11_000),
            online_cpus: Some(0),
            percpu_len: 0,
        };
        let m = RawMem::default();
        let s = normalize(&c, Some(&p), &m);
        assert_eq!(s.cpu_pct, 0.0);
        assert!(s.cpu_pct.is_finite());
    }

    #[test]
    fn cpu_pct_clamped_to_ceiling() {
        // cpu_delta huge so raw > online*100 -> clamps to online*100.
        let p = prev(0, 1_000);
        let c = RawCpu {
            total_usage: 1_000_000_000,
            system_usage: Some(1_100), // tiny system_delta => raw blows up
            online_cpus: Some(4),
            percpu_len: 4,
        };
        let m = RawMem::default();
        let s = normalize(&c, Some(&p), &m);
        assert!(s.cpu_pct <= 400.0 + 1e-3);
        assert!(s.cpu_pct.is_finite());
        // And it should saturate at the ceiling, not just stay tiny.
        assert!((s.cpu_pct - 400.0).abs() < 1e-3, "cpu_pct = {}", s.cpu_pct);
    }

    #[test]
    fn memory_subtracts_cache() {
        let p = prev(1_000, 10_000);
        let c = RawCpu {
            total_usage: 1_000,
            system_usage: Some(10_000),
            online_cpus: Some(4),
            percpu_len: 4,
        };
        let m = RawMem {
            usage: 1_000,
            cache: 400,
            limit: 2_000,
        };
        let s = normalize(&c, Some(&p), &m);
        assert_eq!(s.mem_used, 600);
        assert!(
            (s.mem_fraction - 0.3).abs() < 1e-6,
            "frac = {}",
            s.mem_fraction
        );
    }

    #[test]
    fn memory_clamps_to_limit() {
        // usage > limit after cache -> mem_used <= limit, fraction <= 1.0.
        let p = prev(1_000, 10_000);
        let c = RawCpu {
            total_usage: 1_000,
            system_usage: Some(10_000),
            online_cpus: Some(4),
            percpu_len: 4,
        };
        let m = RawMem {
            usage: 5_000,
            cache: 0,
            limit: 2_000,
        };
        let s = normalize(&c, Some(&p), &m);
        assert!(s.mem_used <= m.limit, "mem_used = {}", s.mem_used);
        assert!(s.mem_fraction <= 1.0 + 1e-6, "frac = {}", s.mem_fraction);
        assert!((s.mem_fraction - 1.0).abs() < 1e-6);
    }

    #[test]
    fn zero_limit_yields_zero_fraction() {
        let p = prev(1_000, 10_000);
        let c = RawCpu {
            total_usage: 1_000,
            system_usage: Some(10_000),
            online_cpus: Some(4),
            percpu_len: 4,
        };
        let m = RawMem {
            usage: 1_000,
            cache: 0,
            limit: 0,
        };
        let s = normalize(&c, Some(&p), &m);
        assert_eq!(s.mem_fraction, 0.0);
        assert_eq!(s.mem_limit, 0);
        assert!(s.mem_fraction.is_finite());
    }

    #[test]
    fn load_is_max_of_cpu_and_mem() {
        // CPU dominates: cpu_norm = 40/400 = 0.1, mem_fraction = 0.05 -> load = 0.1
        let p = prev(1_000, 10_000);
        let c = RawCpu {
            total_usage: 1_100,
            system_usage: Some(11_000),
            online_cpus: Some(4),
            percpu_len: 4,
        };
        let m = RawMem {
            usage: 50,
            cache: 0,
            limit: 1_000,
        };
        let s = normalize(&c, Some(&p), &m);
        assert!((s.load - 0.1).abs() < 1e-3, "load = {}", s.load);

        // Memory dominates: cpu_norm = 0.1, mem_fraction = 0.8 -> load = 0.8
        let m2 = RawMem {
            usage: 800,
            cache: 0,
            limit: 1_000,
        };
        let s2 = normalize(&c, Some(&p), &m2);
        assert!((s2.load - 0.8).abs() < 1e-3, "load = {}", s2.load);
    }

    #[test]
    fn load_feeds_existing_size_map() {
        // load is in [0,1] and feeds load_to_half_extent unchanged.
        let p = prev(1_000, 10_000);
        let c = RawCpu {
            total_usage: 1_100,
            system_usage: Some(11_000),
            online_cpus: Some(4),
            percpu_len: 4,
        };
        let m = RawMem {
            usage: 800,
            cache: 100,
            limit: 1_000,
        };
        let s = normalize(&c, Some(&p), &m);
        let h = load_to_half_extent(s.load);
        assert!(h.is_finite());
        assert!(
            (MIN_HALF..=MAX_HALF).contains(&h),
            "h={h} out of [{MIN_HALF},{MAX_HALF}]"
        );
    }

    #[test]
    fn never_nan_inf() {
        // Adversarial inputs: u64::MAX counters, equal cur/prev, zero limit,
        // missing online_cpus, zero percpu_len.
        let cases: Vec<(RawCpu, Option<RawCpu>, RawMem)> = vec![
            // u64::MAX counters
            (
                RawCpu {
                    total_usage: u64::MAX,
                    system_usage: Some(u64::MAX),
                    online_cpus: Some(4),
                    percpu_len: 4,
                },
                Some(RawCpu {
                    total_usage: 0,
                    system_usage: Some(0),
                    online_cpus: Some(4),
                    percpu_len: 4,
                }),
                RawMem {
                    usage: u64::MAX,
                    cache: 0,
                    limit: 1,
                },
            ),
            // equal cur/prev (system_delta == 0)
            (
                prev(1_000, 10_000),
                Some(prev(1_000, 10_000)),
                RawMem {
                    usage: 0,
                    cache: 0,
                    limit: 0,
                },
            ),
            // None online_cpus + zero percpu_len + None prev
            (
                RawCpu {
                    total_usage: 0,
                    system_usage: None,
                    online_cpus: None,
                    percpu_len: 0,
                },
                None,
                RawMem::default(),
            ),
            // cache > usage (saturating_sub -> 0)
            (
                prev(2_000, 20_000),
                Some(prev(1_000, 10_000)),
                RawMem {
                    usage: 100,
                    cache: 9_999,
                    limit: 1_000,
                },
            ),
            // counter regression (cur < prev)
            (
                RawCpu {
                    total_usage: 100,
                    system_usage: Some(100),
                    online_cpus: Some(4),
                    percpu_len: 4,
                },
                Some(prev(1_000, 10_000)),
                RawMem {
                    usage: 0,
                    cache: 0,
                    limit: 1,
                },
            ),
        ];
        for (cur, p, m) in cases {
            let s = normalize(&cur, p.as_ref(), &m);
            assert!(s.cpu_pct.is_finite(), "cpu_pct = {}", s.cpu_pct);
            assert!(
                s.mem_fraction.is_finite(),
                "mem_fraction = {}",
                s.mem_fraction
            );
            assert!(s.load.is_finite(), "load = {}", s.load);
            assert!(
                (0.0..=1.0).contains(&s.load),
                "load out of [0,1] = {}",
                s.load
            );
            assert!(s.cpu_pct >= 0.0, "negative cpu_pct = {}", s.cpu_pct);
            assert!(
                s.mem_fraction >= 0.0,
                "negative mem_fraction = {}",
                s.mem_fraction
            );
        }
    }
}
