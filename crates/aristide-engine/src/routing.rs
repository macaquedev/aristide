//! Output buses: the first public slice of the effects graph.
//!
//! Every voice renders onto one of [`MAX_BUSES`] stereo buses; each bus
//! runs its insert effects (today: one delay node) and lands on a
//! chosen pair of output channels. The default — every voice on bus 0,
//! no delay, channels 0/1 — is bit-identical to the pre-bus engine, so
//! an organ that never mentions routing pays nothing.
//!
//! RT invariants hold: every buffer here is allocated at engine
//! construction (scratch for the largest render chunk, the delay ring
//! at its maximum length) and only reconfigured through the command
//! queue. Delay-time changes slew the read head (~100 ms one-pole), so
//! they bend pitch tape-style instead of clicking — a feature, not an
//! accident, for the Orgelpark bag of tricks.

/// Stereo buses available to route voices onto.
pub const MAX_BUSES: usize = 8;
/// Largest sub-block rendered at once; callbacks bigger than this are
/// processed in slices so bus scratch can be sized once, up front.
pub const MAX_CHUNK_FRAMES: usize = 4096;
/// Longest configurable bus delay.
pub const MAX_DELAY_SECONDS: f32 = 2.0;

/// One bus's delay node. `mix` is the wet level added to the dry
/// signal (0 bypasses the node entirely); `dry` scales the undelayed
/// signal, so `dry: 0.0, mix: 1.0` *displaces* a division in time
/// rather than echoing it. `feedback` recirculates the wet tap.
#[derive(Debug, Clone, Copy)]
pub struct DelayParams {
    pub seconds: f32,
    pub feedback: f32,
    pub mix: f32,
    pub dry: f32,
}

impl Default for DelayParams {
    fn default() -> Self {
        DelayParams {
            seconds: 0.0,
            feedback: 0.0,
            mix: 0.0,
            dry: 1.0,
        }
    }
}

pub struct Bus {
    /// Interleaved stereo accumulation for the current chunk.
    scratch: Vec<f32>,
    /// Any voice wrote into `scratch` this chunk.
    used: bool,
    /// Wet energy may still be draining out of the delay ring even
    /// when no voice feeds the bus; counts down in frames.
    ringing: u64,
    /// Interleaved stereo delay ring, `MAX_DELAY_SECONDS` long.
    ring: Vec<f32>,
    /// Write head, in frames.
    write: usize,
    /// Read distance behind the write head, in frames — slewed toward
    /// `target_delay` so time changes glide instead of clicking.
    delay_frames: f32,
    target_delay: f32,
    /// One-pole coefficient for that slew (~100 ms).
    slew: f32,
    feedback: f32,
    mix: f32,
    dry: f32,
    /// Output routing: the interleaved channel pair this bus lands on,
    /// and the level it lands at.
    pub left_out: u8,
    pub right_out: u8,
    pub gain: f32,
    ring_capacity: usize,
    sample_rate: f32,
}

impl Bus {
    pub fn new(sample_rate: f32) -> Bus {
        let ring_capacity = (MAX_DELAY_SECONDS * sample_rate).ceil() as usize + 2;
        Bus {
            scratch: vec![0.0; MAX_CHUNK_FRAMES * 2],
            used: false,
            ringing: 0,
            ring: vec![0.0; ring_capacity * 2],
            write: 0,
            delay_frames: 0.0,
            target_delay: 0.0,
            slew: 1.0 - (-1.0 / (0.1 * sample_rate)).exp(),
            feedback: 0.0,
            mix: 0.0,
            dry: 1.0,
            left_out: 0,
            right_out: 1,
            gain: 1.0,
            ring_capacity,
            sample_rate,
        }
    }

    pub fn set_delay(&mut self, params: DelayParams) {
        let seconds = params.seconds.clamp(0.0, MAX_DELAY_SECONDS);
        self.target_delay = seconds * self.sample_rate;
        self.feedback = params.feedback.clamp(0.0, 0.95);
        self.mix = params.mix.clamp(0.0, 4.0);
        self.dry = params.dry.clamp(0.0, 4.0);
    }

    pub fn set_output(&mut self, left: u8, right: u8, gain: f32) {
        self.left_out = left;
        self.right_out = right;
        self.gain = gain.clamp(0.0, 4.0);
    }

    /// Zero the chunk scratch and report whether the bus can be
    /// skipped entirely this chunk (nothing playing, nothing ringing).
    pub fn begin_chunk(&mut self, frames: usize) -> &mut [f32] {
        self.used = false;
        let scratch = &mut self.scratch[..frames * 2];
        scratch.fill(0.0);
        scratch
    }

    /// The scratch to mix a voice into (marks the bus live).
    #[inline]
    pub fn mix_target(&mut self, frames: usize) -> &mut [f32] {
        self.used = true;
        &mut self.scratch[..frames * 2]
    }

    /// Run the insert chain over this chunk's scratch and add the
    /// result onto the interleaved output. Mono outputs fold the pair;
    /// a routed channel the device hasn't got falls back to the main
    /// pair — a misconfigured rig should sound wrong, not go silent.
    pub fn finish_chunk(&mut self, frames: usize, out: &mut [f32], channels: usize) {
        let delay_active = self.mix > 0.0 || self.ringing > 0;
        if !self.used && !delay_active {
            return;
        }
        if self.mix > 0.0 {
            if self.used {
                // Wet energy persists for the delay length plus a
                // feedback-scaled allowance for the recirculation tail.
                let tail = (self.target_delay.max(self.delay_frames) * 2.0
                    + MAX_DELAY_SECONDS * self.sample_rate * self.feedback * 8.0)
                    as u64;
                self.ringing = tail.max(1);
            }
            for frame in 0..frames {
                self.delay_frames += self.slew * (self.target_delay - self.delay_frames);
                let dry_l = self.scratch[frame * 2];
                let dry_r = self.scratch[frame * 2 + 1];
                // Fractional read behind the write head, linearly
                // interpolated (a slewing head sweeps between frames).
                let behind = self.delay_frames.max(0.0);
                let whole = behind as usize;
                let fract = behind - whole as f32;
                let read = |offset: usize, lane: usize, ring: &[f32]| {
                    let index =
                        (self.write + self.ring_capacity - offset.min(self.ring_capacity - 1))
                            % self.ring_capacity;
                    ring[index * 2 + lane]
                };
                let wet_l = read(whole, 0, &self.ring) * (1.0 - fract)
                    + read(whole + 1, 0, &self.ring) * fract;
                let wet_r = read(whole, 1, &self.ring) * (1.0 - fract)
                    + read(whole + 1, 1, &self.ring) * fract;
                self.ring[self.write * 2] = dry_l + wet_l * self.feedback;
                self.ring[self.write * 2 + 1] = dry_r + wet_r * self.feedback;
                self.write = (self.write + 1) % self.ring_capacity;
                self.scratch[frame * 2] = dry_l * self.dry + wet_l * self.mix;
                self.scratch[frame * 2 + 1] = dry_r * self.dry + wet_r * self.mix;
            }
            self.ringing = self.ringing.saturating_sub(frames as u64);
        } else if self.ringing > 0 {
            // Node just disabled: let the ring forget quietly.
            self.ringing = 0;
            self.ring.fill(0.0);
        }
        let gain = self.gain;
        let (left, right) = if channels <= 1 {
            (0, 0)
        } else if (self.left_out as usize) < channels && (self.right_out as usize) < channels {
            (self.left_out as usize, self.right_out as usize)
        } else {
            (0, 1)
        };
        if channels == 1 {
            for (frame, sample) in out.iter_mut().take(frames).enumerate() {
                *sample +=
                    (self.scratch[frame * 2] + self.scratch[frame * 2 + 1]) * 0.5 * gain;
            }
        } else {
            for frame in 0..frames {
                out[frame * channels + left] += self.scratch[frame * 2] * gain;
                out[frame * channels + right] += self.scratch[frame * 2 + 1] * gain;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_default_bus_passes_audio_to_the_main_pair_unchanged() {
        let mut bus = Bus::new(100.0);
        let frames = 8;
        bus.begin_chunk(frames);
        let scratch = bus.mix_target(frames);
        for frame in 0..frames {
            scratch[frame * 2] = 0.25;
            scratch[frame * 2 + 1] = -0.5;
        }
        let mut out = vec![0.0f32; frames * 2];
        bus.finish_chunk(frames, &mut out, 2);
        for frame in 0..frames {
            assert_eq!(out[frame * 2], 0.25);
            assert_eq!(out[frame * 2 + 1], -0.5);
        }
    }

    #[test]
    fn routing_lands_on_the_chosen_pair_and_falls_back_when_absent() {
        let mut bus = Bus::new(100.0);
        bus.set_output(2, 3, 1.0);
        let frames = 4;
        bus.begin_chunk(frames);
        let scratch = bus.mix_target(frames);
        scratch[0] = 1.0;
        scratch[1] = 0.5;
        let mut out = vec![0.0f32; frames * 4];
        bus.finish_chunk(frames, &mut out, 4);
        assert_eq!(out[2], 1.0, "left lands on channel 2");
        assert_eq!(out[3], 0.5, "right lands on channel 3");
        assert_eq!(out[0], 0.0);
        // The same routing on a stereo device: main pair, not silence.
        let mut bus = Bus::new(100.0);
        bus.set_output(2, 3, 1.0);
        bus.begin_chunk(frames);
        bus.mix_target(frames)[0] = 1.0;
        let mut out = vec![0.0f32; frames * 2];
        bus.finish_chunk(frames, &mut out, 2);
        assert_eq!(out[0], 1.0, "fallback to the main pair");
    }

    #[test]
    fn the_delay_node_echoes_at_the_set_distance() {
        let sample_rate = 100.0;
        let mut bus = Bus::new(sample_rate);
        bus.set_delay(DelayParams {
            seconds: 0.1, // 10 frames
            feedback: 0.0,
            mix: 1.0,
            dry: 1.0,
        });
        // Let the read-head slew settle before measuring.
        for _ in 0..50 {
            bus.begin_chunk(16);
            let mut out = vec![0.0f32; 16 * 2];
            bus.finish_chunk(16, &mut out, 2);
        }
        // An impulse: dry now, wet copy 10 frames later.
        bus.begin_chunk(32);
        bus.mix_target(32)[0] = 1.0;
        let mut out = vec![0.0f32; 32 * 2];
        bus.finish_chunk(32, &mut out, 2);
        assert_eq!(out[0], 1.0, "dry impulse");
        let peak = (1..32).max_by(|&a, &b| out[a * 2].total_cmp(&out[b * 2])).unwrap();
        assert_eq!(peak, 10, "echo lands 10 frames later: {:?}", &out[..24]);
        assert!(out[peak * 2] > 0.9);
    }

    #[test]
    fn an_idle_bus_with_no_delay_is_skipped() {
        let mut bus = Bus::new(100.0);
        bus.begin_chunk(8);
        let mut out = vec![0.5f32; 16];
        bus.finish_chunk(8, &mut out, 2);
        assert!(out.iter().all(|&v| v == 0.5), "output untouched");
    }
}
