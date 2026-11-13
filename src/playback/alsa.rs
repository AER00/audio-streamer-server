use crate::playback::{Sample, CHANNELS};
use alsa::pcm::{Access, Format, Frames, HwParams, IO, PCM};
use alsa::{Direction, ValueOr};
use kanal::Receiver;

/// A sample type that can be set up as a native ALSA playback format.
pub trait AlsaSample: Sample {
    fn format() -> Format;
    fn io(pcm: &PCM) -> alsa::Result<IO<'_, Self>>;
}

impl AlsaSample for i16 {
    fn format() -> Format {
        Format::s16()
    }

    fn io(pcm: &PCM) -> alsa::Result<IO<'_, i16>> {
        pcm.io_i16()
    }
}

impl AlsaSample for i32 {
    fn format() -> Format {
        Format::s32()
    }

    fn io(pcm: &PCM) -> alsa::Result<IO<'_, i32>> {
        pcm.io_i32()
    }
}

pub fn playback<S: AlsaSample>(
    receiver: Receiver<S>,
    rate: usize,
    buffer: usize,
) -> anyhow::Result<()> {
    let pcm = PCM::new("default", Direction::Playback, false)?;

    let hwp = HwParams::any(&pcm)?;
    hwp.set_channels(CHANNELS as u32)?;
    hwp.set_rate(rate as u32, ValueOr::Nearest)?;
    hwp.set_format(S::format())?;
    hwp.set_access(Access::RWInterleaved)?;
    hwp.set_buffer_size_near(Frames::from(buffer as i32))?;
    pcm.hw_params(&hwp)?;
    let io = S::io(&pcm)?;

    let hwp = pcm.hw_params_current()?;
    let swp = pcm.sw_params_current()?;

    let threshold = hwp.get_buffer_size()?;
    swp.set_start_threshold(threshold)?;
    pcm.sw_params(&swp)?;

    const PLAYBACK_SIZE: usize = 8;
    let mut playback = [S::zero(); PLAYBACK_SIZE];

    let buffer = threshold as usize;
    let mut filled = false;
    let mut started = false;
    let mut written = 0;

    loop {
        if receiver.is_terminated() {
            if started {
                pcm.drain()?;
            }
            return Ok(());
        }

        for sample in playback.iter_mut() {
            if started && (receiver.is_empty() || filled) {
                *sample = S::zero();
                // we want to write samples in pairs not to reverse the channels
                filled = !filled;
            } else {
                *sample = receiver.recv()?;
            }
        }

        io.writei(&playback).or_else(|e| -> alsa::Result<usize> {
            // snd_pcm_recover handles EPIPE (prepare) and ESTRPIPE (resume).
            eprintln!("XRUN recovered: {}", e);
            pcm.try_recover(e, true)?;
            started = false;
            written = 0;
            filled = false;
            Ok(0)
        })?;

        if !started {
            written += PLAYBACK_SIZE;
            started = written >= buffer;
        }
    }
}
