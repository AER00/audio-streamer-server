use crate::playback::{Jitter, Sample, CHANNELS};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{BufferSize, SampleRate, StreamConfig};
use kanal::Receiver;
use std::time::Duration;

pub fn playback<S: Sample>(
    receiver: Receiver<S>,
    rate: usize,
    _buffer: usize,
    latency_ms: u64,
) -> anyhow::Result<()> {
    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or_else(|| anyhow::anyhow!("no default output device"))?;

    let rate = if rate == 0 { 44_100 } else { rate };
    let config = StreamConfig {
        channels: CHANNELS as u16,
        sample_rate: SampleRate(rate as u32),
        buffer_size: BufferSize::Default,
    };

    let mut jitter = Jitter::new(rate, latency_ms);
    jitter.set_started(true);

    // Keep a handle so this thread can exit when the handler drops the sender.
    let terminated = receiver.clone();

    let stream = device.build_output_stream(
        &config,
        move |data: &mut [f32], _| {
            for frame in data.chunks_exact_mut(CHANNELS) {
                let samples = jitter
                    .next_frame(&receiver)
                    .unwrap_or([S::zero(); CHANNELS]);

                for (slot, sample) in frame.iter_mut().zip(samples.iter()) {
                    *slot = (*sample).to_f32();
                }
            }
        },
        |err| eprintln!("playback error: {}", err),
        None,
    )?;

    stream.play()?;

    while !terminated.is_terminated() {
        std::thread::sleep(Duration::from_millis(100));
    }

    Ok(())
}
