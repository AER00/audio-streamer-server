mod codec;
mod playback;

use byteorder::{ByteOrder, LittleEndian};
use kanal::{bounded, Sender};
use std::io::ErrorKind;
use std::net::UdpSocket;
use std::str::from_utf8;
use std::thread;
use std::time::{Duration, Instant};

const DATA_SILENCE: u8 = 0;
const DATA_CONFIG: u8 = 2;

/// v2 packets start with a tag byte and a u32 LE sequence number.
const HEADER_SIZE: usize = 5;
const STATS_INTERVAL: Duration = Duration::from_secs(5);

struct Options {
    latency_ms: u64,
}

impl Default for Options {
    fn default() -> Self {
        Options { latency_ms: 40 }
    }
}

fn parse_options() -> Options {
    let mut options = Options::default();
    let mut args = std::env::args().skip(1);

    while let Some(arg) = args.next() {
        if let Some(value) = arg.strip_prefix("--latency=") {
            options.latency_ms = value.parse().unwrap_or(options.latency_ms);
        } else if arg == "--latency" {
            if let Some(value) = args.next() {
                options.latency_ms = value.parse().unwrap_or(options.latency_ms);
            }
        }
    }

    options
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Codec {
    Pcm,
    Opus,
}

#[derive(Clone)]
struct Config {
    version: u8,
    bits: usize,
    frequency: usize,
    buffer: usize,
    channels: usize,
    codec: Codec,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            version: 1,
            bits: 32,
            frequency: 44100,
            buffer: 2048,
            channels: playback::CHANNELS,
            codec: Codec::Pcm,
        }
    }
}

fn parse_config(payload: &[u8]) -> Config {
    let mut config = Config::default();

    let conf_str = from_utf8(payload).unwrap_or("");
    let conf: Vec<&str> = conf_str.split_whitespace().collect();

    if conf.len() >= 5 {
        // v2: version bits rate buffer channels [codec]
        config.version = conf[0].parse().unwrap_or(2);
        config.bits = conf[1].parse().unwrap_or(config.bits);
        config.frequency = conf[2].parse().unwrap_or(config.frequency);
        config.buffer = conf[3].parse().unwrap_or(config.buffer);
        config.channels = conf[4].parse().unwrap_or(config.channels).max(1);
    } else if conf.len() == 3 {
        // v1: bits rate buffer
        config.bits = conf[0].parse().unwrap_or(config.bits);
        config.frequency = conf[1].parse().unwrap_or(config.frequency);
        config.buffer = conf[2].parse().unwrap_or(config.buffer);
    }

    if conf.len() >= 6 {
        config.codec = match conf[5].to_ascii_lowercase().as_str() {
            "opus" => Codec::Opus,
            _ => Codec::Pcm,
        };
    }

    config
}

/// Look for a config packet, ignoring anything else still in flight from a
/// previous session. Falls back to the previous config if none arrives, so a
/// client that only announces itself once can still reconnect.
fn read_config(socket: &UdpSocket, previous: Option<&Config>) -> anyhow::Result<Config> {
    let mut buf = [0u8; 256];
    socket.set_read_timeout(Some(Duration::from_millis(1000)))?;

    loop {
        match socket.recv(&mut buf) {
            Ok(size) => {
                if size < 2 || buf[0] != DATA_CONFIG {
                    continue;
                }

                return Ok(parse_config(&buf[1..size]));
            }
            Err(e) if e.kind() == ErrorKind::WouldBlock || e.kind() == ErrorKind::TimedOut => {
                if let Some(config) = previous {
                    return Ok(config.clone());
                }
            }
            Err(e) => return Err(e.into()),
        }
    }
}

/// Enqueue without blocking the UDP receive loop. A full queue drops samples
/// and is counted, instead of stalling receives and losing bursts in the
/// kernel socket buffer.
fn enqueue<S: playback::BackendSample>(
    sender: &Sender<S>,
    sample: S,
    dropped: &mut u64,
) -> anyhow::Result<()> {
    if !sender.try_send(sample)? {
        *dropped += 1;
    }
    Ok(())
}

fn handle<S: playback::BackendSample + From<i16>>(
    socket: &UdpSocket,
    config: &Config,
    latency_ms: u64,
    mut decoder: Option<&mut codec::Decoder>,
) -> anyhow::Result<()> {
    let mut buf = vec![0u8; 16384];

    let rate = config.frequency;
    let buffer = config.buffer;
    let channels = config.channels;
    let mut pcm = vec![0i16; codec::MAX_FRAME_SAMPLES * channels];

    // v1 packets have only the tag byte; v2 adds the sequence number.
    let header = if config.version >= 2 { HEADER_SIZE } else { 1 };
    let silence_header = header + 2;

    let (sender, receiver) = bounded(playback::QUEUE_CAPACITY);

    thread::spawn(move || {
        if let Err(e) = playback::playback(receiver, rate, buffer, latency_ms) {
            eprintln!("{}", e);
        }
    });

    socket.set_nonblocking(true)?;
    while !socket.recv(&mut *buf).is_err() {
        continue;
    }
    socket.set_nonblocking(false)?;

    let mut expected_seq: Option<u32> = None;
    let mut gaps: u64 = 0;
    let mut dropped: u64 = 0;
    let mut last_stats = Instant::now();

    loop {
        if sender.is_disconnected() {
            return Ok(());
        }

        if last_stats.elapsed() >= STATS_INTERVAL {
            if gaps != 0 || dropped != 0 {
                eprintln!(
                    "stats: gaps={} dropped={} queued={}",
                    gaps,
                    dropped,
                    sender.len()
                );
            }
            last_stats = Instant::now();
        }

        // A read timeout only means the sender is idle; keep playback alive.
        let size = match socket.recv(&mut *buf) {
            Ok(size) => size,
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
                continue;
            }
            Err(e) => return Err(e.into()),
        };

        if size == 0 {
            return Ok(());
        }

        if buf[0] == DATA_CONFIG {
            continue;
        }

        if size < header {
            continue;
        }

        let mut lost: u32 = 0;

        if config.version >= 2 {
            let seq = LittleEndian::read_u32(&buf[1..HEADER_SIZE]);
            if let Some(expected) = expected_seq {
                if seq != expected {
                    let gap = seq.wrapping_sub(expected);
                    // Ignore nonsense (reordered old packets, sender restarts).
                    if gap < 512 {
                        gaps += gap as u64;
                        lost = gap;
                    }
                }
            }
            expected_seq = Some(seq.wrapping_add(1));
        }

        // Opus can synthesise the missing frames instead of leaving a hole.
        if lost > 0 {
            if let Some(decoder) = decoder.as_deref_mut() {
                for _ in 0..lost.min(5) {
                    let samples = decoder.decode(None, &mut pcm)?;
                    let end = (samples * channels).min(pcm.len());
                    for &sample in &pcm[..end] {
                        enqueue(&sender, S::from(sample), &mut dropped)?;
                    }
                }
            }
        }

        if buf[0] == DATA_SILENCE {
            if size < silence_header {
                continue;
            }
            // sender.len() counts samples, buffer is in frames.
            if sender.len() > buffer * channels {
                continue;
            }

            let count = LittleEndian::read_u16(&buf[header..silence_header]) as usize;
            // v1 counted the tag byte; v2 stores the payload byte count.
            let iters = if config.version >= 2 {
                count / S::BYTES
            } else {
                count.saturating_sub(1) / S::BYTES
            };

            for _ in 0..iters {
                enqueue(&sender, S::zero(), &mut dropped)?;
            }
            continue;
        }

        if let Some(decoder) = decoder.as_deref_mut() {
            let samples = decoder.decode(Some(&buf[header..size]), &mut pcm)?;
            let end = (samples * channels).min(pcm.len());
            for &sample in &pcm[..end] {
                enqueue(&sender, S::from(sample), &mut dropped)?;
            }
            continue;
        }

        for chunk in buf[header..size].chunks_exact(S::BYTES) {
            enqueue(&sender, S::read_le(chunk), &mut dropped)?;
        }
    }
}

fn main() -> anyhow::Result<()> {
    let socket = UdpSocket::bind("0.0.0.0:9032")?;
    let options = parse_options();

    let sleep_time = Duration::from_secs(1);
    let mut last_config: Option<Config> = None;

    loop {
        thread::sleep(sleep_time);
        println!("waiting...");

        let mut config = match read_config(&socket, last_config.as_ref()) {
            Ok(config) => config,
            Err(e) => {
                eprintln!("starting handler error: {}", e);
                continue;
            }
        };

        // Opus is fixed-rate stereo 16-bit at the decoder output.
        if config.codec == Codec::Opus {
            config.bits = 16;
            config.frequency = codec::RATE as usize;
            config.channels = codec::CHANNELS;
        }

        last_config = Some(config.clone());

        let codec_name = match config.codec {
            Codec::Pcm => "pcm",
            Codec::Opus => "opus",
        };

        println!(
            "protocol v{}; codec: {}; bit depth: {}; frequency: {} Hz; buffer: {} frames; channels: {}; latency: {} ms",
            config.version,
            codec_name,
            config.bits,
            config.frequency,
            config.buffer,
            config.channels,
            options.latency_ms
        );

        let mut decoder = if config.codec == Codec::Opus {
            match codec::Decoder::new(config.channels) {
                Ok(decoder) => Some(decoder),
                Err(e) => {
                    eprintln!("cannot start opus playback: {}", e);
                    continue;
                }
            }
        } else {
            None
        };

        let result = if config.bits == 16 {
            handle::<i16>(&socket, &config, options.latency_ms, decoder.as_mut())
        } else {
            handle::<i32>(&socket, &config, options.latency_ms, decoder.as_mut())
        };

        if let Err(e) = result {
            eprintln!("handler error: {}", e);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_legacy_config() {
        let config = parse_config(b"16 48000 2048");
        assert_eq!(config.version, 1);
        assert_eq!(config.bits, 16);
        assert_eq!(config.frequency, 48_000);
        assert_eq!(config.buffer, 2048);
        assert_eq!(config.channels, playback::CHANNELS);
    }

    #[test]
    fn parses_versioned_config() {
        let config = parse_config(b"2 16 44100 1024 2");
        assert_eq!(config.version, 2);
        assert_eq!(config.bits, 16);
        assert_eq!(config.frequency, 44_100);
        assert_eq!(config.buffer, 1024);
        assert_eq!(config.channels, 2);
    }

    #[test]
    fn falls_back_to_defaults_on_garbage() {
        let config = parse_config(b"nonsense");
        assert_eq!(config.bits, 32);
        assert_eq!(config.frequency, 44_100);
    }

    #[test]
    fn rejects_zero_channels() {
        let config = parse_config(b"2 16 48000 2048 0");
        assert!(config.channels >= 1);
    }

    #[test]
    fn parses_codec() {
        assert_eq!(parse_config(b"2 16 48000 2048 2 pcm").codec, Codec::Pcm);
        assert_eq!(parse_config(b"3 16 48000 2048 2 opus").codec, Codec::Opus);
        assert_eq!(parse_config(b"2 16 48000 2048 2").codec, Codec::Pcm);
    }

    #[cfg(feature = "opus")]
    #[test]
    fn decodes_packets_and_conceals_loss() {
        let mut encoder =
            opus::Encoder::new(codec::RATE, opus::Channels::Stereo, opus::Application::Audio)
                .unwrap();
        let input = vec![1000i16; codec::FRAME_SAMPLES * codec::CHANNELS];
        let mut packet = vec![0u8; 4000];
        let n = encoder.encode(&input, &mut packet).unwrap();

        let mut decoder = codec::Decoder::new(codec::CHANNELS).unwrap();
        let mut out = vec![0i16; codec::MAX_FRAME_SAMPLES * codec::CHANNELS];

        assert_eq!(
            decoder.decode(Some(&packet[..n]), &mut out).unwrap(),
            codec::FRAME_SAMPLES
        );
        // One lost frame is concealed at the 20 ms frame size.
        assert_eq!(decoder.decode(None, &mut out).unwrap(), codec::FRAME_SAMPLES);
    }
}
