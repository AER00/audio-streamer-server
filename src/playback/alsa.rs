use crate::playback::SampleFormat;
use alsa::pcm::{Access, Format, Frames, HwParams, PCM};
use alsa::{Direction, ValueOr};
use kanal::Receiver;

pub fn playback(
    receiver: Receiver<SampleFormat>,
    rate: usize,
    buffer: usize,
) -> anyhow::Result<()> {
    let pcm = PCM::new("default", Direction::Playback, false)?;

    let hwp = HwParams::any(&pcm)?;
    hwp.set_channels(2)?;
    hwp.set_rate(rate as u32, ValueOr::Nearest)?;
    hwp.set_format(Format::s32())?;
    hwp.set_access(Access::RWInterleaved)?;
    hwp.set_buffer_size_near(Frames::from(buffer as i32))?;
    pcm.hw_params(&hwp)?;
    let io = pcm.io_i32()?;

    let hwp = pcm.hw_params_current()?;
    let swp = pcm.sw_params_current()?;

    let threshold = hwp.get_buffer_size()?;
    swp.set_start_threshold(threshold)?;
    pcm.sw_params(&swp)?;

    const PLAYBACK_SIZE: usize = 8;
    let mut playback = [0i32; PLAYBACK_SIZE];

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
                *sample = 0;
                // we want to write samples in pairs not to reverse the channels
                filled = !filled;
            } else {
                *sample = receiver.recv()?;
            }
        }

        io.writei(&playback)?;

        if !started {
            written += PLAYBACK_SIZE;
            started = written >= buffer;
        }
    }
}
