use crate::playback::{Jitter, Sample, CHANNELS};
use kanal::Receiver;
use wasapi::*;

/// WASAPI errors are not `Send + Sync`, so they can't `?` into anyhow directly.
fn checked<T, E: std::fmt::Display>(result: Result<T, E>) -> anyhow::Result<T> {
    result.map_err(|e| anyhow::anyhow!("{}", e))
}

pub fn playback<S: Sample>(
    receiver: Receiver<S>,
    rate: usize,
    _buffer: usize,
    latency_ms: u64,
) -> anyhow::Result<()> {
    checked(initialize_mta())?;

    let channels = CHANNELS;
    let device = checked(get_default_device(&Direction::Render))?;
    let mut audio_client = checked(device.get_iaudioclient())?;
    let desired_format = WaveFormat::new(S::BITS, S::BITS, &SampleType::Int, rate, channels, None);

    // Check if the desired format is supported.
    // Since we have convert = true in the initialize_client call later,
    // it's ok to run with an unsupported format.
    match audio_client.is_supported(&desired_format, &ShareMode::Shared) {
        Ok(None) => {
            println!("Device supports format {:?}", desired_format);
        }
        Ok(Some(modified)) => {
            println!(
                "Device doesn't support format:\n{:#?}\nClosest match is:\n{:#?}",
                desired_format, modified
            )
        }
        Err(err) => {
            println!(
                "Device doesn't support format:\n{:#?}\nError: {}",
                desired_format, err
            );
        }
    }

    // Blockalign is the number of bytes per frame
    let blockalign = desired_format.get_blockalign();
    println!("Desired playback format: {:?}", desired_format);

    let (def_time, min_time) = checked(audio_client.get_periods())?;
    println!("default period {}, min period {}", def_time, min_time);

    checked(audio_client.initialize_client(
        &desired_format,
        def_time as i64,
        &Direction::Render,
        &ShareMode::Shared,
        true,
    ))?;
    println!("initialized playback");

    let h_event = checked(audio_client.set_get_eventhandle())?;

    let render_client = checked(audio_client.get_audiorenderclient())?;

    // The device buffer can be much larger than the old fixed 16 KiB.
    let max_frames = checked(audio_client.get_bufferframecount())? as usize;
    let mut buf = vec![0u8; max_frames * blockalign as usize];

    println!("started");

    checked(audio_client.start_stream())?;

    let mut jitter = Jitter::new(rate, latency_ms);
    jitter.set_started(true);

    loop {
        if receiver.is_terminated() {
            return Ok(());
        }

        let buffer_frame_count = checked(audio_client.get_available_space_in_frames())?;

        let size = (buffer_frame_count as usize * blockalign as usize).min(buf.len());

        for frame in buf[..size].chunks_exact_mut(blockalign as usize) {
            let samples = jitter.next_frame(&receiver)?;
            for (i, sample) in samples.iter().enumerate() {
                (*sample).write_le(&mut frame[i * S::BYTES..(i + 1) * S::BYTES]);
            }
        }

        checked(render_client.write_to_device(
            buffer_frame_count as usize,
            blockalign as usize,
            &buf[..size],
            None,
        ))?;

        if h_event.wait_for_event(2000).is_err() {
            eprintln!("error, stopping playback");
            let _ = audio_client.stop_stream();
            break;
        }
    }
    Ok(())
}
