//! The memory budget: start a session only when the machine can afford it.
//!
//! Instead of letting the OS pick victims when memory runs out, hangar
//! admits a new session only if, after paying its expected cost, at least
//! `reserve` bytes of physical memory would still be free. Otherwise the
//! session waits in a queue and starts when memory frees up. The expected
//! cost is learned from sessions hangar has already run.

use crate::config::BudgetConfig;
use crate::procs;
use serde::Serialize;
use std::collections::VecDeque;

const SAMPLES: usize = 20;

#[derive(Debug, Clone)]
pub struct Budget {
    pub reserve_bytes: u64,
    pub max_sessions: usize,
    initial_estimate: u64,
    /// Peak footprints of recent sessions.
    samples: VecDeque<u64>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Machine {
    pub total_bytes: u64,
    pub available_bytes: u64,
    pub reserve_bytes: u64,
    pub estimate_bytes: u64,
}

impl Budget {
    pub fn new(cfg: &BudgetConfig) -> Budget {
        let (total, _) = procs::system_memory();
        let auto = (2u64 << 30).max(total / 10);
        Budget {
            reserve_bytes: if cfg.reserve_mb > 0 { cfg.reserve_mb << 20 } else { auto },
            max_sessions: cfg.max_sessions,
            initial_estimate: cfg.initial_estimate_mb.max(1) << 20,
            samples: VecDeque::new(),
        }
    }

    /// Expected memory cost of one more session: the median recent peak.
    pub fn estimate(&self) -> u64 {
        if self.samples.is_empty() {
            return self.initial_estimate;
        }
        let mut v: Vec<u64> = self.samples.iter().copied().collect();
        v.sort_unstable();
        v[v.len() / 2]
    }

    pub fn record_peak(&mut self, bytes: u64) {
        if bytes == 0 {
            return;
        }
        if self.samples.len() >= SAMPLES {
            self.samples.pop_front();
        }
        self.samples.push_back(bytes);
    }

    pub fn machine(&self) -> Machine {
        let (total, available) = procs::system_memory();
        Machine { total_bytes: total, available_bytes: available, reserve_bytes: self.reserve_bytes, estimate_bytes: self.estimate() }
    }

    /// `Ok` if one more session may start now, else why not.
    pub fn admit(&self, running: usize, available: u64) -> Result<(), String> {
        if self.max_sessions > 0 && running >= self.max_sessions {
            return Err(format!("{running} sessions running (limit {})", self.max_sessions));
        }
        let need = self.estimate() + self.reserve_bytes;
        if available < need {
            return Err(format!(
                "waiting for memory: needs about {} free (session ~{} + reserve {}), {} available",
                crate::util::fmt_bytes(need),
                crate::util::fmt_bytes(self.estimate()),
                crate::util::fmt_bytes(self.reserve_bytes),
                crate::util::fmt_bytes(available)
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn budget(reserve_mb: u64, max: usize) -> Budget {
        Budget::new(&BudgetConfig { reserve_mb, max_sessions: max, initial_estimate_mb: 500, ..Default::default() })
    }

    #[test]
    fn admits_only_when_reserve_survives() {
        let b = budget(1000, 0);
        let mb = |n: u64| n << 20;
        assert!(b.admit(0, mb(1600)).is_ok());
        let err = b.admit(0, mb(1400)).unwrap_err();
        assert!(err.contains("waiting for memory"), "{err}");
    }

    #[test]
    fn respects_the_session_cap() {
        let b = budget(1, 2);
        assert!(b.admit(1, u64::MAX).is_ok());
        assert!(b.admit(2, u64::MAX).unwrap_err().contains("limit 2"));
    }

    #[test]
    fn learns_the_median_session_cost() {
        let mut b = budget(1, 0);
        assert_eq!(b.estimate(), 500 << 20);
        for mb in [300u64, 900, 400] {
            b.record_peak(mb << 20);
        }
        assert_eq!(b.estimate(), 400 << 20);
        b.record_peak(0);
        assert_eq!(b.samples.len(), 3, "zero samples ignored");
    }

    #[test]
    fn automatic_reserve_is_at_least_two_gigabytes() {
        assert!(budget(0, 0).reserve_bytes >= 2 << 30);
    }
}
