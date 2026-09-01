//! The audio device and its stream: RT-priority promotion for the
//! callback thread, the cpal stream lifecycle (`AudioOutput`, rebuilt
//! whenever an organ load needs a new engine), device/format
//! selection, and the diagnostics recorder.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use aristide_engine::{Engine, EngineHandle};
use cpal::traits::{DeviceTrait, StreamTrait};

use crate::SHUTDOWN;

/// One-time setup on the audio callback's own thread (cpal creates it,
/// so this runs on first callback): real-time scheduling — without
/// SCHED_FIFO any desktop load preempts us and no buffer size saves you
/// — and flush-to-zero so denormals can't burn cycles.
fn audio_thread_setup(buffer_frames: u32, sample_rate: u32) {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        // MXCSR bit 15 = flush-to-zero, bit 6 = denormals-are-zero.
        let mut csr: u32 = 0;
        core::arch::asm!("stmxcsr [{}]", in(reg) &mut csr, options(nostack));
        csr |= 0x8000 | 0x0040;
        core::arch::asm!("ldmxcsr [{}]", in(reg) &csr, options(nostack, readonly));
    }
    let _ = (buffer_frames, sample_rate);
    #[cfg(target_os = "linux")]
    unsafe {
        let param = libc::sched_param { sched_priority: 70 };
        let result =
            libc::pthread_setschedparam(libc::pthread_self(), libc::SCHED_FIFO, &param);
        if result == 0 {
            tracing::info!("audio thread: SCHED_FIFO real-time priority acquired");
        } else {
            // Ordinary desktop users have no rtprio rlimit, so this is the
            // common path. Publish our kernel tid; a helper thread will ask
            // RealtimeKit (the mechanism PipeWire and every DAW use) to
            // promote us — rtkit only accepts requests by tid.
            let tid = libc::syscall(libc::SYS_gettid) as i64;
            AUDIO_TID.store(tid, std::sync::atomic::Ordering::Release);
            tracing::info!(
                "audio thread: direct SCHED_FIFO denied (errno {result}); \
                 asking RealtimeKit to promote tid {tid}"
            );
        }
    }
}

/// Kernel tid of the audio callback thread, published only when direct
/// SCHED_FIFO promotion failed and RealtimeKit should be tried.
#[cfg(target_os = "linux")]
static AUDIO_TID: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);

/// Promote the audio thread through the RealtimeKit D-Bus service, from a
/// normal thread (never the audio thread — D-Bus is IO). Uses `dbus-send`
/// (present on any desktop that has rtkit) instead of a D-Bus library
/// dependency. rtkit refuses processes without an RLIMIT_RTTIME ceiling —
/// the kernel's runaway-RT-thread guard; the accounting resets every time
/// the callback blocks on the device (~every buffer), so 200 ms of
/// *continuous* RT CPU only happens if we are catastrophically broken.
#[cfg(target_os = "linux")]
fn promote_audio_thread_via_rtkit() {
    use std::sync::atomic::Ordering;
    let mut tid = 0;
    // The first callback (which publishes the tid) fires within one buffer
    // of stream.play(); 3 s covers even a device that starts paused.
    for _ in 0..300 {
        tid = AUDIO_TID.load(Ordering::Acquire);
        if tid != 0 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    if tid == 0 {
        return; // direct promotion succeeded (or no callback ever ran)
    }
    unsafe {
        let limit = libc::rlimit {
            rlim_cur: 200_000, // µs of continuous RT CPU before SIGKILL
            rlim_max: 200_000,
        };
        libc::setrlimit(libc::RLIMIT_RTTIME, &limit);
    }
    let max_priority = std::process::Command::new("dbus-send")
        .args([
            "--system",
            "--print-reply",
            "--dest=org.freedesktop.RealtimeKit1",
            "/org/freedesktop/RealtimeKit1",
            "org.freedesktop.DBus.Properties.Get",
            "string:org.freedesktop.RealtimeKit1",
            "string:MaxRealtimePriority",
        ])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| {
            String::from_utf8_lossy(&o.stdout)
                .split_whitespace()
                .last()
                .and_then(|v| v.parse::<i64>().ok())
        })
        .unwrap_or(10)
        .clamp(1, 70);
    // MakeThreadRealtime only promotes threads of the D-Bus caller — and
    // our caller is dbus-send, a different process (rtkit answers ENOENT:
    // it looked for our tid inside dbus-send). The WithPID variant names
    // the target process explicitly; rtkit still checks it belongs to the
    // same uid, so this stays unprivileged.
    let request = std::process::Command::new("dbus-send")
        .args([
            "--system",
            "--print-reply",
            "--dest=org.freedesktop.RealtimeKit1",
            "/org/freedesktop/RealtimeKit1",
            "org.freedesktop.RealtimeKit1.MakeThreadRealtimeWithPID",
            &format!("uint64:{}", std::process::id()),
            &format!("uint64:{tid}"),
            &format!("uint32:{max_priority}"),
        ])
        .output();
    let policy = unsafe { libc::sched_getscheduler(tid as libc::pid_t) };
    if policy == libc::SCHED_FIFO || policy == libc::SCHED_RR {
        tracing::info!(
            "audio thread: real-time priority {max_priority} acquired via RealtimeKit"
        );
    } else {
        let detail = match request {
            Ok(out) if !out.status.success() => {
                String::from_utf8_lossy(&out.stderr).trim().to_string()
            }
            Ok(_) => "rtkit accepted but the policy did not change".into(),
            Err(err) => format!("dbus-send unavailable: {err}"),
        };
        tracing::warn!(
            "audio thread: NO real-time priority ({detail}) — audio will glitch \
             under desktop load. Manual fix: add '@audio - rtprio 95' to \
             /etc/security/limits.d/audio.conf, add your user to the 'audio' \
             group, log out/in. Quick test: \
             sudo setcap cap_sys_nice+ep <path-to-aristide-server>"
        );
    }
}
/// The audio device and the stream playing into it, owned by the main
/// thread (a cpal stream is not `Send`). The engine's sample bank is
/// fixed at its construction — the RT path never swaps pointers — so
/// loading an organ means a new engine, and a new engine means a new
/// stream; [`AudioOutput::start`] does both.
pub(crate) struct AudioOutput {
    pub(crate) device: cpal::Device,
    pub(crate) config: cpal::StreamConfig,
    pub(crate) channels: usize,
    pub(crate) sample_rate: f32,
    pub(crate) buffer_frames: u32,
    pub(crate) safe: bool,
    pub(crate) overruns: Arc<std::sync::atomic::AtomicU32>,
    pub(crate) dsp_peak_ns: Arc<std::sync::atomic::AtomicU64>,
    pub(crate) dsp_over_budget: Arc<std::sync::atomic::AtomicU32>,
    pub(crate) dsp_budget_ns: Arc<std::sync::atomic::AtomicU64>,
    /// Where each new engine's recording tap goes; the recorder thread
    /// drains them all into one WAV.
    pub(crate) record: Option<std::sync::mpsc::Sender<(rtrb::Consumer<f32>, u16)>>,
    pub(crate) stream: Option<cpal::Stream>,
}

impl AudioOutput {
    /// Widen the stream to at least `wanted` channels for a routed
    /// organ, keeping the sample rate (the bank was decoded against
    /// it). No such layout is a warning, not an error: the engine
    /// folds unreachable output pairs back onto the main pair.
    pub(crate) fn ensure_channels(&mut self, wanted: usize) {
        if wanted <= self.channels {
            return;
        }
        let rate = self.config.sample_rate.0;
        let found = self
            .device
            .supported_output_configs()
            .ok()
            .and_then(|configs| {
                configs
                    .filter(|range| range.sample_format() == cpal::SampleFormat::F32)
                    .filter(|range| range.channels() as usize >= wanted)
                    .filter(|range| {
                        (range.min_sample_rate().0..=range.max_sample_rate().0).contains(&rate)
                    })
                    .min_by_key(|range| range.channels())
                    .map(|range| range.with_sample_rate(cpal::SampleRate(rate)))
            });
        match found {
            Some(config) => {
                let config: cpal::StreamConfig = config.into();
                tracing::info!(
                    "audio: widening the stream to {} channels for routing",
                    config.channels
                );
                self.channels = config.channels as usize;
                self.config = config;
            }
            None => tracing::warn!(
                "audio: routing wants {wanted} channels but the device offers no such \
                 f32 layout at {rate} Hz — routed buses fold to the main pair"
            ),
        }
    }

    /// Replace the running engine (and its stream) with a fresh one on
    /// `bank`. Falls back to the backend's default buffer size if the
    /// requested one is refused.
    pub(crate) fn start(
        &mut self,
        bank: Arc<aristide_engine::bank::SampleBank>,
        reverb: Option<(Arc<aristide_engine::reverb::PreparedIr>, f32)>,
    ) -> Result<EngineHandle> {
        // Drop the old stream first: exclusive backends refuse a second
        // stream on the device, and the old engine (with its bank) dies
        // with it here on the control side, never on the audio thread.
        self.stream = None;
        #[cfg(target_os = "linux")]
        AUDIO_TID.store(0, std::sync::atomic::Ordering::Release);
        let (stream, handle) =
            match self.build(cpal::BufferSize::Fixed(self.buffer_frames), &bank, &reverb) {
                Ok(pair) => pair,
                Err(err) => {
                    tracing::warn!(
                        "device refused a {}-frame buffer ({err}); using its default \
                         (expect higher latency — try another --buffer value)",
                        self.buffer_frames
                    );
                    self.build(cpal::BufferSize::Default, &bank, &reverb)?
                }
            };
        stream.play()?;
        self.stream = Some(stream);
        // Each stream gets a fresh callback thread; promote it again.
        #[cfg(target_os = "linux")]
        std::thread::spawn(promote_audio_thread_via_rtkit);
        Ok(handle)
    }

    fn build(
        &self,
        buffer_size: cpal::BufferSize,
        bank: &Arc<aristide_engine::bank::SampleBank>,
        reverb: &Option<(Arc<aristide_engine::reverb::PreparedIr>, f32)>,
    ) -> Result<(cpal::Stream, EngineHandle)> {
        let (mut engine, handle) = Engine::new(self.sample_rate, Arc::clone(bank));
        engine.set_reverb(
            reverb.as_ref().map(|(ir, _)| Arc::clone(ir)),
            reverb.as_ref().map_or(0.0, |(_, wet)| *wet),
        );
        if self.safe {
            engine.set_lite(true);
        }
        let mut tap = None;
        if self.record.is_some() {
            // ~90 s of stereo headroom; the writer drains far faster.
            let (producer, consumer) = rtrb::RingBuffer::new(1 << 23);
            engine.set_tap(producer);
            tap = Some(consumer);
        }
        let mut stream_config = self.config.clone();
        stream_config.buffer_size = buffer_size;
        let mut rt_ready = false;
        let mut last_callback: Option<std::time::Instant> = None;
        let overruns = Arc::clone(&self.overruns);
        let dsp_peak_ns = Arc::clone(&self.dsp_peak_ns);
        let dsp_over_budget = Arc::clone(&self.dsp_over_budget);
        let dsp_budget_ns = Arc::clone(&self.dsp_budget_ns);
        let channels = self.channels;
        let sample_rate = self.sample_rate;
        let buffer_hint = self.buffer_frames;
        let mut engine = engine;
        let stream = self.device.build_output_stream(
            &stream_config,
            move |data: &mut [f32], _| {
                use std::sync::atomic::Ordering::Relaxed;
                if !rt_ready {
                    rt_ready = true;
                    audio_thread_setup(buffer_hint, sample_rate as u32);
                }
                let now = std::time::Instant::now();
                let nominal = data.len() as f64 / channels as f64 / sample_rate as f64;
                if let Some(previous) = last_callback {
                    if now.duration_since(previous).as_secs_f64() > nominal * 2.0 {
                        overruns.fetch_add(1, Relaxed);
                    }
                }
                last_callback = Some(now);
                engine.process(data, channels);
                let spent_ns = now.elapsed().as_nanos() as u64;
                let budget_ns = (nominal * 1e9) as u64;
                dsp_budget_ns.store(budget_ns, Relaxed);
                dsp_peak_ns.fetch_max(spent_ns, Relaxed);
                if spent_ns > budget_ns {
                    dsp_over_budget.fetch_add(1, Relaxed);
                }
            },
            |err| tracing::error!("audio stream error: {err}"),
            None,
        )?;
        // Only a real stream's tap reaches the recorder: a consumer
        // whose build failed would drain nothing forever.
        if let (Some(sender), Some(consumer)) = (&self.record, tap) {
            let _ = sender.send((consumer, self.channels.min(u16::MAX as usize) as u16));
        }
        Ok((stream, handle))
    }
}
/// The recording tap's writer: drains every engine's tap ring into one
/// WAV (16-bit PCM). Loading an organ replaces the engine, so taps
/// arrive over a channel and each engine's output is appended in turn.
#[allow(clippy::type_complexity)]
pub(crate) fn spawn_recorder(
    path: PathBuf,
    rate: u32,
) -> Result<(
    std::sync::mpsc::Sender<(rtrb::Consumer<f32>, u16)>,
    std::thread::JoinHandle<std::io::Result<()>>,
)> {
    tracing::info!("recording engine output to {}", path.display());
    let (sender, receiver) = std::sync::mpsc::channel::<(rtrb::Consumer<f32>, u16)>();
    let worker = std::thread::Builder::new()
        .name("aristide-record".into())
        .spawn(move || -> std::io::Result<()> {
            use std::io::{Seek, SeekFrom, Write};

            // The header's channel count comes from the tap, not from a
            // stereo assumption: routed buses can widen the stream, and
            // an N-channel tap under a 2-channel header is a corrupt
            // file. A mid-run channel change (organ load reopening the
            // device wider) rotates to a numbered segment file.
            fn open_segment(
                path: &std::path::Path,
                segment: u32,
                rate: u32,
                channels: u16,
            ) -> std::io::Result<std::io::BufWriter<std::fs::File>> {
                let target = if segment == 1 {
                    path.to_path_buf()
                } else {
                    let stem = path.file_stem().unwrap_or_default().to_string_lossy();
                    let ext = path.extension().unwrap_or_default().to_string_lossy();
                    path.with_file_name(format!("{stem}-{segment}.{ext}"))
                };
                if segment > 1 {
                    tracing::info!(
                        "recording continues in {} ({channels} channels)",
                        target.display()
                    );
                }
                let mut file = std::io::BufWriter::new(std::fs::File::create(target)?);
                // Placeholder RIFF/data sizes, patched by finish().
                file.write_all(b"RIFF\0\0\0\0WAVEfmt ")?;
                file.write_all(&16u32.to_le_bytes())?;
                file.write_all(&1u16.to_le_bytes())?; // PCM
                file.write_all(&channels.to_le_bytes())?;
                file.write_all(&rate.to_le_bytes())?;
                file.write_all(&(rate * 2 * u32::from(channels)).to_le_bytes())?;
                file.write_all(&(2 * channels).to_le_bytes())?;
                file.write_all(&16u16.to_le_bytes())?;
                file.write_all(b"data\0\0\0\0")?;
                Ok(file)
            }

            fn finish(
                file: &mut std::io::BufWriter<std::fs::File>,
                written: u32,
            ) -> std::io::Result<()> {
                let inner = file.get_mut();
                inner.seek(SeekFrom::Start(4))?;
                inner.write_all(&(36 + written).to_le_bytes())?;
                inner.seek(SeekFrom::Start(40))?;
                inner.write_all(&written.to_le_bytes())?;
                file.flush()
            }

            let mut file: Option<std::io::BufWriter<std::fs::File>> = None;
            let mut written: u32 = 0;
            let mut segment: u32 = 0;
            let mut channels: u16 = 0;
            let mut active: Vec<rtrb::Consumer<f32>> = Vec::new();
            let mut pending: std::collections::VecDeque<(rtrb::Consumer<f32>, u16)> =
                std::collections::VecDeque::new();
            while !SHUTDOWN.load(std::sync::atomic::Ordering::Relaxed) {
                while let Ok(tap) = receiver.try_recv() {
                    pending.push_back(tap);
                }
                // Adopt taps that match the current segment's channel
                // count (the first tap decides it).
                while let Some(&(_, tap_channels)) = pending.front() {
                    if file.is_none() {
                        channels = tap_channels.max(1);
                        segment += 1;
                        file = Some(open_segment(&path, segment, rate, channels)?);
                        written = 0;
                    } else if tap_channels != channels {
                        break;
                    }
                    active.push(pending.pop_front().expect("front exists").0);
                }
                if let Some(out) = file.as_mut() {
                    for tap in &mut active {
                        while let Ok(value) = tap.pop() {
                            let clamped = (value.clamp(-1.0, 1.0) * 32767.0) as i16;
                            out.write_all(&clamped.to_le_bytes())?;
                            written = written.saturating_add(2);
                        }
                    }
                }
                // A differently-shaped tap is waiting: once the current
                // engines are gone and drained, close this segment so
                // the next loop pass opens the new one.
                if !pending.is_empty()
                    && active
                        .iter()
                        .all(|tap| tap.is_abandoned() && tap.is_empty())
                {
                    if let Some(mut out) = file.take() {
                        finish(&mut out, written)?;
                    }
                    active.clear();
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            if let Some(mut out) = file.take() {
                finish(&mut out, written)?;
            }
            Ok(())
        })?;
    Ok((sender, worker))
}
/// M1+ supports f32 output only; PipeWire/JACK and ALSA's plug layer all
/// offer it. Format conversion becomes the server's job later.
///
/// CRITICAL: never take a range's MAX rate blindly — a device whose
/// default format isn't f32 used to land us on its highest f32 rate
/// (up to 192 kHz = 4× the CPU per voice), guaranteeing overruns that
/// no engine optimization could ever fix.
pub(crate) fn pick_f32_config(device: &cpal::Device) -> Result<cpal::StreamConfig> {
    let default = device.default_output_config()?;
    if default.sample_format() == cpal::SampleFormat::F32 {
        let rate = default.sample_rate().0;
        if rate > 96_000 {
            tracing::warn!(
                "device default is {rate} Hz — unusually high; engine cost \
                 scales with rate"
            );
        }
        return Ok(default.into());
    }
    let target = 48_000u32;
    let mut best: Option<cpal::StreamConfig> = None;
    let mut best_distance = u32::MAX;
    for range in device.supported_output_configs()? {
        if range.sample_format() != cpal::SampleFormat::F32 {
            continue;
        }
        let rate = target.clamp(range.min_sample_rate().0, range.max_sample_rate().0);
        let distance = rate.abs_diff(target);
        if distance < best_distance {
            best_distance = distance;
            best = Some(range.with_sample_rate(cpal::SampleRate(rate)).into());
        }
    }
    best.context("audio device offers no f32 output format")
}
