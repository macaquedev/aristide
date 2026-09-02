//! Disk streaming: the release material a set is too big to keep in RAM.
//!
//! What streams and why. A held note must never depend on the disk, so
//! everything from the first frame through the end of the last sustain
//! loop stays resident exactly as before — the attack and the loop are
//! bit-for-bit what they were. What dominates a set's bytes is the
//! *release*: tails run 5–15 s against 1–3 s of attack-plus-loop, and a
//! tail is read exactly once, forward, by one voice. That is textbook
//! streaming material.
//!
//! The head of every tail stays resident too ([`HEAD_SECONDS`]). The
//! splice at note-off is the most delicate moment in the engine (phase
//! alignment, level match, a crossfade up to 184 ms long); it must
//! start on the same instant the key comes up, whatever the disk is
//! doing. The head is sized so it cannot be exhausted before a streamer
//! thread can answer: worst case a worker is mid-sweep over all its
//! slots (160 slots ÷ 2 workers × ~0.2 ms per 64 KiB SSD read ≈ 16 ms),
//! plus its 1 ms poll period and the read itself — call it 20 ms on a
//! solid-state disk. 350 ms of source is >10× that at unity rate, and
//! still ~4× for a pipe repitched two octaves up (which consumes source
//! frames 4× as fast). It also exceeds the longest crossfade, so the
//! splice itself is always RAM.
//!
//! The RT side owns a fixed pool of slots, allocated once at engine
//! construction. Each slot is an SPSC byte ring written by a streamer
//! thread and read by the audio thread into a small linear window — the
//! sinc reader needs `taps` contiguous frames, so the window (not the
//! ring) is what it dots against. Resident reads never touch any of
//! this: they run the identical code they always did.
//!
//! Nothing here allocates, locks, or does I/O on the audio thread. The
//! streamer threads poll their rings' free space (an atomic load) and
//! fill them; the audio thread never signals them, so there is not even
//! a futex wake in the callback.

use std::fs::File;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use rtrb::{Consumer, Producer, RingBuffer};

use crate::bank::{SampleBank, SampleFormat, StreamRegion};

/// Resident head of every streamed region, in seconds of source audio.
/// See the module docs for the derivation.
pub const HEAD_SECONDS: f64 = 0.35;

/// Frames of the resident region duplicated at the front of the stored
/// region. A sinc read needs taps *before* its position; the widest
/// kernel is 64 taps, so the store starts 64 frames early and the
/// window can cover the crossing without ever splicing RAM to disk
/// mid-kernel.
pub const OVERLAP_FRAMES: u64 = 64;

/// How far before the resident end a reader must switch to the stream
/// window. The widest kernel reads 31 frames back of its position and
/// 33 forward, and the stored region begins [`OVERLAP_FRAMES`] early,
/// so `resident_end − 33` is the one crossing where *both* sides could
/// serve a whole kernel — below it RAM holds every tap, at or above it
/// the window does. Any other choice would let one kernel straddle the
/// two and read clamped garbage on one side of the seam.
pub const CROSSOVER_FRAMES: u64 = OVERLAP_FRAMES / 2 + 1;

/// Stream slots. Only release tails stream, and the engine already caps
/// concurrent tails at `TAIL_VOICE_BUDGET` (128) — the rest is headroom
/// for the blocks it takes shedding to catch up with a mass release,
/// plus the occasional long one-shot.
pub const DEFAULT_SLOTS: usize = 160;

/// Streamer threads. Throughput is trivial (a stereo 16-bit voice at
/// unity eats ~190 KB/s; 128 of them ~24 MB/s), so the reason for more
/// than one is latency: a sweep's worth of read syscalls is serialized
/// per thread, and two threads halve the worst case.
pub const DEFAULT_WORKERS: usize = 2;

/// Bytes per slot ring: 128 KiB = 0.34 s of stereo 16-bit at 48 kHz,
/// ~10× the worst-case fill latency the head is sized for. The whole
/// pool is 160 × (128 KiB ring + 8 KiB window) ≈ 21 MiB — a fixed cost
/// paid only by organs that actually stream, and small against the
/// gigabytes that streaming is there to not hold.
const RING_BYTES: usize = 128 * 1024;

/// Frames in a slot's linear window. Big enough that a refill happens
/// at most every few hundred output frames, small enough that the
/// memmove behind it is a few KiB.
const WINDOW_FRAMES: usize = 1024;

/// Biggest read a worker issues per slot per pass. Large enough to
/// amortize the syscall, small enough that one slow slot cannot stall
/// the sweep.
const FILL_BYTES: usize = 64 * 1024;

/// Slot handle meaning "this voice holds no slot".
pub const NO_SLOT: u16 = u16::MAX;

/// Slot handle meaning "this voice asked and the pool was empty". It
/// does not ask again: a slot acquired halfway through a tail would
/// start delivering from the tail's beginning, seconds behind the
/// cursor. The voice plays its resident head and the EOF guard fades
/// it — and the denial is counted exactly once.
pub const DENIED_SLOT: u16 = u16::MAX - 1;

/// A real slot, as opposed to [`NO_SLOT`] / [`DENIED_SLOT`].
#[inline]
pub(crate) fn holds_slot(index: u16) -> bool {
    index < DENIED_SLOT
}

/// The files a bank's streamed regions live in. Opened control-side,
/// read by the streamer threads with positional reads so several
/// threads can share one handle without a seek race.
pub struct StreamStores {
    files: Vec<File>,
}

impl std::fmt::Debug for StreamStores {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "StreamStores({} files)", self.files.len())
    }
}

impl StreamStores {
    pub fn new() -> StreamStores {
        StreamStores { files: Vec::new() }
    }

    /// Register an already-open store, returning its index.
    pub fn push(&mut self, file: File) -> u16 {
        self.files.push(file);
        (self.files.len() - 1) as u16
    }

    pub fn open(&mut self, path: &Path) -> std::io::Result<u16> {
        Ok(self.push(File::open(path)?))
    }

    pub fn len(&self) -> usize {
        self.files.len()
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    /// Fill `buf` from `store` at `offset`. Positional (pread): no
    /// shared cursor, so streamer threads never serialize on a seek.
    pub fn read_exact_at(
        &self,
        store: u16,
        offset: u64,
        buf: &mut [u8],
    ) -> std::io::Result<()> {
        let file = self.files.get(store as usize).ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::NotFound, "stream store missing")
        })?;
        let mut done = 0usize;
        while done < buf.len() {
            let read = read_at(file, offset + done as u64, &mut buf[done..])?;
            if read == 0 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "stream store truncated",
                ));
            }
            done += read;
        }
        Ok(())
    }
}

impl Default for StreamStores {
    fn default() -> Self {
        StreamStores::new()
    }
}

#[cfg(unix)]
fn read_at(file: &File, offset: u64, buf: &mut [u8]) -> std::io::Result<usize> {
    std::os::unix::fs::FileExt::read_at(file, buf, offset)
}

#[cfg(windows)]
fn read_at(file: &File, offset: u64, buf: &mut [u8]) -> std::io::Result<usize> {
    std::os::windows::fs::FileExt::seek_read(file, buf, offset)
}

/// Somewhere streamed audio can be appended, returning the byte offset
/// it landed at. Implemented by the loader's spool file and by the
/// load cache's tail file.
pub trait TailSink {
    fn append(&mut self, bytes: &[u8]) -> std::io::Result<u64>;
}

/// What the audio thread asks a streamer thread for.
enum SlotRequest {
    Start {
        slot: u16,
        region: StreamRegion,
        bytes_per_frame: u32,
    },
    Stop {
        slot: u16,
    },
}

/// One slot, audio-thread side: the consuming end of its ring plus the
/// contiguous window the sinc reader dots against.
struct Slot {
    ring: Consumer<u8>,
    /// `WINDOW_FRAMES × channels` values in the sample's own resident
    /// format, held as f32-aligned storage and viewed as bytes when
    /// filling (the POD byte-view idiom the sample cache already uses).
    window: Vec<f32>,
    /// Sample frame of the window's first frame.
    window_start: u64,
    /// Frames currently valid in the window.
    window_frames: usize,
    /// Sample frame the ring will deliver next.
    next_frame: u64,
    channels: usize,
    format: SampleFormat,
    /// Last frame the store holds for this region (inclusive).
    last_frame: u64,
    /// The ring ran dry before the region ended — the voice must fade.
    underrun: bool,
    state: SlotState,
    /// Which worker owns this slot's producer.
    worker: usize,
}

#[derive(Clone, Copy, PartialEq)]
enum SlotState {
    Free,
    Active,
    /// Stop sent; the worker still owns the producer until it acks.
    Stopping,
}

/// A slot's data as the reader sees it: a contiguous window of frames
/// in the sample's resident format.
pub(crate) enum StreamWindow<'a> {
    F32(&'a [f32]),
    I16(&'a [i16]),
}

impl StreamWindow<'_> {
    /// Linearly interpolated stereo read — the lite/diagnostic reader's
    /// view of a streamed window, matching `Sample::read`.
    #[inline]
    pub(crate) fn read_linear(
        &self,
        window_start: u64,
        window_frames: usize,
        channels: usize,
        position: f64,
    ) -> (f32, f32) {
        let last = window_frames.saturating_sub(1) as i64;
        let local = position.floor() as i64 - window_start as i64;
        let index = local.clamp(0, last) as usize;
        let next = (local + 1).clamp(0, last) as usize;
        let fraction = (position - position.floor()) as f32;
        let at = |frame: usize, channel: usize| match self {
            StreamWindow::F32(all) => all[frame * channels + channel],
            StreamWindow::I16(all) => {
                f32::from(all[frame * channels + channel]) * crate::bank::I16_SCALE
            }
        };
        let left = at(index, 0) + (at(next, 0) - at(index, 0)) * fraction;
        if channels == 1 {
            return (left, left);
        }
        let right = at(index, 1) + (at(next, 1) - at(index, 1)) * fraction;
        (left, right)
    }
}

/// Streaming health, shared with whoever reports it. Cumulative across
/// organ loads, like the audio callback's own overrun count.
#[derive(Clone, Default)]
pub struct StreamCounters {
    /// Voices whose ring ran dry mid-tail; each took a fast fade.
    pub underruns: Arc<AtomicU64>,
    /// Releases that found no free slot and played their resident head
    /// only.
    pub denials: Arc<AtomicU64>,
}

/// The audio thread's side of the pool.
pub struct StreamRt {
    slots: Vec<Slot>,
    free: Vec<u16>,
    requests: Vec<Producer<SlotRequest>>,
    returns: Vec<Consumer<u16>>,
    counters: StreamCounters,
}

/// One streamer thread's side: the producing ends of the slots it owns.
pub struct StreamWorker {
    stores: Arc<StreamStores>,
    slots: Vec<WorkerSlot>,
    requests: Consumer<SlotRequest>,
    returns: Producer<u16>,
    scratch: Vec<u8>,
    /// Slots are dealt round-robin, so this worker owns the global slot
    /// indices `local × stride + worker`; `stride` maps one to the
    /// other.
    stride: usize,
    stop: Arc<AtomicBool>,
}

struct WorkerSlot {
    ring: Producer<u8>,
    store: u16,
    offset: u64,
    remaining: u64,
    active: bool,
}

/// Build the pool for `bank`, or `None` when nothing in it streams.
/// Control-side: this is where every allocation the streaming path will
/// ever need is made.
pub fn attach(
    bank: &SampleBank,
    slots: usize,
    workers: usize,
    counters: StreamCounters,
) -> Option<(StreamRt, Vec<StreamWorker>)> {
    let stores = bank.stores()?;
    if bank.streamed_bytes() == 0 {
        return None;
    }
    let workers = workers.max(1);
    let slots = slots.max(workers);
    let stop = Arc::new(AtomicBool::new(false));

    let mut rt_slots: Vec<Slot> = Vec::with_capacity(slots);
    let mut worker_slots: Vec<Vec<WorkerSlot>> = (0..workers).map(|_| Vec::new()).collect();
    for index in 0..slots {
        let worker = index % workers;
        let (producer, consumer) = RingBuffer::new(RING_BYTES);
        rt_slots.push(Slot {
            ring: consumer,
            window: vec![0.0; WINDOW_FRAMES * 2],
            window_start: 0,
            window_frames: 0,
            next_frame: 0,
            channels: 1,
            format: SampleFormat::F32,
            last_frame: 0,
            underrun: false,
            state: SlotState::Free,
            worker,
        });
        worker_slots[worker].push(WorkerSlot {
            ring: producer,
            store: 0,
            offset: 0,
            remaining: 0,
            active: false,
        });
    }

    let mut request_producers = Vec::with_capacity(workers);
    let mut return_consumers = Vec::with_capacity(workers);
    let mut built = Vec::with_capacity(workers);
    for slots_for_worker in worker_slots {
        let count = slots_for_worker.len().max(1);
        let (request_tx, request_rx) = RingBuffer::new(count * 4);
        let (return_tx, return_rx) = RingBuffer::new(count * 4);
        request_producers.push(request_tx);
        return_consumers.push(return_rx);
        built.push(StreamWorker {
            stores: Arc::clone(&stores),
            slots: slots_for_worker,
            requests: request_rx,
            returns: return_tx,
            scratch: vec![0u8; FILL_BYTES],
            stride: workers,
            stop: Arc::clone(&stop),
        });
    }

    // Hand slots out from the end so the first voices land on worker 0,
    // 1, 0, … (the free list pops from the back).
    let free: Vec<u16> = (0..slots as u16).rev().collect();
    Some((
        StreamRt {
            slots: rt_slots,
            free,
            requests: request_producers,
            returns: return_consumers,
            counters,
        },
        built,
    ))
}

impl StreamRt {
    pub fn slot_count(&self) -> usize {
        self.slots.len()
    }

    /// Slots currently serving a voice.
    pub fn active_slots(&self) -> usize {
        self.slots
            .iter()
            .filter(|slot| slot.state == SlotState::Active)
            .count()
    }

    /// Collect slots the workers have finished with and make them
    /// reusable. Called once per block, before any acquisition.
    pub(crate) fn reclaim(&mut self) {
        for worker in 0..self.returns.len() {
            while let Ok(index) = self.returns[worker].pop() {
                let slot = &mut self.slots[index as usize];
                // The worker has stopped producing, so whatever is left
                // in the ring is ours to throw away.
                let leftover = slot.ring.slots();
                if leftover > 0 && let Ok(chunk) = slot.ring.read_chunk(leftover) {
                    chunk.commit_all();
                }
                slot.window_frames = 0;
                slot.underrun = false;
                slot.state = SlotState::Free;
                self.free.push(index);
            }
        }
    }

    /// Arm a slot on `region`. Returns [`NO_SLOT`] when the pool is
    /// exhausted — the caller then plays the resident head and fades.
    pub(crate) fn acquire(
        &mut self,
        region: StreamRegion,
        channels: usize,
        format: SampleFormat,
    ) -> u16 {
        let Some(index) = self.free.pop() else {
            self.counters.denials.fetch_add(1, Ordering::Relaxed);
            return NO_SLOT;
        };
        let bytes_per_frame = (channels * format.bytes()) as u32;
        let slot = &mut self.slots[index as usize];
        slot.channels = channels;
        slot.format = format;
        slot.window_start = region.first_frame;
        slot.window_frames = 0;
        slot.next_frame = region.first_frame;
        slot.last_frame = region.first_frame + region.frames.saturating_sub(1);
        slot.underrun = false;
        let worker = slot.worker;
        let request = SlotRequest::Start {
            slot: index,
            region,
            bytes_per_frame,
        };
        if self.requests[worker].push(request).is_err() {
            // The worker never learned about this slot, so nothing owns
            // it but us: take it straight back.
            self.free.push(index);
            self.counters.denials.fetch_add(1, Ordering::Relaxed);
            return NO_SLOT;
        }
        self.slots[index as usize].state = SlotState::Active;
        index
    }

    /// Hand a slot back. It becomes reusable only once the worker acks
    /// (see [`StreamRt::reclaim`]) — until then the worker still owns
    /// the producing end.
    pub(crate) fn release(&mut self, index: u16) {
        let Some(slot) = self.slots.get_mut(index as usize) else {
            return;
        };
        if slot.state != SlotState::Active {
            return;
        }
        slot.state = SlotState::Stopping;
        let worker = slot.worker;
        // A failed push leaves the slot parked rather than risking two
        // owners; it comes back with the next organ load. The queue
        // holds four requests per slot, so it cannot actually fill.
        let _ = self.requests[worker].push(SlotRequest::Stop { slot: index });
    }

    /// The window covering `frames` frames from `first`, refilling from
    /// the ring when the window has run past. `None` = no data (the
    /// caller clamps and fades).
    #[inline]
    pub(crate) fn window(
        &mut self,
        index: u16,
        first: i64,
        frames: usize,
    ) -> Option<(StreamWindow<'_>, u64, usize)> {
        let slot = self.slots.get_mut(index as usize)?;
        if slot.state != SlotState::Active {
            return None;
        }
        let first = first.max(0) as u64;
        let have = slot.window_start + slot.window_frames as u64;
        if first < slot.window_start || first + frames as u64 > have {
            slot.refill(first, frames);
        }
        if slot.window_frames == 0 {
            return None;
        }
        let count = slot.window_frames * slot.channels;
        let start = slot.window_start;
        let window = match slot.format {
            SampleFormat::F32 => StreamWindow::F32(&slot.window[..count]),
            // SAFETY: `window` is f32-backed (align 4 ≥ align 2) and
            // `count` i16 values fit in `count/2` f32 slots, which the
            // window always has; the bytes were written as native i16.
            SampleFormat::I16 => StreamWindow::I16(unsafe {
                std::slice::from_raw_parts(slot.window.as_ptr() as *const i16, count)
            }),
        };
        Some((window, start, slot.window_frames))
    }

    /// Did this slot run dry (as opposed to reaching the end of its
    /// region)? Cleared by the caller's fade.
    #[inline]
    pub(crate) fn underrun(&self, index: u16) -> bool {
        self.slots
            .get(index as usize)
            .is_some_and(|slot| slot.underrun)
    }

    #[inline]
    pub(crate) fn note_underrun(&self, index: u16) {
        let _ = index;
        self.counters.underruns.fetch_add(1, Ordering::Relaxed);
    }
}

impl Slot {
    /// Slide the window to cover `first..first + frames`, keeping
    /// whatever it already holds and topping up from the ring. Pure
    /// memory work: a memmove and one or two memcpys out of the ring.
    #[inline]
    fn refill(&mut self, first: u64, frames: usize) {
        let element = self.format.bytes();
        let frame_bytes = self.channels * element;
        let capacity = (self.window.len() * 4) / frame_bytes;
        // Bytes of the window as a mutable byte view (POD, native
        // endianness — the same idiom the sample cache uses).
        // SAFETY: f32 storage is 4-byte aligned and `capacity` frames
        // of `frame_bytes` fit inside it by construction.
        let bytes: &mut [u8] = unsafe {
            std::slice::from_raw_parts_mut(
                self.window.as_mut_ptr() as *mut u8,
                self.window.len() * 4,
            )
        };

        // Drop frames the reader has already passed. A reader that went
        // *backwards* past what the ring has already delivered cannot be
        // served at all (a ring does not rewind): starve it, and the
        // voice fades. Voices only ever move forward, so this is a
        // guard, not a path.
        if first < self.window_start {
            self.window_frames = 0;
            if first < self.next_frame {
                self.underrun = true;
                return;
            }
            self.window_start = first;
        } else {
            let drop_frames = (first - self.window_start) as usize;
            if drop_frames >= self.window_frames {
                self.window_frames = 0;
                self.window_start = first;
            } else if drop_frames > 0 {
                let keep = self.window_frames - drop_frames;
                bytes.copy_within(
                    drop_frames * frame_bytes..(drop_frames + keep) * frame_bytes,
                    0,
                );
                self.window_frames = keep;
                self.window_start = first;
            }
        }

        // The ring only ever delivers forward. If the reader is ahead of
        // the ring's cursor, discard the gap (never happens in practice:
        // a voice enters its region at the region's first frame).
        let window_end = self.window_start + self.window_frames as u64;
        if self.next_frame < window_end {
            let skip = ((window_end - self.next_frame) as usize * frame_bytes)
                .min(self.ring.slots());
            if skip > 0 && let Ok(chunk) = self.ring.read_chunk(skip) {
                chunk.commit_all();
                self.next_frame += (skip / frame_bytes) as u64;
            }
        }

        // Append only while the ring's cursor lines up with the window's
        // end — that alignment is what makes the window's frame labels
        // true.
        let window_end = self.window_start + self.window_frames as u64;
        let want_frames = (capacity - self.window_frames)
            .min((self.last_frame + 1).saturating_sub(window_end) as usize);
        let available = self.ring.slots() / frame_bytes;
        let take = want_frames.min(available);
        if self.next_frame == window_end
            && take > 0
            && let Ok(chunk) = self.ring.read_chunk(take * frame_bytes)
        {
            let (first_half, second_half) = chunk.as_slices();
            let base = self.window_frames * frame_bytes;
            bytes[base..base + first_half.len()].copy_from_slice(first_half);
            let base = base + first_half.len();
            bytes[base..base + second_half.len()].copy_from_slice(second_half);
            chunk.commit_all();
            self.window_frames += take;
            self.next_frame += take as u64;
        }

        // Short of what the reader asked for, with region data still to
        // come: the disk lost the race.
        let have = self.window_start + self.window_frames as u64;
        if have < (first + frames as u64).min(self.last_frame + 1) {
            self.underrun = true;
        }
    }
}

impl StreamWorker {
    /// One pass: apply the requests that arrived, then top every active
    /// ring up. Returns true when it moved bytes (the caller then polls
    /// again immediately instead of sleeping).
    pub fn poll_once(&mut self) -> bool {
        while let Ok(request) = self.requests.pop() {
            match request {
                SlotRequest::Start {
                    slot,
                    region,
                    bytes_per_frame,
                } => {
                    if let Some(state) = self.slots.get_mut(slot as usize / self.stride) {
                        state.store = region.store;
                        state.offset = region.offset;
                        state.remaining = region.frames * u64::from(bytes_per_frame);
                        state.active = true;
                    }
                }
                SlotRequest::Stop { slot } => {
                    let local = slot as usize / self.stride;
                    if let Some(state) = self.slots.get_mut(local) {
                        state.active = false;
                        state.remaining = 0;
                    }
                    // Ack: the audio thread may reuse the slot now.
                    let _ = self.returns.push(slot);
                }
            }
        }
        let mut moved = false;
        for index in 0..self.slots.len() {
            moved |= self.fill_slot(index);
        }
        moved
    }

    fn fill_slot(&mut self, index: usize) -> bool {
        let slot = &mut self.slots[index];
        if !slot.active || slot.remaining == 0 {
            return false;
        }
        let free = slot.ring.slots();
        if free < 4096 {
            return false; // not worth a syscall yet
        }
        let want = free.min(FILL_BYTES).min(slot.remaining as usize);
        let buffer = &mut self.scratch[..want];
        if self.stores.read_exact_at(slot.store, slot.offset, buffer).is_err() {
            // A vanished or truncated store: stop feeding this slot and
            // let the voice fade on the underrun path.
            slot.active = false;
            slot.remaining = 0;
            return false;
        }
        let Ok(mut chunk) = slot.ring.write_chunk(want) else {
            return false;
        };
        let (first_half, second_half) = chunk.as_mut_slices();
        first_half.copy_from_slice(&buffer[..first_half.len()]);
        second_half.copy_from_slice(&buffer[first_half.len()..]);
        chunk.commit_all();
        slot.offset += want as u64;
        slot.remaining -= want as u64;
        true
    }

    /// Run until the pool is torn down.
    pub fn run(mut self) {
        while !self.stop.load(Ordering::Relaxed) {
            if !self.poll_once() {
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
        }
    }
}

/// Live streamer threads. Dropping this stops and joins them, which is
/// what makes an organ swap safe: the engine (and its slots) is dropped
/// first, then the threads that were feeding it.
pub struct StreamThreads {
    stop: Arc<AtomicBool>,
    handles: Vec<std::thread::JoinHandle<()>>,
}

impl Drop for StreamThreads {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        for handle in self.handles.drain(..) {
            let _ = handle.join();
        }
    }
}

/// Put the workers on threads.
pub fn spawn(workers: Vec<StreamWorker>) -> StreamThreads {
    let stop = workers
        .first()
        .map(|worker| Arc::clone(&worker.stop))
        .unwrap_or_else(|| Arc::new(AtomicBool::new(false)));
    let handles = workers
        .into_iter()
        .enumerate()
        .map(|(index, worker)| {
            std::thread::Builder::new()
                .name(format!("aristide-stream-{index}"))
                .spawn(move || worker.run())
                .expect("streamer thread")
        })
        .collect();
    StreamThreads { stop, handles }
}
