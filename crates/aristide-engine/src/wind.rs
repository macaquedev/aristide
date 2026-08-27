//! Wind supply model: per-windchest regulator with real dynamics.
//!
//! A pipe organ's blower feeds a spring/weight-regulated reservoir;
//! every speaking pipe bleeds it. What you *hear* of that system is not
//! a slow drift: opening a chord produces a **fast dip** (the pallets
//! gulp air before the regulator answers, ~50–80 ms), then the pressure
//! **bounces back with a slightly underdamped wobble** — the classic
//! bellows character — and settles a touch below nominal while the load
//! holds. Releasing the chord gives the mirror image with a small
//! overshoot above nominal.
//!
//! Model: a damped second-order regulator per wind group,
//!
//! ```text
//! target = 1 − sag_depth · min(D / D_ref, 3)
//! ẍ = ω²·(target − x) − 2ζω·ẋ            (x = pressure)
//! ```
//!
//! with `D` the summed wind weight of sounding voices, boosted for each
//! voice's first ~70 ms (pallet-opening transient — single notes dip
//! too, not just chords). Defaults: ω = 2π·3.5 Hz, ζ = 0.5 → dip
//! reached in ~70 ms, settled within ~250 ms with one audible bounce.
//! First-order-crawl behaviour (v1 of this file) is exactly what this
//! replaces: a 120 ms exponential glide reads as portamento, not wind.
//!
//! Pressure maps to per-voice factors as `P^pitch_exponent` on playback
//! rate and `P^gain_exponent` on gain. Depth defaults are deliberately
//! subtle: a full chorus settles ≈ −3 cents, transient dips reach
//! ≈ 1.3× that. Live rate modulation is only possible because voice
//! rate is a live value here (GrandOrgue freezes it at note start; see
//! docs/go-critique.md §2).
//!
//! Everything runs on the audio thread with fixed state — no
//! allocation; one integration step and two `powf` per group per block.

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
    /// Regulator natural frequency in Hz — how fast the system responds
    /// (and where the bellows bounce sits).
    pub natural_hz: f32,
    /// Damping ratio ζ: < 1 is underdamped (bouncy), 1 critical.
    pub damping: f32,
    /// Extra demand factor a voice adds right at its attack
    /// (pallet-opening gulp), decaying linearly over `attack_ms`.
    pub attack_boost: f32,
    pub attack_ms: f32,
    /// Pressure→pitch coupling: rate factor = P^this.
    pub pitch_exponent: f32,
    /// Pressure→volume coupling: gain factor = P^this.
    pub gain_exponent: f32,
    /// Pressure→harmonic-content coupling: upper-partial gain = P^this
    /// (Fletcher 1976: harmonic development rises with pressure; HW's
    /// "harmonic shaping" leg). Applied via each voice's tilt filter.
    pub brightness_exponent: f32,
    /// Per-pipe wind-flow noise: each voice's draw wanders within
    /// ±this fraction, slowly and independently (measured 1–5 %,
    /// docs/research/hauptwerk-wind-model.md §2). 0 disables.
    pub flow_noise: f32,
}

impl Default for WindParams {
    fn default() -> Self {
        WindParams {
            // Calibrated against measured reality (docs/research/):
            // chest pressure drops 1–10 % at full load are the realistic
            // range (Fraunhofer ISMA 2007; HW CODM recommends 1–10 %);
            // pipes detune ≈ 0.5–0.65 cents per 1 % pressure (Pykett's
            // measurement; HW's own 3.3-cents-at-6.3 % calibration).
            // 6 % × 0.032 → ≈ −3.4 cents steady at a full chorus, with
            // note-on/off transients dipping deeper — per Hauptwerk's
            // designer, the wobble at transitions IS the audible effect,
            // not the static sag.
            sag_depth: 0.06,
            reference_demand: 30.0,
            // Reservoir resonance: Fraunhofer measured 3–10 Hz bellows
            // modes; Walker's patent uses 2–5 Hz, ζ 0.4–0.7.
            natural_hz: 3.5,
            damping: 0.5,
            // Pallet gulp: onset dips measured 2–4× the sustained sag
            // (600→400 Pa dips vs −15 % sustained, ISMA 2007).
            attack_boost: 2.0,
            attack_ms: 50.0,
            // ≈ 0.55 cents per 1 % pressure (0.032 × 1200·log2 ≈ 0.55).
            pitch_exponent: 0.032,
            // Fletcher 1976: source power ∝ P^1.5 → ~15 dB/decade →
            // linear gain ∝ P^0.75.
            gain_exponent: 0.75,
            // ≈ −1.6 dB of upper partials at a −6 % chest drop; the
            // tremulant's ±22 % pressure swings it ±5 dB — the
            // "breathing timbre" a gain-only trem lacks.
            brightness_exponent: 3.0,
            flow_noise: 0.02,
        }
    }
}

/// A tremulant: periodic pressure modulation on one wind group.
///
/// Physically a tremulant is a valve venting the wind supply at a few
/// Hz; pitch, amplitude, and (later) brightness all move together
/// because they all follow pressure. Measured targets
/// (docs/research/organ-wind-acoustics.md §5): rate ~6 Hz, FM ±10–15
/// cents typical (±24 ceiling), AM ≥ 1 dB, and — characteristically —
/// cycle-to-cycle irregularity, modeled here as slow random walks on
/// rate and depth (Hauptwerk likewise randomizes both continuously).
#[derive(Debug, Clone, Copy)]
pub struct TremulantParams {
    pub rate_hz: f32,
    /// Peak pressure modulation as a fraction (0.22 ≈ ±12 cents via the
    /// default pitch exponent).
    pub depth: f32,
    /// Engage/disengage ramp, seconds (the valve spins up/down).
    pub ramp_seconds: f32,
    /// Slow random variation of rate and depth, as a fraction (0.08 =
    /// ±8 % wander).
    pub wobble: f32,
}

impl Default for TremulantParams {
    fn default() -> Self {
        TremulantParams {
            // Literature spans ~4–7 Hz (Pykett calls 6 "representative";
            // theatre trems run fast, classical church trems slower).
            // 5 Hz reads as a calm church tremulant; the sidecar knob
            // decides per instrument.
            rate_hz: 5.0,
            depth: 0.22,
            ramp_seconds: 0.7,
            wobble: 0.08,
        }
    }
}

/// Slow random-walk state: a value slewing toward a periodically
/// re-rolled target — a damped random process, not white noise.
/// Used chest-side (tremulant irregularity) and per-voice (flow noise).
#[derive(Debug, Clone, Copy)]
pub struct Wander {
    value: f32,
    target: f32,
    /// Seconds until the next target re-roll.
    countdown: f32,
}

impl Default for Wander {
    fn default() -> Self {
        Wander {
            value: 1.0,
            target: 1.0,
            countdown: 0.0,
        }
    }
}

impl Wander {
    /// Signed deviation from nominal, in (−spread, +spread).
    #[inline]
    pub fn deviation(&self) -> f32 {
        self.value - 1.0
    }

    /// Advance by `dt`, wandering within ±`spread` of 1.0.
    pub fn step(&mut self, dt: f32, spread: f32, rng: &mut u32) {
        self.countdown -= dt;
        if self.countdown <= 0.0 {
            // Re-roll every ~0.5–1.5 s.
            self.countdown = 0.5 + xorshift_unit(rng);
            self.target = 1.0 + spread * (2.0 * xorshift_unit(rng) - 1.0);
        }
        // Slew with ~0.5 s time constant.
        self.value += (self.target - self.value) * (dt * 2.0).min(1.0);
    }
}

/// The venting valve's pressure signature over one cycle — a
/// relaxation wave, not a sine. A tremulant beater blows open fast and
/// closes against the wind more slowly, and the reservoir rounds every
/// corner: pressure digs down quickly, climbs back gradually. A pure
/// sine here is exactly the Hammond-vibrato sound this engine must not
/// make. Built as a phase-skewed sine plus its second harmonic —
/// smooth everywhere (nothing to alias at valve rates), strongly
/// asymmetric, peaks ≈ ±1 with the *downward* lobe the sharper one.
#[inline]
pub(crate) fn valve_wave(phase: f32) -> f32 {
    use core::f32::consts::TAU;
    // Phase skew shapes the timing (fast fall, slow recovery); the
    // quadratic bend shapes the values (the vent digs deeper than it
    // crests — venting can dump pressure faster than the regulator can
    // overshoot it). Scaled so the dip lands near −1.
    let theta = TAU * phase - 0.75 * (TAU * phase).sin();
    let base = theta.sin() - 0.35 * (2.0 * theta).sin();
    0.7 * (base - 0.15 * base * base)
}

/// Subtracted from the valve wave before it scales pressure: a running
/// vent bleeds the reservoir, so engaging the tremulant also settles
/// the chest a touch below nominal — the slight flatten-and-soften a
/// listener hears when the stop goes on.
pub(crate) const VENT_BIAS: f32 = 0.18;

#[inline]
pub(crate) fn xorshift_unit(state: &mut u32) -> f32 {
    let mut x = *state;
    x ^= x << 13;
    x ^= x >> 17;
    x ^= x << 5;
    *state = x;
    (x >> 8) as f32 / (1u32 << 24) as f32
}

#[derive(Debug, Clone, Copy)]
pub struct WindGroup {
    params: WindParams,
    pressure: f32,
    velocity: f32,
    tremulant: TremulantParams,
    /// 0 = off, 1 = on; the engage envelope slews between them.
    tremulant_target: f32,
    tremulant_level: f32,
    tremulant_phase: f32,
    rate_wander: Wander,
    depth_wander: Wander,
    rng: u32,
    /// Cached per-block factors.
    rate_factor: f32,
    gain_factor: f32,
    brightness_factor: f32,
}

impl Default for WindGroup {
    fn default() -> Self {
        WindGroup {
            params: WindParams::default(),
            pressure: 1.0,
            velocity: 0.0,
            tremulant: TremulantParams::default(),
            tremulant_target: 0.0,
            tremulant_level: 0.0,
            tremulant_phase: 0.0,
            rate_wander: Wander::default(),
            depth_wander: Wander::default(),
            rng: 0x9E3779B9,
            rate_factor: 1.0,
            gain_factor: 1.0,
            brightness_factor: 1.0,
        }
    }
}

impl WindGroup {
    pub fn set_params(&mut self, params: WindParams) {
        self.params = params;
        if !self.enabled() {
            self.pressure = 1.0;
            self.velocity = 0.0;
            self.rate_factor = 1.0;
            self.gain_factor = 1.0;
            self.brightness_factor = 1.0;
        }
    }

    #[inline]
    fn enabled(&self) -> bool {
        self.params.sag_depth > 0.0
            && self.params.reference_demand > 0.0
            && self.params.natural_hz > 0.0
    }

    /// Give this group its place in the wind system, once at
    /// construction: a small fixed tremulant phase lag — pressure
    /// waves take milliseconds to reach a farther chest — while every
    /// group keeps the same random sequence, because ONE beater
    /// drives them all: their rates must wander together, never
    /// apart. (Fully independent wander was tried and beats the
    /// chests against each other at ~1 Hz — a slow seasick pump no
    /// organ makes.)
    pub fn decorrelate(&mut self, index: usize) {
        self.tremulant_phase = (index as f32 * 0.618_034).fract() * 0.06;
    }

    pub fn set_tremulant_params(&mut self, params: TremulantParams) {
        self.tremulant = params;
    }

    /// Engage/disengage the tremulant (ramped, safe while notes sound).
    pub fn set_tremulant(&mut self, engaged: bool) {
        self.tremulant_target = if engaged { 1.0 } else { 0.0 };
    }

    pub fn tremulant_engaged(&self) -> bool {
        self.tremulant_target > 0.5
    }

    /// Advance the regulator by `dt` seconds under demand `demand`
    /// (already attack-boosted by the caller), refreshing the factors.
    pub fn step(&mut self, demand: f32, dt: f32) {
        let tremulant_active = self.tremulant_level > 1e-4 || self.tremulant_target > 0.0;
        if !self.enabled() && !tremulant_active {
            return;
        }
        let p = &self.params;
        if self.enabled() {
            // Load response is linear in demand, capped so a freak tutti
            // can't fold the model in half.
            let target = 1.0 - p.sag_depth * (demand / p.reference_demand).min(3.0);
            let omega = core::f32::consts::TAU * p.natural_hz;

            // Semi-implicit Euler, substepped so blocks far larger than
            // the regulator period stay stable (ω·dt ≤ ~0.3 per substep).
            let steps = (dt * omega / 0.25).ceil().max(1.0) as u32;
            let h = dt / steps as f32;
            for _ in 0..steps {
                let accel = omega * omega * (target - self.pressure)
                    - 2.0 * p.damping * omega * self.velocity;
                self.velocity += h * accel;
                self.pressure = (self.pressure + h * self.velocity).clamp(0.5, 1.2);
            }
        }

        // Tremulant: pressure modulation on top of the regulator state.
        // The wave is the valve's relaxation cycle (see `valve_wave`),
        // sitting slightly below nominal on average (`VENT_BIAS` — a
        // running vent bleeds the reservoir), with rate and depth
        // wandering slowly the way a real beater never quite repeats.
        let mut effective = self.pressure;
        if tremulant_active {
            let t = &self.tremulant;
            let ramp = (dt / t.ramp_seconds.max(0.01)).min(1.0);
            self.tremulant_level += (self.tremulant_target - self.tremulant_level) * ramp;
            self.rate_wander.step(dt, t.wobble, &mut self.rng);
            self.depth_wander.step(dt, t.wobble, &mut self.rng);
            self.tremulant_phase =
                (self.tremulant_phase + dt * t.rate_hz * self.rate_wander.value).fract();
            let modulation = t.depth
                * self.depth_wander.value
                * self.tremulant_level
                * (valve_wave(self.tremulant_phase) - VENT_BIAS);
            effective = (effective * (1.0 + modulation)).clamp(0.3, 1.5);
        }
        self.rate_factor = effective.powf(p.pitch_exponent);
        self.gain_factor = effective.powf(p.gain_exponent);
        self.brightness_factor = effective.powf(p.brightness_exponent);
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

    #[inline]
    pub fn brightness_factor(&self) -> f32 {
        self.brightness_factor
    }

    #[inline]
    pub fn params(&self) -> &WindParams {
        &self.params
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(group: &mut WindGroup, demand: f32, seconds: f32, dt: f32) -> (f32, f32) {
        let mut min_p = f32::MAX;
        let mut max_p = f32::MIN;
        let steps = (seconds / dt) as usize;
        for _ in 0..steps {
            group.step(demand, dt);
            min_p = min_p.min(group.pressure());
            max_p = max_p.max(group.pressure());
        }
        (min_p, max_p)
    }

    #[test]
    fn fast_dip_then_settles_at_calibrated_sag() {
        let mut group = WindGroup::default();
        let p = *group.params();
        let dt = 0.005;

        // Within the first 120 ms the dip must already be near (or past)
        // the steady-state target — this is what "fast" means.
        let (early_min, _) = run(&mut group, p.reference_demand, 0.12, dt);
        let steady_target = 1.0 - p.sag_depth;
        assert!(
            early_min < steady_target + 0.001,
            "dip too slow: min {early_min} vs target {steady_target} within 120 ms"
        );

        // Underdamped: the transient undershoots below the steady value.
        // Theory at ζ=0.5: overshoot = exp(−πζ/√(1−ζ²)) ≈ 16% of the step.
        let (transient_min, _) = run(&mut group, p.reference_demand, 0.5, dt);
        assert!(
            transient_min < steady_target - 0.1 * p.sag_depth,
            "no bounce: min {transient_min} vs steady {steady_target}"
        );

        // And settles at the calibrated steady state.
        run(&mut group, p.reference_demand, 1.5, dt);
        assert!(
            (group.pressure() - steady_target).abs() < 0.0005,
            "steady {} vs {steady_target}",
            group.pressure()
        );
    }

    #[test]
    fn release_recovers_quickly_with_slight_overshoot() {
        let mut group = WindGroup::default();
        let p = *group.params();
        let dt = 0.005;
        run(&mut group, p.reference_demand, 2.0, dt);

        // Recovery: back within 10% of nominal inside 300 ms.
        run(&mut group, 0.0, 0.3, dt);
        assert!(
            group.pressure() > 1.0 - 0.1 * p.sag_depth,
            "slow recovery: {}",
            group.pressure()
        );
        // Underdamped release passes slightly above nominal.
        let (_, max_p) = run(&mut group, 0.0, 1.0, dt);
        assert!(
            max_p > 1.0 && max_p < 1.0 + p.sag_depth,
            "overshoot {max_p} out of character"
        );
    }

    #[test]
    fn double_demand_sags_roughly_double() {
        let mut group = WindGroup::default();
        let p = *group.params();
        run(&mut group, 2.0 * p.reference_demand, 3.0, 0.005);
        let sag = 1.0 - group.pressure();
        assert!(
            sag > 1.8 * p.sag_depth && sag < 2.2 * p.sag_depth,
            "sag {sag} not ~2x {}",
            p.sag_depth
        );
    }

    #[test]
    fn tremulant_modulates_at_rate_and_disengages() {
        let mut group = WindGroup::default();
        let trem = TremulantParams {
            wobble: 0.0,  // deterministic for the rate check
            rate_hz: 6.0, // pinned: the assertion counts cycles
            ..TremulantParams::default()
        };
        group.set_tremulant_params(trem);
        group.set_tremulant(true);

        let dt = 0.002;
        // Let the engage ramp finish.
        for _ in 0..((3.0 / dt) as usize) {
            group.step(0.0, dt);
        }
        // Track the rate factor over 2 s: depth and rate must match.
        let mut min_f = f32::MAX;
        let mut max_f = f32::MIN;
        let mut crossings = 0;
        let mut previous = group.rate_factor() - 1.0;
        for _ in 0..((2.0 / dt) as usize) {
            group.step(0.0, dt);
            let value = group.rate_factor() - 1.0;
            min_f = min_f.min(group.rate_factor());
            max_f = max_f.max(group.rate_factor());
            if previous < 0.0 && value >= 0.0 {
                crossings += 1;
            }
            previous = value;
        }
        // ±22 % pressure through P^0.032 → ≈ ±0.64 % rate (±11 cents).
        // The valve wave digs deeper than it overshoots and rides a
        // vented (negative) mean, so the downward excursion carries
        // most of the depth.
        let expected = (1.0f32 + trem.depth).powf(group.params().pitch_exponent) - 1.0;
        assert!(
            1.0 - min_f > 0.8 * expected,
            "dip too shallow: {min_f} vs expected −{expected}"
        );
        assert!(
            max_f - 1.0 > 0.3 * expected,
            "no upward swing at all: {max_f} vs expected +{expected}"
        );
        assert!(
            (11..=13).contains(&crossings),
            "rate: {crossings} cycles in 2 s, expected ~12 at 6 Hz"
        );

        // Disengage: modulation ramps out.
        group.set_tremulant(false);
        for _ in 0..((3.0 / dt) as usize) {
            group.step(0.0, dt);
        }
        let mut spread = 0.0f32;
        for _ in 0..((1.0 / dt) as usize) {
            group.step(0.0, dt);
            spread = spread.max((group.rate_factor() - 1.0).abs());
        }
        assert!(spread < 0.0005, "tremulant should have died out: {spread}");
    }

    /// The valve wave is a relaxation cycle, not a sine: strongly
    /// asymmetric (it spends unequal time above and below its mean),
    /// smooth, roughly ±1 in peak, and — via `VENT_BIAS` — its applied
    /// form averages below zero, settling a running chest flat-and-soft
    /// of nominal. A pure sine fails every one of these.
    #[test]
    fn the_valve_wave_is_no_sine() {
        let n = 4096;
        let samples: Vec<f32> = (0..n).map(|i| valve_wave(i as f32 / n as f32)).collect();
        let peak_up = samples.iter().cloned().fold(f32::MIN, f32::max);
        let peak_down = samples.iter().cloned().fold(f32::MAX, f32::min);
        assert!(
            (0.5..=0.9).contains(&peak_up) && (-1.1..=-0.8).contains(&peak_down),
            "peaks out of calibration: {peak_down}..{peak_up}"
        );
        assert!(
            peak_down.abs() > peak_up,
            "the vent must dig deeper than it overshoots: {peak_down} vs {peak_up}"
        );
        // Slope asymmetry: the vent dumps pressure faster than the
        // reservoir recovers it, so the steepest fall clearly outruns
        // the steepest rise. A sine's are equal.
        let mut steepest_fall = 0.0f32;
        let mut steepest_rise = 0.0f32;
        for i in 0..n {
            let slope = samples[(i + 1) % n] - samples[i];
            steepest_rise = steepest_rise.max(slope);
            steepest_fall = steepest_fall.max(-slope);
        }
        assert!(
            steepest_fall > 1.3 * steepest_rise,
            "fall {steepest_fall} not clearly faster than rise {steepest_rise} — \
             this is a sine again"
        );
        // Harmonic content: correlation with the best-phase sine stays
        // clearly under 1 (a sine of any phase correlates ~1.0).
        let mut sin_dot = 0.0f64;
        let mut cos_dot = 0.0f64;
        let mut energy = 0.0f64;
        for (i, v) in samples.iter().enumerate() {
            let phi = core::f32::consts::TAU * i as f32 / n as f32;
            sin_dot += (*v * phi.sin()) as f64;
            cos_dot += (*v * phi.cos()) as f64;
            energy += (*v * *v) as f64;
        }
        let fundamental = (sin_dot * sin_dot + cos_dot * cos_dot).sqrt() / (n as f64 / 2.0);
        let rms = (energy / n as f64).sqrt();
        let purity = fundamental / (rms * std::f64::consts::SQRT_2);
        assert!(
            purity < 0.97,
            "waveform is {purity:.3} sine-pure — the Hammond is back"
        );
        // And the mean pressure under the engaged tremulant sits below
        // nominal: the vent bleeds the reservoir.
        let wave_mean = samples.iter().sum::<f32>() / n as f32;
        assert!(
            wave_mean - VENT_BIAS < -0.1,
            "applied wave must average below zero: {}",
            wave_mean - VENT_BIAS
        );
    }

    /// Two groups never tick in lock-step once decorrelated: different
    /// seeds, different starting phases.
    #[test]
    fn decorrelated_groups_drift_apart() {
        let mut a = WindGroup::default();
        let mut b = WindGroup::default();
        a.decorrelate(0);
        b.decorrelate(1);
        a.set_tremulant(true);
        b.set_tremulant(true);
        let dt = 0.005;
        let mut differed = false;
        for _ in 0..1000 {
            a.step(0.0, dt);
            b.step(0.0, dt);
            if (a.rate_factor() - b.rate_factor()).abs() > 1e-4 {
                differed = true;
            }
        }
        assert!(differed, "groups 0 and 1 ran identically");
    }

    #[test]
    fn tremulant_works_even_with_sag_disabled() {
        let mut group = WindGroup::default();
        group.set_params(WindParams {
            sag_depth: 0.0,
            ..WindParams::default()
        });
        group.set_tremulant(true);
        for _ in 0..1000 {
            group.step(0.0, 0.005);
        }
        assert!(
            (group.rate_factor() - 1.0).abs() > 1e-4 || {
                // could be near a zero crossing; scan a cycle
                let mut hit = false;
                for _ in 0..100 {
                    group.step(0.0, 0.005);
                    if (group.rate_factor() - 1.0).abs() > 1e-3 {
                        hit = true;
                        break;
                    }
                }
                hit
            },
            "tremulant must run on a sag-disabled chest"
        );
    }

    #[test]
    fn zero_sag_disables_everything() {
        let mut group = WindGroup::default();
        group.set_params(WindParams {
            sag_depth: 0.0,
            ..WindParams::default()
        });
        run(&mut group, 1000.0, 1.0, 0.01);
        assert_eq!(group.pressure(), 1.0);
        assert_eq!(group.rate_factor(), 1.0);
        assert_eq!(group.gain_factor(), 1.0);
    }

    #[test]
    fn stable_at_audio_block_rates() {
        // 4096-frame blocks at 44.1 kHz = 93 ms steps: far beyond the
        // regulator period; the substepping must keep it stable.
        let mut group = WindGroup::default();
        let p = *group.params();
        let (min_p, max_p) = run(&mut group, p.reference_demand, 5.0, 0.093);
        assert!(min_p > 0.9 && max_p <= 1.2, "unstable: {min_p}..{max_p}");
        assert!((group.pressure() - (1.0 - p.sag_depth)).abs() < 0.002);
    }
}
