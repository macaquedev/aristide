//! Swell box / enclosure model: per-box shutter state driving a gain
//! and a high-shelf filter applied per enclosed voice.
//!
//! Grounded in docs/research/enclosure-modeling.md:
//! - A closed box is a **low-pass filter, not a volume knob**: measured
//!   10–20 dB broadband (Braasch 2008), rising to ~35 dB at 8 kHz with
//!   near-zero attenuation at 30 Hz (Pykett 2023). Model: a broadband
//!   floor (the set's `AmpMinimumLevel`) plus a high-shelf leg whose
//!   corner slides down as the box closes (HW's model; GO issue #717's
//!   requested design).
//! - **dB-linear taper** in shutter position (HW's law): the raw
//!   physics is front-loaded near closed, but real installations
//!   compensate with staged/exponential pedal schedules — modeling
//!   physics-then-compensation would be two curves canceling. GO's
//!   linear-amplitude taper is the floor, not the target.
//! - **Shutter inertia**: shutters are heavy wooden panels driven
//!   through a linkage; the pedal cannot teleport them. A critically
//!   damped second-order slew (HW models accel+damping coefficients)
//!   both sounds right and kills control zipper.
//! - **Closed-box pressure rise**: the box is not only an acoustic
//!   filter, it is the volume the enclosed pipes exhaust *into*. With
//!   the shutters shut their outflow pressurizes it, and a pipe speaks
//!   on the difference between its chest and its mouth — so the rise
//!   is a small pressure loss for every pipe in the box (HW's
//!   `WindModel_BoxPressureRisePctAtMaxLoadWhenClosed`, 1–5 %: "a very
//!   slight, but just discernible detuning when the box is fully
//!   closed"). See [`Enclosure::step`] for the derivation.
//!
//! The per-voice filter leg lives in the engine's voice loop; this
//! module owns the box state and its per-block factors. Everything
//! runs on the audio thread with fixed state — no allocation; one
//! `powf`+`exp` pair per box per block.

use aristide_model::units::db_to_linear;

/// Enclosures the engine tracks; matches `MAX_WIND_GROUPS` in spirit —
/// real organs rarely exceed a handful of boxes.
pub const MAX_ENCLOSURES: usize = 16;

/// Voice marker for "not enclosed".
pub const ENCLOSURE_NONE: u8 = u8::MAX;

/// Enclosure memberships one voice can carry. Real instruments do nest
/// boxes — an Echo or Solo box standing inside the Swell is standard
/// English and American practice, and GrandOrgue lets a windchest join
/// any number of them — but two deep is the realistic maximum, and a
/// fixed pair keeps the voice's state small and its render loop
/// branch-free for the overwhelmingly common single-box case.
pub const MAX_VOICE_ENCLOSURES: usize = 2;

/// Flow conductance of the shutter front wide open, as a multiple of
/// the closed box's residual leakage (gaps around the shutters, the
/// grille, the joinery). A swell front is square metres of opening
/// against a few hundred square centimetres of residual gap, so two
/// orders of magnitude is the conservative end. It is the ONE number
/// that decides how fast cracking the shutters vents the box, and it
/// is why HW's parameter is named "…WhenClosed": at 10 % open the
/// steady rise is already down to ~9 % of its closed value.
const SHUTTER_VENT_RATIO: f32 = 100.0;

#[derive(Debug, Clone, Copy)]
pub struct EnclosureParams {
    /// Broadband attenuation with the box fully closed, dB (negative).
    /// From the ODF's `AmpMinimumLevel` unless the sidecar overrides:
    /// 20 % → −14 dB, squarely inside Braasch's measured 10–20 dB.
    pub floor_db: f32,
    /// Extra high-shelf attenuation (above the corner) fully closed,
    /// dB (negative). HW's worked example and Pykett's high-frequency
    /// excess both sit near −10 dB.
    pub shelf_db: f32,
    /// Shelf corner with the box fully open, Hz. HW CODM starting
    /// value: 8 kHz (the shelf all but vanishes).
    pub corner_open_hz: f32,
    /// Shelf corner fully closed, Hz. HW CODM starting value: 1 kHz.
    pub corner_closed_hz: f32,
    /// Exponent on closedness for both dB laws: 1 = dB-linear (HW).
    /// `>1` concentrates change near closed (raw-physics lean), `<1`
    /// the opposite.
    pub taper: f32,
    /// Full-sweep settle time of the shutter inertia model, seconds.
    /// ≤0 disables (pedal drives the shutters directly).
    pub full_sweep_s: f32,
    /// Static-pressure rise inside the box at full enclosed load with
    /// the shutters shut, as a percentage of chest pressure — HW's
    /// `WindModel_BoxPressureRisePctAtMaxLoadWhenClosed` (1–5 %
    /// suggested; above that the box robs its own wind). 0 disables
    /// the leg entirely.
    pub pressure_rise_pct: f32,
    /// Enclosed wind draw that counts as "full load", in the same
    /// units and on the same scale as a chest's
    /// [`reference_demand`](crate::wind::WindParams::reference_demand)
    /// — a box usually holds exactly one division, so one full chorus
    /// on the box is one full chorus on its chest.
    pub reference_demand: f32,
    /// Fill/leak time constant of the CLOSED box, seconds; the open
    /// box vents `SHUTTER_VENT_RATIO` times faster. Derived below.
    pub fill_seconds: f32,
}

impl Default for EnclosureParams {
    fn default() -> Self {
        EnclosureParams {
            floor_db: -14.0,
            shelf_db: -10.0,
            corner_open_hz: 8_000.0,
            corner_closed_hz: 1_000.0,
            taper: 1.0,
            full_sweep_s: 0.5,
            // HW's suggested band is 1–5 %; its midpoint is the
            // "just discernible when fully closed" the docs describe.
            pressure_rise_pct: 2.0,
            reference_demand: 30.0,
            fill_seconds: 0.25,
        }
    }
}

/// One swell box: smoothed shutter position plus the per-block factors
/// enclosed voices read.
#[derive(Debug, Clone, Copy)]
pub struct Enclosure {
    params: EnclosureParams,
    /// Pedal demand, 0 = closed .. 1 = open.
    target: f32,
    /// Shutter position after inertia.
    position: f32,
    velocity: f32,
    /// Overpressure inside the box as a fraction of chest pressure.
    pressure_rise: f32,
    /// Cached per-block factors.
    gain: f32,
    hi_gain: f32,
    coeff: f32,
}

impl Default for Enclosure {
    fn default() -> Self {
        Enclosure {
            params: EnclosureParams::default(),
            // GO's default enclosure value is 127 = fully open.
            target: 1.0,
            position: 1.0,
            velocity: 0.0,
            pressure_rise: 0.0,
            gain: 1.0,
            hi_gain: 1.0,
            coeff: 0.0,
        }
    }
}

impl Enclosure {
    pub fn set_params(&mut self, params: EnclosureParams) {
        self.params = params;
    }

    /// Pedal input, 0 = closed .. 1 = open (control side clamps too).
    pub fn set_target(&mut self, position: f32) {
        if position.is_finite() {
            self.target = position.clamp(0.0, 1.0);
        }
    }

    /// Advance the shutter model by `dt` seconds under the enclosed
    /// wind draw `demand`, and refresh the factors.
    ///
    /// **The pressure leg, from first principles.** The box is a
    /// semi-sealed volume `V` the enclosed pipes exhaust into. Mass
    /// balance over it, linearized about the static pressure, is
    ///
    /// ```text
    /// (V/ρc²)·dδp/dt = Q_in − C(k)·δp
    /// ```
    ///
    /// with `Q_in` the volume flow the sounding enclosed pipes push in
    /// (exactly the wind draw the chest model already aggregates) and
    /// `C(k)` the flow conductance out — the shutter gaps at opening
    /// `k` plus the box's own leakage. That is a first-order lag whose
    /// steady state and time constant are set by the SAME conductance:
    ///
    /// ```text
    /// δp∞ = Q_in / C(k)        τ = V / (ρc²·C(k))
    /// ```
    ///
    /// So the closed box builds slowly to its full rise, and cracking
    /// the shutters both collapses the rise and dumps it fast — which
    /// is why HW's parameter is calibrated "when closed" only.
    ///
    /// Magnitudes, to check the calibration is physical: a 30 m³ box,
    /// a division pushing ~0.1 m³/s, and HW's 2 % of an 800 Pa chest
    /// (16 Pa) imply a residual leak area of ~190 cm² (16 Pa drives a
    /// ~5 m/s jet through it) — a plausible swell front. The same
    /// numbers give `τ = 2V·δp∞/(ρc²·Q_in) ≈ 0.07 s`, and across
    /// small-box/light-registration to big-chamber/full-organ the band
    /// is roughly 0.03–0.5 s. Air fills a box fast; the audible
    /// smoothing comes from the per-voice speech lags downstream, not
    /// from here. `fill_seconds` defaults to 0.25 s — mid-band and on
    /// the slow side, so the detuning swells in rather than snapping.
    pub fn step(&mut self, demand: f32, dt: f32, sample_rate: f32) {
        let p = self.params;
        if p.full_sweep_s > 1e-3 {
            // Critically damped second-order slew: settles to ~2 % in
            // ≈ 5.8/ω, so ω = 6/full_sweep_s makes `full_sweep_s` mean
            // what it says. Substepped like the wind regulator so huge
            // blocks stay stable (ω·h ≤ ~0.25).
            let omega = 6.0 / p.full_sweep_s;
            let steps = (dt * omega / 0.25).ceil().max(1.0) as u32;
            let h = dt / steps as f32;
            for _ in 0..steps {
                let accel =
                    omega * omega * (self.target - self.position) - 2.0 * omega * self.velocity;
                self.velocity += h * accel;
                self.position = (self.position + h * self.velocity).clamp(0.0, 1.0);
            }
        } else {
            self.position = self.target;
            self.velocity = 0.0;
        }

        // The pressure leg (derivation above). `vent` is C(k)/C_closed:
        // the shutter front wide open passes SHUTTER_VENT_RATIO times
        // what the closed box leaks, and both the steady rise and the
        // time constant scale as its reciprocal.
        if p.pressure_rise_pct > 0.0 && p.reference_demand > 0.0 {
            let vent = 1.0 + (SHUTTER_VENT_RATIO - 1.0) * self.position;
            // Load capped like the chest regulator's, so a freak tutti
            // on one box cannot drive the model somewhere silly.
            let load = (demand / p.reference_demand).min(3.0);
            let target = 0.01 * p.pressure_rise_pct * load / vent;
            let tau = p.fill_seconds.max(1e-3) / vent;
            // Exact one-pole: stable at any block size, and a wide-open
            // box (τ ≈ 2.5 ms) simply tracks, as it should.
            let alpha = 1.0 - (-dt / tau).exp();
            self.pressure_rise += (target - self.pressure_rise) * alpha;
        } else {
            self.pressure_rise = 0.0;
        }

        let closed = (1.0 - self.position).max(0.0).powf(p.taper.max(0.05));
        self.gain = db_to_linear((p.floor_db * closed) as f64) as f32;
        self.hi_gain = db_to_linear((p.shelf_db * closed) as f64) as f32;
        // Corner slides geometrically (log-frequency is the perceptual
        // axis, and slit-transmission cutoff scales ~1/opening, which
        // geometric interpolation tracks far better than HW's linear
        // Hz). Clamped to Nyquist headroom.
        let open = p.corner_open_hz.max(20.0);
        let ratio = (p.corner_closed_hz.max(20.0) / open).max(1e-3);
        let corner = (open * ratio.powf(closed)).min(0.45 * sample_rate);
        self.coeff = 1.0 - (-core::f32::consts::TAU * corner / sample_rate).exp();
    }

    #[inline]
    pub fn gain(&self) -> f32 {
        self.gain
    }

    #[inline]
    pub fn hi_gain(&self) -> f32 {
        self.hi_gain
    }

    #[inline]
    pub fn coeff(&self) -> f32 {
        self.coeff
    }

    #[inline]
    pub fn position(&self) -> f32 {
        self.position
    }

    /// Overpressure inside the box, as a fraction of static chest
    /// pressure. A pipe speaks on the difference between its chest and
    /// its mouth, so for every voice in this box this is a pressure
    /// *loss* of the same size.
    #[inline]
    pub fn pressure_loss(&self) -> f32 {
        self.pressure_rise
    }

    #[inline]
    pub fn params(&self) -> &EnclosureParams {
        &self.params
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    fn settle(enclosure: &mut Enclosure, seconds: f32, sr: f32) {
        let dt = 0.005;
        for _ in 0..((seconds / dt) as usize) {
            enclosure.step(0.0, dt, sr);
        }
    }

    #[test]
    fn closed_box_hits_floor_and_shelf() {
        let mut e = Enclosure::default();
        e.set_target(0.0);
        settle(&mut e, 3.0, 44_100.0);
        let p = *e.params();
        assert!((e.position() - 0.0).abs() < 0.01, "position {}", e.position());
        assert!(
            (20.0 * e.gain().log10() - p.floor_db).abs() < 0.2,
            "gain {} dB vs floor {}",
            20.0 * e.gain().log10(),
            p.floor_db
        );
        assert!(
            (20.0 * e.hi_gain().log10() - p.shelf_db).abs() < 0.2,
            "shelf {} dB",
            20.0 * e.hi_gain().log10()
        );
    }

    #[test]
    fn open_box_is_transparent() {
        let mut e = Enclosure::default();
        e.set_target(1.0);
        settle(&mut e, 2.0, 44_100.0);
        assert!((e.gain() - 1.0).abs() < 1e-3);
        assert!((e.hi_gain() - 1.0).abs() < 1e-3);
    }

    #[test]
    fn inertia_makes_the_sweep_take_time() {
        let mut e = Enclosure::default();
        e.set_target(1.0);
        settle(&mut e, 2.0, 44_100.0);
        e.set_target(0.0);
        // After a tenth of the sweep time the shutters have barely moved;
        // after 2x they have settled.
        settle(&mut e, 0.05, 44_100.0);
        assert!(e.position() > 0.8, "moved too fast: {}", e.position());
        settle(&mut e, 1.0, 44_100.0);
        assert!(e.position() < 0.02, "did not settle: {}", e.position());
    }

    #[test]
    fn no_inertia_tracks_instantly() {
        let mut e = Enclosure::default();
        e.set_params(EnclosureParams {
            full_sweep_s: 0.0,
            ..EnclosureParams::default()
        });
        e.set_target(0.25);
        e.step(0.0, 0.005, 44_100.0);
        assert_eq!(e.position(), 0.25);
    }

    #[test]
    fn corner_slides_down_as_the_box_closes() {
        let sr = 44_100.0;
        let mut open = Enclosure::default();
        open.set_target(1.0);
        settle(&mut open, 2.0, sr);
        let mut closed = Enclosure::default();
        closed.set_target(0.0);
        settle(&mut closed, 3.0, sr);
        // Higher corner = larger one-pole coefficient.
        assert!(
            open.coeff() > 2.0 * closed.coeff(),
            "open coeff {} vs closed {}",
            open.coeff(),
            closed.coeff()
        );
    }

    #[test]
    fn taper_is_db_linear_by_default() {
        let sr = 44_100.0;
        let mut half = Enclosure::default();
        half.set_target(0.5);
        settle(&mut half, 3.0, sr);
        let p = *half.params();
        let db = 20.0 * half.gain().log10();
        assert!(
            (db - 0.5 * p.floor_db).abs() < 0.3,
            "half-open gain {db} dB, expected {}",
            0.5 * p.floor_db
        );
    }
}
