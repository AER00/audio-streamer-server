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

/// Playback channel count assumed by the wire protocol.
pub const CHANNELS: usize = 2;

/// A PCM sample format that can be carried over the wire and played natively.
#[allow(dead_code)] // BITS and write_le are only needed by the WASAPI backend
pub trait Sample: Copy + Send + 'static {
    const BITS: usize;
    const BYTES: usize;

    fn zero() -> Self;
    fn read_le(chunk: &[u8]) -> Self;
    fn write_le(self, dst: &mut [u8]);
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
}
