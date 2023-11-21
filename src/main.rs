mod playback;

use byteorder::{ByteOrder, LittleEndian};
use kanal::bounded;
use kanal::{Receiver, Sender};
use playback::{SampleFormat, SAMPLE_BYTES};
use std::net::UdpSocket;
use std::str::from_utf8;
use std::thread;
use std::time::Duration;

const DATA_SILENCE: u8 = 0;
const DATA_CONFIG: u8 = 2;

struct Handler {
    buffer: usize,
    frequency: usize,
    sender: Sender<SampleFormat>,
}

impl Handler {
    fn new(socket: &UdpSocket) -> anyhow::Result<(Handler, Receiver<i32>)> {
        let mut format = 32;
        let mut frequency = 44100;
        let mut buffer = 2048;

        let mut buf = vec![0u8; 256];
        socket.set_read_timeout(None)?;

        let size = socket.recv(&mut *buf)?;

        if buf[0] == DATA_CONFIG {
            let conf_str = from_utf8(&buf[1..size]).unwrap_or("");
            let conf: Vec<&str> = conf_str.trim().split(" ").collect();

            if conf.len() == 3 {
                format = conf[0].parse().unwrap_or(format);
                frequency = conf[1].parse().unwrap_or(frequency);
                buffer = conf[2].parse().unwrap_or(buffer);
            }
        }

        println!("bit depth: {format}; frequency: {frequency} Hz; buffer: {buffer} frames");

        let (sender, receiver) = bounded(16384);

        Ok((
            Handler {
                buffer,
                frequency,
                sender,
            },
            receiver,
        ))
    }

    fn handle(self, socket: &UdpSocket, receiver: Receiver<i32>) -> anyhow::Result<()> {
        let mut buf = vec![0u8; 16384];

        socket.set_read_timeout(Some(Duration::from_millis(1000)))?;

        let rate = self.frequency;
        let buffer = self.buffer;

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
            if self.sender.is_disconnected() {
                return Ok(());
            }

            let mut size = socket.recv(&mut *buf)?;

            if size == 0 {
                return Ok(());
            }

            if buf[0] == DATA_CONFIG {
                continue;
            }

            if buf[0] == DATA_SILENCE {
                if self.sender.len() > self.buffer {
                    continue;
                }
                size = LittleEndian::read_u16(&buf[1..3]) as usize;

                let iters = (size - 1) / SAMPLE_BYTES;
                for _ in 0..iters {
                    self.sender.send(0)?;
                }
                continue;
            }

            for chunk in buf[1..size].chunks_exact(SAMPLE_BYTES) {
                self.sender.send(LittleEndian::read_i32(chunk))?;
            }
        }
    }
}

fn main() -> anyhow::Result<()> {
    let socket = UdpSocket::bind("0.0.0.0:9032")?;

    let sleep_time = Duration::from_secs(1);

    loop {
        thread::sleep(sleep_time);
        println!("waiting...");

        let (handler, receiver) = match Handler::new(&socket) {
            Ok((h, r)) => (h, r),
            Err(e) => {
                eprintln!("starting handler error: {}", e);
                continue;
            }
        };

        if let Err(e) = handler.handle(&socket, receiver) {
            eprintln!("handler error: {}", e);
        }
    }
}
