//! A local HTTP server for the tests that fetch URLs, in this crate and in
//! those built on it (feature `served`).

use std::io::{BufRead, BufReader, Write};

/// Serves each of `files`, a path such as `/board.elf` and its bytes, on a
/// local port, and 404 at any other path, until the process ends; the
/// server's URL.
pub fn serve(files: Vec<(String, Vec<u8>)>) -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let mut stream = stream.unwrap();
            let mut request = BufReader::new(&stream);
            let mut line = String::new();
            request.read_line(&mut line).unwrap();
            let mut header = String::new();
            while request.read_line(&mut header).unwrap() > 2 {
                header.clear();
            }
            let path = line.split(' ').nth(1).unwrap_or_default();
            let (status, body) = match files.iter().find(|(served, _)| served == path) {
                Some((_, body)) => ("200 OK", body.as_slice()),
                None => ("404 Not Found", &b""[..]),
            };
            let head = format!(
                "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            stream.write_all(head.as_bytes()).unwrap();
            stream.write_all(body).unwrap();
        }
    });
    url
}
