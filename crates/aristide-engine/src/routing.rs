//! Output buses: the first public slice of the effects graph.
//!
//! Every voice renders onto one of [`MAX_BUSES`] stereo buses; each bus
//! runs its insert effects (today: one delay node) and fans out to up
//! to [`MAX_SENDS`] output channel pairs at once, each at its own
//! level — a routing matrix, not a single patch cord. The default —
//! every voice on bus 0, no delay, one send to channels 0/1 at unity —
//! is bit-identical to the pre-bus engine, so an organ that never
//! mentions routing pays nothing.
//!
//! RT invariants hold: every buffer here is allocated at engine
//! construction (scratch for the largest render chunk, the delay ring
//! at its maximum length, the send list at its maximum count) and only
//! reconfigured through the command queue. Delay-time changes slew the
//! read head (~100 ms one-pole), so they bend pitch tape-style instead
//! of clicking — a feature, not an accident, for the Orgelpark bag of
//! tricks. Send-gain changes ramp linearly across one chunk for the
//! same reason: a fader move (or a send appearing/disappearing) must
//! not click.

/// Stereo buses available to route voices onto.
pub const MAX_BUSES: usize = 16;
/// Output channel pairs a single bus can feed at once.
pub const MAX_SENDS: usize = 8;
/// Largest sub-block rendered at once; callbacks bigger than this are
/// processed in slices so bus scratch can be sized once, up front.
pub const MAX_CHUNK_FRAMES: usize = 4096;
/// Longest configurable bus delay.
pub(crate) const MAX_DELAY_SECONDS: f32 = 2.0;

/// One tap from a bus onto an output channel pair, at a linear gain.
/// Channels are 0-based; a channel the device hasn't got falls back to
/// the main pair (0/1) at render time, same as `SetBusOutput` today.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Send {
    pub left: u8,
    pub right: u8,
    pub gain: f32,
}

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
    /// Fixed-size send list — the channel pair and target gain for
    /// each output this bus feeds. Unused slots carry gain 0 but keep
    /// their last channel pair, so a send that just disappeared still
    /// ramps its old pair down to silence instead of cutting there.
    sends: [Send; MAX_SENDS],
    /// Gain actually reached at the end of the last chunk, per slot —
    /// the ramp's start point for the next `finish_chunk`.
    send_gain_now: [f32; MAX_SENDS],
    /// The last chunk reached the output. A bus that is silent has
    /// nothing to click, so new sends take effect at once.
    sounding: bool,
    ring_capacity: usize,
    sample_rate: f32,
}

impl Bus {
    pub fn new(sample_rate: f32) -> Bus {
        let ring_capacity = (MAX_DELAY_SECONDS * sample_rate).ceil() as usize + 2;
        let mut sends = [Send { left: 0, right: 1, gain: 0.0 }; MAX_SENDS];
        sends[0] = Send { left: 0, right: 1, gain: 1.0 };
        let mut send_gain_now = [0.0; MAX_SENDS];
        send_gain_now[0] = 1.0;
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
            sends,
            send_gain_now,
            sounding: false,
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

    /// Replace the bus's whole send list. Sends beyond `MAX_SENDS` are
    /// dropped; slots past the new count are zeroed to gain 0 so any
    /// send that just fell off ramps its old pair down to silence
    /// instead of cutting. Gain is clamped 0..=4; a gain that changed
    /// (including 0<->nonzero) ramps linearly across the next chunk
    /// from its previous value — no click.
    ///
    /// A send keeps the slot already feeding its pair, so a list that
    /// was reordered or shortened ramps each pair from where it stood
    /// rather than moving a slot's level onto another speaker.
    pub fn set_sends(&mut self, sends: &[Send]) {
        let count = sends.len().min(MAX_SENDS);
        let mut taken = [false; MAX_SENDS];
        let mut placed = [None; MAX_SENDS];
        for (index, send) in sends.iter().take(count).enumerate() {
            let same_pair = (0..MAX_SENDS).find(|&slot| {
                !taken[slot]
                    && self.sends[slot].left == send.left
                    && self.sends[slot].right == send.right
                    && (self.sends[slot].gain > 0.0 || self.send_gain_now[slot] > 0.0)
            });
            if let Some(slot) = same_pair {
                taken[slot] = true;
                placed[index] = Some(slot);
            }
        }
        for (index, send) in sends.iter().take(count).enumerate() {
            let slot = match placed[index] {
                Some(slot) => slot,
                None => {
                    // Prefer a silent slot; failing that, one that is
                    // only ramping down, which then starts from silence.
                    let silent = (0..MAX_SENDS).find(|&slot| {
                        !taken[slot] && self.sends[slot].gain == 0.0 && self.send_gain_now[slot] == 0.0
                    });
                    let slot = silent
                        .or_else(|| (0..MAX_SENDS).find(|&slot| !taken[slot]))
                        .expect("no more sends than slots");
                    self.send_gain_now[slot] = 0.0;
                    taken[slot] = true;
                    slot
                }
            };
            self.sends[slot] = Send {
                left: send.left,
                right: send.right,
                gain: send.gain.clamp(0.0, 4.0),
            };
        }
        for slot in (0..MAX_SENDS).filter(|&slot| !taken[slot]) {
            self.sends[slot].gain = 0.0;
        }
        if !self.sounding {
            for (now, send) in self.send_gain_now.iter_mut().zip(&self.sends) {
                *now = send.gain;
            }
        }
    }

    /// Shorthand for a single send: `set_sends(&[Send { left, right, gain }])`.
    pub fn set_output(&mut self, left: u8, right: u8, gain: f32) {
        self.set_sends(&[Send { left, right, gain }]);
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
            self.sounding = false;
            return;
        }
        self.sounding = true;
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
        // Each send adds its own scaled copy of the (now wet/dry-mixed)
        // scratch onto the output — one bus, several destinations. A
        // send whose gain didn't move this chunk (the overwhelming
        // common case) costs a constant multiply; one that did ramps
        // linearly from its last-reached value so appearing,
        // disappearing or faded sends never step.
        for i in 0..MAX_SENDS {
            let send = self.sends[i];
            let start = self.send_gain_now[i];
            let target = send.gain;
            if start == 0.0 && target == 0.0 {
                continue;
            }
            let (left, right) = if channels <= 1 {
                (0, 0)
            } else if (send.left as usize) < channels && (send.right as usize) < channels {
                (send.left as usize, send.right as usize)
            } else {
                (0, 1)
            };
            if channels == 1 {
                for (frame, sample) in out.iter_mut().take(frames).enumerate() {
                    let g = start + (target - start) * ((frame + 1) as f32 / frames as f32);
                    *sample += (self.scratch[frame * 2] + self.scratch[frame * 2 + 1]) * 0.5 * g;
                }
            } else {
                for frame in 0..frames {
                    let g = start + (target - start) * ((frame + 1) as f32 / frames as f32);
                    out[frame * channels + left] += self.scratch[frame * 2] * g;
                    out[frame * channels + right] += self.scratch[frame * 2 + 1] * g;
                }
            }
            self.send_gain_now[i] = target;
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
    fn a_send_gain_change_ramps_instead_of_stepping() {
        let mut bus = Bus::new(100.0);
        let frames = 10;
        // Settle at gain 1.0 (the default), then drop to 0.2 mid-signal.
        bus.begin_chunk(frames);
        bus.mix_target(frames)[0] = 1.0;
        let mut out = vec![0.0f32; frames * 2];
        bus.finish_chunk(frames, &mut out, 2);
        bus.set_output(0, 1, 0.2);
        bus.begin_chunk(frames);
        let scratch = bus.mix_target(frames);
        for frame in 0..frames {
            scratch[frame * 2] = 1.0;
        }
        let mut out = vec![0.0f32; frames * 2];
        bus.finish_chunk(frames, &mut out, 2);
        // No step: first frame is close to the old gain, not the new one.
        assert!(out[0] > 0.5, "first frame close to the old gain: {}", out[0]);
        // Strictly decreasing toward the new gain, and reaches it by
        // the last frame — a ramp, not a jump.
        for frame in 1..frames {
            assert!(
                out[frame * 2] <= out[(frame - 1) * 2] + 1e-6,
                "gain should decrease monotonically toward the target"
            );
        }
        assert!((out[(frames - 1) * 2] - 0.2).abs() < 1e-4, "settles at the new gain");
    }

    #[test]
    fn a_reordered_send_list_keeps_each_pair_at_its_level() {
        let mut bus = Bus::new(100.0);
        let frames = 10;
        let run = |bus: &mut Bus| {
            bus.begin_chunk(frames);
            let scratch = bus.mix_target(frames);
            for frame in 0..frames {
                scratch[frame * 2] = 1.0;
            }
            let mut out = vec![0.0f32; frames * 4];
            bus.finish_chunk(frames, &mut out, 4);
            out
        };
        bus.set_sends(&[Send { left: 0, right: 1, gain: 1.0 }, Send { left: 2, right: 3, gain: 0.5 }]);
        run(&mut bus);
        bus.set_sends(&[Send { left: 2, right: 3, gain: 0.5 }, Send { left: 0, right: 1, gain: 1.0 }]);
        let out = run(&mut bus);
        assert_eq!((out[0], out[2]), (1.0, 0.5), "no pair changed level");
    }

    #[test]
    fn set_sends_replaces_the_whole_list_and_silences_dropped_slots() {
        let mut bus = Bus::new(100.0);
        bus.set_sends(&[
            Send { left: 0, right: 1, gain: 0.5 },
            Send { left: 2, right: 3, gain: 0.25 },
        ]);
        let frames = 20;
        // Settle the ramp.
        bus.begin_chunk(frames);
        bus.mix_target(frames)[0] = 1.0;
        let mut out = vec![0.0f32; frames * 4];
        bus.finish_chunk(frames, &mut out, 4);
        // Steady measurement.
        bus.begin_chunk(frames);
        let scratch = bus.mix_target(frames);
        for frame in 0..frames {
            scratch[frame * 2] = 1.0;
        }
        let mut out = vec![0.0f32; frames * 4];
        bus.finish_chunk(frames, &mut out, 4);
        assert_eq!(out[0], 0.5, "first send at its own gain");
        assert_eq!(out[2], 0.25, "second send at its own gain");

        // Replacing with a single send drops the second one to silence.
        bus.set_sends(&[Send { left: 0, right: 1, gain: 1.0 }]);
        for _ in 0..2 {
            bus.begin_chunk(frames);
            let scratch = bus.mix_target(frames);
            for frame in 0..frames {
                scratch[frame * 2] = 1.0;
            }
            let mut out = vec![0.0f32; frames * 4];
            bus.finish_chunk(frames, &mut out, 4);
            if out[2] == 0.0 {
                break;
            }
        }
        bus.begin_chunk(frames);
        bus.mix_target(frames)[0] = 1.0;
        let mut out = vec![0.0f32; frames * 4];
        bus.finish_chunk(frames, &mut out, 4);
        assert_eq!(out[2], 0.0, "dropped send stays silent");
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
