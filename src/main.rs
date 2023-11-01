use std::io;
use std::io::Write;
use std::net::UdpSocket;
use std::process::{Command, Stdio};
use std::str::from_utf8;
use std::time::{Duration, Instant};
use byteorder::{ByteOrder, LittleEndian};

const DATA_SILENCE: u8 = 0;
// const DATA_SOUND: u8 = 1;
const DATA_CONFIG: u8 = 2;
// const DATA_FILLED: u8 = 3;

struct Handler {
    process: std::process::Child,
    buffer: usize,
    frequency: usize,
    fill: bool
}

fn optimal_read_timeout(chunk_size: usize, frequency: usize) -> Duration {
    let ms = ((chunk_size as f64 * 1000.0 / 8.0) / frequency as f64).ceil() as u64;
    Duration::from_millis(ms)
}

impl Handler {
    fn new(socket: &UdpSocket) -> anyhow::Result<Handler> {
        let mut format = "S32_LE";
        let mut rate = "44100";
        let mut buf_size = "1888";
        let mut fill = true;

        let mut buf = vec![0u8; 256];
        socket.set_read_timeout(None)?;

        let size = socket.recv(&mut *buf)?;

        if buf[0] == DATA_CONFIG {
            let conf_str = from_utf8(&buf[1..size]).unwrap_or("");
            let conf: Vec<&str> = conf_str.trim().split(" ").collect();

            if conf.len() >= 3 {
                println!("{:?}", conf);
                format = conf[0];
                rate = conf[1];
                buf_size = conf[2];
                if conf.len() == 4 && conf[3] == "false" {
                    fill = false;
                }
            }
        }

        let buffer: usize = buf_size.parse()?;
        let frequency: usize = rate.parse()?;

        let process = Command::new("aplay")
            .arg("--buffer-size")
            .arg(buf_size)
            .arg("-r")
            .arg(rate)
            .arg("-f")
            .arg(format)
            .arg("-c")
            .arg("2")
            .stdin(Stdio::piped())
            .spawn()?;

        Ok(Handler {
            process,
            buffer,
            frequency,
            fill,
        })
    }

    fn handle(mut self, socket: &UdpSocket) -> io::Result<()> {
        let mut buf = vec![0u8; 16384];

        let mut stdin = self.process.stdin.take().unwrap();

        // fill buffer with silence
        buf[..self.buffer].iter_mut().for_each(|x| *x = 0);
        stdin.write_all(&buf[..self.buffer])?;

        socket.set_read_timeout(Some(Duration::from_millis(1000)))?;

        // maximum no packet timeout
        let mut max_silence = Duration::from_millis(1500);

        if self.fill {
            max_silence = Duration::from_millis(1000);
            socket.set_read_timeout(Some(optimal_read_timeout(self.buffer, self.frequency)))?;
        }

        let zeroes = vec![0u8; self.buffer];

        let mut last = Instant::now();

        loop {
            let mut size = socket.recv(&mut *buf).unwrap_or(0);
            let duration = last.elapsed();

            if duration > max_silence {
                return Ok(());
            }

            if size > 0 {
                last = Instant::now();
            } else if self.fill {
                // stdin.write_all(&buf[1..(self.buffer+1)])?;
                stdin.write_all(&zeroes)?;
                continue;
            } else {
                return Ok(());
            }

            if buf[0] == DATA_CONFIG {
                continue;
            }

            if buf[0] == DATA_SILENCE {
                size = LittleEndian::read_u16(&buf[1..3]) as usize;
                buf[1..size].iter_mut().for_each(|x| *x = 0);
            }

            // // set read timeout to optimal time based on regular chunk size
            // // (will never execute when fill is set to false)
            // if !optimal_timeout && buf[0] != DATA_FILLED {
            //     chunk_size = size - 1;
            //     socket.set_read_timeout(Some(optimal_read_timeout(self.buffer, self.frequency)))?;
            //     optimal_timeout = true;
            // }

            stdin.write_all(&buf[1..size])?;
        }
    }
}

impl Drop for Handler {
    fn drop(&mut self) {
        let _ = self.process.wait();
    }
}

fn main() -> io::Result<()> {
    let socket = UdpSocket::bind("0.0.0.0:9032")?;

    let sleep_time = Duration::from_secs(1);

    loop {
        std::thread::sleep(sleep_time);
        println!("waiting...");

        let handler = match Handler::new(&socket) {
            Ok(handler) => handler,
            Err(e) => {
                println!("starting handler error: {}", e);
                continue;
            }
        };

        if let Err(e) = handler.handle(&socket) {
            println!("handler error: {}", e);
        }
    }
}
