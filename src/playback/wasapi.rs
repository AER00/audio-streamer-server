use crate::playback::{SampleFormat, SAMPLE_BYTES};
use byteorder::ByteOrder;
use byteorder::LittleEndian;
use kanal::Receiver;
use wasapi::*;

pub fn playback(
    receiver: Receiver<SampleFormat>,
    rate: usize,
    buffer: usize,
) -> anyhow::Result<()> {
    initialize_mta().unwrap();

    let channels = 2;
    let device = get_default_device(&Direction::Render).unwrap();
    let mut audio_client = device.get_iaudioclient().unwrap();
    let desired_format = WaveFormat::new(32, 32, &SampleType::Int, rate, channels, None);

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

    let (def_time, min_time) = audio_client.get_periods().unwrap();
    println!("default period {}, min period {}", def_time, min_time);

    audio_client
        .initialize_client(
            &desired_format,
            def_time as i64,
            &Direction::Render,
            &ShareMode::Shared,
            true,
        )
        .unwrap();
    println!("initialized playback");

    let h_event = audio_client.set_get_eventhandle().unwrap();

    let render_client = audio_client.get_audiorenderclient().unwrap();

    let mut buf = vec![0u8; 16384];

    println!("started");

    audio_client.start_stream().unwrap();

    let mut flipped = false;

    loop {
        if receiver.is_terminated() {
            return Ok(());
        }

        let buffer_frame_count = audio_client.get_available_space_in_frames().unwrap();

        let size = buffer_frame_count as usize * blockalign as usize;

        for samples in buf[..size].chunks_exact_mut(SAMPLE_BYTES) {
            if receiver.is_empty() || flipped {
                flipped = !flipped;
                samples.iter_mut().for_each(|x| *x = 0);
            } else {
                LittleEndian::write_i32(samples, receiver.recv()?);
            }
        }

        render_client
            .write_to_device(
                buffer_frame_count as usize,
                blockalign as usize,
                &buf[..size],
                None,
            )
            .unwrap();

        if h_event.wait_for_event(2000).is_err() {
            eprintln!("error, stopping playback");
            audio_client.stop_stream().unwrap();
            break;
        }
    }
    Ok(())
}
