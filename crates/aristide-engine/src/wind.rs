//! Wind supply model: per-windchest reservoir pressure.
//!
//! A pipe organ's blower feeds a spring- or weight-regulated reservoir;
//! every speaking pipe bleeds it. Under a big chord the pressure sags —
//! pitch droops and volume gives a little — then the reservoir recovers.
//! That breathing is the single most characteristic dynamic behaviour
//! of the instrument, and samples alone cannot reproduce it: it couples
//! *simultaneously sounding* pipes on the same chest.
//!
//! Model: normalized first-order reservoir per wind group,
//!
//! ```text
//! dP/dt = (1 − P)/τ  −  s·D·P
//! ```
//!
//! where `D` is the summed wind weight of sounding voices and `s` is
//! calibrated so that the configured `reference_demand` produces the
//! configured `sag_depth` at steady state: `P∞ = 1/(1 + s·τ·D)`.
//! Attack dips and recovery overshoot-free settling fall out of the
//! dynamics; no special cases.
//!
//! Pressure maps to per-voice factors as `P^pitch_exponent` on playback
//! rate and `P^gain_exponent` on gain — flue pipes flatten and soften
//! together as pressure drops. This is only possible because voice rate
//! is a live value here (GrandOrgue freezes rate at note start; see
//! docs/go-critique.md §2).
//!
//! Everything runs on the audio thread with fixed state — no
//! allocation, one integration step and two `powf` per group per block.

/// Wind groups the engine tracks; ODF windchests past this share the
/// last group. Sixteen covers every real organ definition seen so far.
pub const MAX_WIND_GROUPS: usize = 16;

#[derive(Debug, Clone, Copy)]
pub struct WindParams {
    /// Steady-state pressure loss (0..1) at `reference_demand`.
    /// 0 disables the model for the group entirely.
    pub sag_depth: f32,
    /// Total voice wind-weight that produces `sag_depth`.
    pub reference_demand: f32,
    /// Reservoir recovery time constant, in seconds.
    pub recovery_seconds: f32,
    /// Pressure→pitch coupling: rate factor = P^this.
    pub pitch_exponent: f32,
    /// Pressure→volume coupling: gain factor = P^this.
    pub gain_exponent: f32,
}

impl Default for WindParams {
    fn default() -> Self {
        WindParams {
            sag_depth: 0.02,
            reference_demand: 30.0,
            recovery_seconds: 0.12,
            pitch_exponent: 0.4,
            gain_exponent: 0.8,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct WindGroup {
    params: WindParams,
    /// Calibrated draw coefficient (`s` above).
    draw: f32,
    pressure: f32,
    /// Cached per-block factors.
    rate_factor: f32,
    gain_factor: f32,
}

impl Default for WindGroup {
    fn default() -> Self {
        let mut group = WindGroup {
            params: WindParams::default(),
            draw: 0.0,
            pressure: 1.0,
            rate_factor: 1.0,
            gain_factor: 1.0,
        };
        group.set_params(group.params);
        group
    }
}

impl WindGroup {
    pub fn set_params(&mut self, params: WindParams) {
        self.params = params;
        let WindParams {
            sag_depth,
            reference_demand,
            recovery_seconds,
            ..
        } = params;
        self.draw = if sag_depth > 0.0 && reference_demand > 0.0 && recovery_seconds > 0.0 {
            sag_depth / ((1.0 - sag_depth) * recovery_seconds * reference_demand)
        } else {
            0.0
        };
        if self.draw == 0.0 {
            self.pressure = 1.0;
            self.rate_factor = 1.0;
            self.gain_factor = 1.0;
        }
    }

    /// Advance the reservoir by `dt` seconds under demand `demand`,
    /// refreshing the cached factors.
    pub fn step(&mut self, demand: f32, dt: f32) {
        if self.draw == 0.0 {
            return;
        }
        let recovery = (1.0 - self.pressure) / self.params.recovery_seconds;
        let consumption = self.draw * demand * self.pressure;
        self.pressure = (self.pressure + dt * (recovery - consumption)).clamp(0.2, 1.2);
        self.rate_factor = self.pressure.powf(self.params.pitch_exponent);
        self.gain_factor = self.pressure.powf(self.params.gain_exponent);
    }

    #[inline]
    pub fn pressure(&self) -> f32 {
        self.pressure
    }

    #[inline]
    pub fn rate_factor(&self) -> f32 {
        self.rate_factor
    }

    #[inline]
    pub fn gain_factor(&self) -> f32 {
        self.gain_factor
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sags_to_calibrated_steady_state_and_recovers() {
        let mut group = WindGroup::default();
        let params = group.params;
        // Run at exactly the reference demand for many time constants.
        let dt = 0.01;
        for _ in 0..500 {
            group.step(params.reference_demand, dt);
        }
        let expected = 1.0 - params.sag_depth;
        assert!(
            (group.pressure() - expected).abs() < 0.002,
            "steady pressure {} vs expected {expected}",
            group.pressure()
        );
        assert!(group.rate_factor() < 1.0 && group.gain_factor() < 1.0);

        // Silence: recovers to nominal within a few time constants.
        for _ in 0..100 {
            group.step(0.0, dt);
        }
        assert!(group.pressure() > 0.999, "recovered {}", group.pressure());
    }

    #[test]
    fn double_demand_sags_roughly_double() {
        let mut group = WindGroup::default();
        let params = group.params;
        let dt = 0.005;
        for _ in 0..1000 {
            group.step(2.0 * params.reference_demand, dt);
        }
        let sag = 1.0 - group.pressure();
        assert!(
            sag > 1.5 * params.sag_depth && sag < 2.5 * params.sag_depth,
            "sag {sag} not ~2x {}",
            params.sag_depth
        );
    }

    #[test]
    fn zero_sag_disables_everything() {
        let mut group = WindGroup::default();
        group.set_params(WindParams {
            sag_depth: 0.0,
            ..WindParams::default()
        });
        for _ in 0..100 {
            group.step(1000.0, 0.01);
        }
        assert_eq!(group.pressure(), 1.0);
        assert_eq!(group.rate_factor(), 1.0);
        assert_eq!(group.gain_factor(), 1.0);
    }
}
