use std::io;
use std::io::Write;
use std::net::UdpSocket;
use std::process::{Command, Stdio};
use std::time::Duration;

struct Handler {
    process: std::process::Child,
}

impl Handler {
    fn new(format: &str) -> io::Result<Handler> {
        let process = Command::new("aplay")
            .arg("--buffer-size=1888")
            .arg("-r")
            .arg("44100")
            .arg("-c")
            .arg("2")
            .arg("-f")
            .arg(format)
            .stdin(Stdio::piped())
            .spawn()?;

        Ok(Handler {
            process
        })
    }

    fn handle(mut self, conn: &UdpSocket) -> io::Result<()> {
        let mut buf = vec![0u8; 10240];

        conn.set_read_timeout(Some(Duration::from_secs(5)))?;

        let mut stdin = self.process.stdin.take().unwrap();

        loop {
            let size = conn.recv(&mut *buf)?;
            if size == 0 {
                return Ok(())
            }
            stdin.write_all(&buf[..size])?;
        }
    }
}

impl Drop for Handler {
    fn drop(&mut self) {
        self.process.wait();
    }
}

fn listener(port: &str, format: &str) -> io::Result<()> {
    let socket = UdpSocket::bind("0.0.0.0:".to_owned() + port)?;

    let mut buf = vec![0u8; 1];

    let sleep_time = Duration::from_secs(1);

    loop {
        std::thread::sleep(sleep_time);
        println!("waiting...");
        socket.set_read_timeout(None)?;
        let _ = socket.recv(&mut *buf);
        let handler = match Handler::new(format) {
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

fn main() -> io::Result<()> {
    let handle1 = std::thread::spawn(|| {
        listener("9016", "S16_LE")
    });
    listener("9032", "S32_LE")?;

    handle1.join();

    Ok(())
}
