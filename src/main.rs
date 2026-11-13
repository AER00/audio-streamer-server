mod playback;

use byteorder::{ByteOrder, LittleEndian};
use kanal::bounded;
use std::io::ErrorKind;
use std::net::UdpSocket;
use std::str::from_utf8;
use std::thread;
use std::time::Duration;

const DATA_SILENCE: u8 = 0;
const DATA_CONFIG: u8 = 2;

#[derive(Clone)]
struct Config {
    bits: usize,
    frequency: usize,
    buffer: usize,
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

                let mut config = Config {
                    bits: 32,
                    frequency: 44100,
                    buffer: 2048,
                };

                let conf_str = from_utf8(&buf[1..size]).unwrap_or("");
                let conf: Vec<&str> = conf_str.trim().split(" ").collect();

                if conf.len() == 3 {
                    config.bits = conf[0].parse().unwrap_or(config.bits);
                    config.frequency = conf[1].parse().unwrap_or(config.frequency);
                    config.buffer = conf[2].parse().unwrap_or(config.buffer);
                }

                return Ok(config);
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

fn handle<S: playback::BackendSample>(
    socket: &UdpSocket,
    config: &Config,
) -> anyhow::Result<()> {
    let mut buf = vec![0u8; 16384];

    let rate = config.frequency;
    let buffer = config.buffer;

    let (sender, receiver) = bounded(16384);

    thread::spawn(move || {
        if let Err(e) = playback::playback(receiver, rate, buffer) {
            eprintln!("{}", e);
        }
    });

    socket.set_nonblocking(true)?;
    while !socket.recv(&mut *buf).is_err() {
        continue;
    }
    socket.set_nonblocking(false)?;

    loop {
        if sender.is_disconnected() {
            return Ok(());
        }

        // A read timeout only means the sender is idle; keep playback alive.
        let mut size = match socket.recv(&mut *buf) {
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

        if buf[0] == DATA_SILENCE {
            // sender.len() counts samples, buffer is in frames.
            if sender.len() > buffer * playback::CHANNELS {
                continue;
            }
            size = LittleEndian::read_u16(&buf[1..3]) as usize;

            let iters = (size - 1) / S::BYTES;
            for _ in 0..iters {
                sender.send(S::zero())?;
            }
            continue;
        }

        for chunk in buf[1..size].chunks_exact(S::BYTES) {
            sender.send(S::read_le(chunk))?;
        }
    }
}

fn main() -> anyhow::Result<()> {
    let socket = UdpSocket::bind("0.0.0.0:9032")?;

    let sleep_time = Duration::from_secs(1);
    let mut last_config: Option<Config> = None;

    loop {
        thread::sleep(sleep_time);
        println!("waiting...");

        let config = match read_config(&socket, last_config.as_ref()) {
            Ok(config) => config,
            Err(e) => {
                eprintln!("starting handler error: {}", e);
                continue;
            }
        };

        last_config = Some(config.clone());

        println!(
            "bit depth: {}; frequency: {} Hz; buffer: {} frames",
            config.bits, config.frequency, config.buffer
        );

        let result = if config.bits == 16 {
            handle::<i16>(&socket, &config)
        } else {
            handle::<i32>(&socket, &config)
        };

        if let Err(e) = result {
            eprintln!("handler error: {}", e);
        }
    }
}
