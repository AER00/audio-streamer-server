use alsa::pcm::{Access, Format, Frames, HwParams, PCM};
use alsa::{Direction, ValueOr};
use byteorder::{ByteOrder, LittleEndian};
use kanal::bounded;
use kanal::{Receiver, Sender};
use std::net::UdpSocket;
use std::str::from_utf8;
use std::thread;
use std::time::Duration;

const DATA_SILENCE: u8 = 0;
const DATA_CONFIG: u8 = 2;

struct Handler {
    buffer: usize,
    frequency: usize,
    sender: Sender<i32>,
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

        println!("sample rate: {format}; frequency: {frequency} Hz; buffer: {buffer} frames");

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

    fn handle(mut self, socket: &UdpSocket, receiver: Receiver<i32>) -> anyhow::Result<()> {
        let mut buf = vec![0u8; 16384];

        socket.set_read_timeout(Some(Duration::from_millis(1000)))?;

        let rate = self.frequency;
        let buffer = self.buffer;

        thread::spawn(move || {
            if let Err(e) = playback(receiver, rate, buffer) {
                eprintln!("{}", e);
            }
        });

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
                size = LittleEndian::read_u16(&buf[1..3]) as usize;
                let iters = (size - 1) / 4;
                for _ in 0..iters {
                    self.sender.send(0)?;
                }
                continue;
            }

            for chunk in buf[1..size].chunks_exact(4) {
                self.sender.send(LittleEndian::read_i32(chunk))?;
            }
        }
    }
}

impl Drop for Handler {
    fn drop(&mut self) {
        self.sender.close();
    }
}

fn playback(receiver: Receiver<i32>, rate: usize, buffer: usize) -> anyhow::Result<()> {
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
