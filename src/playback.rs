#[cfg(target_os = "linux")]
mod alsa;
#[cfg(target_os = "linux")]
pub use alsa::playback;

#[cfg(target_os = "windows")]
mod wasapi;
#[cfg(target_os = "windows")]
pub use wasapi::playback;

pub type SampleFormat = i32;
pub const SAMPLE_BYTES: usize = 4;
