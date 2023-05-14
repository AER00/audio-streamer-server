use std::io;
use std::io::Write;
use std::net::UdpSocket;
use std::process::{Command, Stdio};
use std::str::from_utf8;
use std::time::Duration;

struct Handler {
    process: std::process::Child,
}

impl Handler {
    fn new(socket: &UdpSocket) -> anyhow::Result<Handler> {
        let mut format = "S32_LE";
        let mut rate = "44100";
        let mut buf_size = "1888";

        let mut buf = vec![0u8; 256];
        socket.set_read_timeout(None)?;

        let size = socket.recv(&mut *buf)?;

        let conf_str = from_utf8(&buf[..size]).unwrap_or("");
        let conf: Vec<&str> = conf_str.trim().split(" ").collect();

        if conf.len() == 3 {
            println!("{:?}", conf);
            format = conf[0];
            rate = conf[1];
            buf_size = conf[2];
        }

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
            process
        })
    }

    fn handle(mut self, socket: &UdpSocket) -> io::Result<()> {
        let mut buf = vec![0u8; 16384];

        socket.set_read_timeout(Some(Duration::from_secs(1)))?;

        let mut stdin = self.process.stdin.take().unwrap();

        loop {
            let size = socket.recv(&mut *buf)?;
            if size == 0 {
                return Ok(())
            }

            if size <= 32 {
                if let Ok(text) = from_utf8(&buf[..size]) {
                    if text.trim().split(" ").collect::<Vec<&str>>().len() == 3 {
                        continue;
                    }
                }
            }

            stdin.write_all(&buf[..size])?;
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
