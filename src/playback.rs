#[cfg(target_os = "linux")]
mod alsa;
#[cfg(target_os = "linux")]
pub use alsa::{playback, AlsaSample as BackendSample};

#[cfg(target_os = "windows")]
mod wasapi;
#[cfg(target_os = "windows")]
pub use wasapi::playback;

#[cfg(target_os = "windows")]
pub trait BackendSample: Sample {}
#[cfg(target_os = "windows")]
impl<T: Sample> BackendSample for T {}

use byteorder::{ByteOrder, LittleEndian};
use kanal::Receiver;
use std::time::{Duration, Instant};

/// Playback channel count assumed by the wire protocol.
pub const CHANNELS: usize = 2;

/// Capacity of the receiver queue, in samples.
pub const QUEUE_CAPACITY: usize = 16384;

/// At most one drift correction this often, so a burst of loss cannot turn
/// into a frenzy of splices.
const MIN_CORRECTION_INTERVAL: Duration = Duration::from_millis(100);

/// How far the queue may sit from its target before correcting (10 ms).
fn band(rate: usize) -> usize {
    (10 * rate / 1000).max(1) * CHANNELS
}

/// Holds the receiver queue near a target depth by dropping or repeating a
/// single interleaved frame at a time. No resampling, so the correction is
/// invisible in the steady state and happens at most once per
/// `MIN_CORRECTION_INTERVAL`.
pub struct Jitter<S: Sample> {
    held: [S; CHANNELS],
    pending: Option<[S; CHANNELS]>,
    target: usize,
    band: usize,
    last_correction: Instant,
    started: bool,
    pub corrections: u64,
}

impl<S: Sample> Jitter<S> {
    pub fn new(rate: usize, latency_ms: u64) -> Self {
        let frames = (latency_ms as usize * rate / 1000).max(1);
        let target = (frames * CHANNELS).clamp(CHANNELS, (QUEUE_CAPACITY / 2).max(CHANNELS));

        Jitter {
            held: [S::zero(); CHANNELS],
            pending: None,
            target,
            band: band(rate),
            // Allow the first correction immediately
            last_correction: Instant::now()
                .checked_sub(MIN_CORRECTION_INTERVAL)
                .unwrap_or_else(Instant::now),
            started: false,
            corrections: 0,
        }
    }

    /// Before playback starts the queue is filled by blocking reads; after it
    /// starts, an empty queue is filled with silence instead.
    pub fn set_started(&mut self, started: bool) {
        self.started = started;
    }

    fn recv_frame(receiver: &Receiver<S>) -> anyhow::Result<[S; CHANNELS]> {
        let mut frame = [S::zero(); CHANNELS];
        for sample in frame.iter_mut() {
            *sample = receiver.recv()?;
        }
        Ok(frame)
    }

    /// Produce the next interleaved frame.
    pub fn next_frame(&mut self, receiver: &Receiver<S>) -> anyhow::Result<[S; CHANNELS]> {
        // Finish an insertion by emitting the frame that was held back.
        if let Some(pending) = self.pending.take() {
            self.held = pending;
            return Ok(pending);
        }

        let queued = receiver.len();
        let can_correct = self.started && self.last_correction.elapsed() >= MIN_CORRECTION_INTERVAL;

        if can_correct && queued > self.target + self.band && queued >= 2 * CHANNELS {
            // Too much latency: consume two frames and emit their blend, so
            // one frame is removed with a one-frame crossfade.
            let first = Self::recv_frame(receiver)?;
            let second = Self::recv_frame(receiver)?;
            let blended = [S::blend(first[0], second[0]), S::blend(first[1], second[1])];

            self.last_correction = Instant::now();
            self.corrections = self.corrections.wrapping_add(1);
            self.held = blended;
            return Ok(blended);
        } else if can_correct && queued + self.band < self.target && queued >= CHANNELS {
            // Too little latency: emit one extra frame by blending it with
            // the next queued frame, and emit that frame next time.
            let next = Self::recv_frame(receiver)?;
            let blended = [S::blend(self.held[0], next[0]), S::blend(self.held[1], next[1])];

            self.last_correction = Instant::now();
            self.corrections = self.corrections.wrapping_add(1);
            self.pending = Some(next);
            self.held = blended;
            return Ok(blended);
        }

        if self.started && queued < CHANNELS {
            // Underrun. Whole frames of silence, so channels cannot swap.
            return Ok([S::zero(); CHANNELS]);
        }

        let frame = Self::recv_frame(receiver)?;
        self.held = frame;
        Ok(frame)
    }
}

/// A PCM sample format that can be carried over the wire and played natively.
#[allow(dead_code)] // BITS and write_le are only needed by the WASAPI backend
pub trait Sample: Copy + Send + 'static {
    const BITS: usize;
    const BYTES: usize;

    fn zero() -> Self;
    fn read_le(chunk: &[u8]) -> Self;
    fn write_le(self, dst: &mut [u8]);

    /// Midpoint of two samples, used for the one-frame splice crossfade.
    fn blend(a: Self, b: Self) -> Self;
}

impl Sample for i16 {
    const BITS: usize = 16;
    const BYTES: usize = 2;

    fn zero() -> Self {
        0
    }

    fn read_le(chunk: &[u8]) -> Self {
        LittleEndian::read_i16(chunk)
    }

    fn write_le(self, dst: &mut [u8]) {
        LittleEndian::write_i16(dst, self)
    }

    fn blend(a: Self, b: Self) -> Self {
        ((a as i32 + b as i32) / 2) as i16
    }
}

impl Sample for i32 {
    const BITS: usize = 32;
    const BYTES: usize = 4;

    fn zero() -> Self {
        0
    }

    fn read_le(chunk: &[u8]) -> Self {
        LittleEndian::read_i32(chunk)
    }

    fn write_le(self, dst: &mut [u8]) {
        LittleEndian::write_i32(dst, self)
    }

    fn blend(a: Self, b: Self) -> Self {
        ((a as i64 + b as i64) / 2) as i32
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kanal::bounded;

    #[test]
    fn repeats_a_frame_when_the_queue_is_shallow() {
        let (tx, rx) = bounded::<i32>(QUEUE_CAPACITY);
        let mut jitter = Jitter::new(48_000, 40);
        jitter.set_started(true);

        // Empty queue: underrun silence, and no correction yet.
        assert_eq!(jitter.next_frame(&rx).unwrap(), [0, 0]);

        tx.send(7).unwrap();
        tx.send(8).unwrap();

        // One frame queued against a 40 ms target: emit a blend with the held
        // frame, then emit the frame that was held back.
        assert_eq!(jitter.next_frame(&rx).unwrap(), [3, 4]);
        assert_eq!(jitter.corrections, 1);
        assert_eq!(jitter.next_frame(&rx).unwrap(), [7, 8]);
    }

    #[test]
    fn drops_a_frame_when_the_queue_is_deep() {
        let (tx, rx) = bounded::<i32>(QUEUE_CAPACITY);
        for i in 0..5000 {
            tx.send(i).unwrap();
        }

        let mut jitter = Jitter::new(48_000, 40);
        jitter.set_started(true);

        // Consumes [0, 1] and [2, 3], emits their blend, so one frame is gone.
        assert_eq!(jitter.next_frame(&rx).unwrap(), [1, 2]);
        assert_eq!(jitter.corrections, 1);
        assert_eq!(rx.len(), 5000 - 4);
        assert_eq!(jitter.next_frame(&rx).unwrap(), [4, 5]);
    }

    #[test]
    fn leaves_a_queue_near_target_alone() {
        let (tx, rx) = bounded::<i32>(QUEUE_CAPACITY);
        for i in 0..4000 {
            tx.send(i).unwrap();
        }

        let mut jitter = Jitter::new(48_000, 40);
        jitter.set_started(true);

        assert_eq!(jitter.next_frame(&rx).unwrap(), [0, 1]);
        assert_eq!(jitter.corrections, 0);
    }

    #[test]
    fn consumes_without_correcting_before_start() {
        let (tx, rx) = bounded::<i32>(QUEUE_CAPACITY);
        for i in 0..5000 {
            tx.send(i).unwrap();
        }

        let mut jitter = Jitter::new(48_000, 40);

        assert_eq!(jitter.next_frame(&rx).unwrap(), [0, 1]);
        assert_eq!(jitter.corrections, 0);
    }
}
