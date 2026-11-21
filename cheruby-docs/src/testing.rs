//! A stand-in HTTP server for the tests: one thread, one request per
//! connection, every request logged as "<method> <path>".

use std::{
    io::{BufRead, BufReader, Read, Write},
    net::TcpListener,
    sync::{Arc, Mutex},
};

pub type Log = Arc<Mutex<Vec<String>>>;

/// Serves answer(method, path, body) -> (status, JSON body) on a free
/// loopback port; returns its base URL and the log of requests.
pub fn serve(answer: impl Fn(&str, &str, &str) -> (u16, String) + Send + 'static) -> (String, Log) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("a free port");
    let url = format!("http://{}", listener.local_addr().expect("an address"));
    let log: Log = Arc::default();
    let seen = Arc::clone(&log);
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut reader = BufReader::new(stream.try_clone().expect("a stream"));
            let mut first = String::new();
            reader.read_line(&mut first).ok();
            let mut length = 0;
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).ok();
                if line.trim().is_empty() {
                    break;
                }
                if let Some((k, v)) = line.split_once(':')
                    && k.eq_ignore_ascii_case("content-length")
                {
                    length = v.trim().parse().unwrap_or(0);
                }
            }
            let mut body = vec![0; length];
            reader.read_exact(&mut body).ok();
            let mut words = first.split_whitespace();
            let (method, path) = (words.next().unwrap_or(""), words.next().unwrap_or(""));
            seen.lock().unwrap().push(format!("{method} {path}"));
            let (code, reply) = answer(method, path, &String::from_utf8_lossy(&body));
            let _ = write!(
                stream,
                "HTTP/1.1 {code} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",
                reply.len()
            );
        }
    });
    (url, log)
}
