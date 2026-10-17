//! The pages' server: a few routes on 127.0.0.1, all under a random token
//! only den's pages are given, so no other page on the machine can read or
//! write through it. Each file a page was made for has an index, and its
//! routes sit under `/<token>/<index>/`:
//!
//! - `GET  .../`      the page (`page.html`)
//! - `GET  .../scene` the file, with its stamp in `X-Stamp`
//! - `PUT  .../scene` the file's new text; answers the new stamp
//! - `GET  .../stat`  `{ stamp, theme }`, which the page asks every second
//!
//! A stamp is the file's modified time and length, enough to tell a change on
//! disk from the page's own write. Every connection gets a thread and stays
//! open for the next request until the browser closes it or it idles: on
//! Windows, a server that closes first sometimes loses its answer. The server
//! lives as long as den: its threads end with the process.

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, UNIX_EPOCH};

/// The page, with `{{name}}` for the drawing's name (a JSON string).
const PAGE: &str = include_str!("page.html");
/// The most a drawing (images and all) may weigh.
const MAX_BODY: usize = 256 << 20;
/// How long a connection may idle before it is closed.
const IDLE: Duration = Duration::from_secs(30);

#[derive(Default)]
struct Shared {
    /// The files pages were made for, by index.
    files: Vec<PathBuf>,
    theme: String,
}

pub struct Server {
    /// `http://127.0.0.1:<port>/<token>`.
    base: String,
    shared: Arc<Mutex<Shared>>,
}

impl Server {
    pub fn start(theme: &str) -> io::Result<Server> {
        let listener = TcpListener::bind(("127.0.0.1", 0))?;
        let port = listener.local_addr()?.port();
        let token = token();
        let shared = Arc::new(Mutex::new(Shared { files: Vec::new(), theme: theme.to_string() }));
        let state = shared.clone();
        let prefix = format!("/{token}/");
        std::thread::Builder::new().name("excalidraw-server".into()).spawn(move || {
            for stream in listener.incoming().flatten() {
                let (state, prefix) = (state.clone(), prefix.clone());
                _ = std::thread::Builder::new()
                    .name("excalidraw-request".into())
                    .spawn(move || _ = serve(&stream, &prefix, &state));
            }
        })?;
        Ok(Server { base: format!("http://127.0.0.1:{port}/{token}"), shared })
    }

    /// The address of `path`'s page.
    pub fn page_url(&self, path: &Path) -> String {
        let mut shared = lock(&self.shared);
        let index = match shared.files.iter().position(|file| file == path) {
            Some(index) => index,
            None => {
                shared.files.push(path.to_path_buf());
                shared.files.len() - 1
            }
        };
        format!("{}/{index}/", self.base)
    }

    /// The pages take it on their next `stat`.
    pub fn set_theme(&self, theme: &str) {
        lock(&self.shared).theme = theme.to_string();
    }
}

fn lock(shared: &Mutex<Shared>) -> MutexGuard<'_, Shared> {
    shared.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// 128 random bits as hex, from the keys std seeds its hash maps with.
fn token() -> String {
    let random = || RandomState::new().build_hasher().finish();
    format!("{:016x}{:016x}", random(), random())
}

/// What tells a change of `path` on disk; empty when it can't be read.
fn stamp(path: &Path) -> String {
    let Ok(meta) = std::fs::metadata(path) else {
        return String::new();
    };
    let modified = meta
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |since| since.as_nanos());
    format!("{modified}-{}", meta.len())
}

struct Request {
    method: String,
    /// The path, without the query.
    path: String,
    body: Vec<u8>,
    /// Asked to close the connection after.
    close: bool,
}

/// The next request; None when the browser closed the connection.
fn read_request(reader: &mut BufReader<&TcpStream>) -> io::Result<Option<Request>> {
    let mut line = String::new();
    if reader.read_line(&mut line)? == 0 {
        return Ok(None);
    }
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_string();
    let target = parts.next().unwrap_or_default();
    let path = target.split('?').next().unwrap_or_default().to_string();
    let (mut length, mut close) = (0, false);
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header)? == 0 || header.trim_end().is_empty() {
            break;
        }
        if let Some((name, value)) = header.split_once(':') {
            let (name, value) = (name.trim(), value.trim());
            if name.eq_ignore_ascii_case("content-length") {
                length = value.parse().unwrap_or(0);
            } else if name.eq_ignore_ascii_case("connection") {
                close = value.eq_ignore_ascii_case("close");
            }
        }
    }
    if length > MAX_BODY {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "too large"));
    }
    let mut body = vec![0; length];
    reader.read_exact(&mut body)?;
    Ok(Some(Request { method, path, body, close }))
}

fn respond(mut stream: &TcpStream, status: &str, kind: &str, headers: &str, body: &[u8]) -> io::Result<()> {
    write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nCache-Control: no-store\r\n{headers}\r\n",
        body.len()
    )?;
    stream.write_all(body)?;
    stream.flush()
}

/// Answer the requests on `stream` until the browser closes it, it idles, or
/// a request asks to close it.
fn serve(stream: &TcpStream, prefix: &str, shared: &Mutex<Shared>) -> io::Result<()> {
    stream.set_read_timeout(Some(IDLE))?;
    let mut reader = BufReader::new(stream);
    while let Some(request) = read_request(&mut reader)? {
        answer(stream, &request, prefix, shared)?;
        if request.close {
            break;
        }
    }
    Ok(())
}

fn answer(stream: &TcpStream, request: &Request, prefix: &str, shared: &Mutex<Shared>) -> io::Result<()> {
    let not_found = || respond(stream, "404 Not Found", "text/plain", "", b"not found");
    let Some((index, route)) = request.path.strip_prefix(prefix).and_then(|rest| rest.split_once('/')) else {
        return not_found();
    };
    let (path, theme) = {
        let shared = lock(shared);
        match index.parse::<usize>().ok().and_then(|index| shared.files.get(index)) {
            Some(path) => (path.clone(), shared.theme.clone()),
            None => return not_found(),
        }
    };
    match (request.method.as_str(), route) {
        ("GET", "") => {
            let name = path.file_stem().map(|stem| stem.to_string_lossy()).unwrap_or_default();
            let name = serde_json::to_string(&name).unwrap_or_default().replace('<', "\\u003c");
            respond(stream, "200 OK", "text/html; charset=utf-8", "", PAGE.replace("{{name}}", &name).as_bytes())
        }
        ("GET", "scene") => match std::fs::read(&path) {
            Ok(text) => {
                let header = format!("X-Stamp: {}\r\n", stamp(&path));
                respond(stream, "200 OK", "application/json; charset=utf-8", &header, &text)
            }
            Err(err) => respond(stream, "404 Not Found", "text/plain", "", err.to_string().as_bytes()),
        },
        ("PUT", "scene") => match std::fs::write(&path, &request.body) {
            Ok(()) => respond(stream, "200 OK", "text/plain", "", stamp(&path).as_bytes()),
            Err(err) => respond(stream, "500 Internal Server Error", "text/plain", "", err.to_string().as_bytes()),
        },
        ("GET", "stat") => {
            let stat = serde_json::json!({ "stamp": stamp(&path), "theme": theme });
            respond(stream, "200 OK", "application/json", "", stat.to_string().as_bytes())
        }
        _ => not_found(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Requests on one connection, as a browser makes them: answered as
    /// (status line, headers, body).
    struct Client(BufReader<TcpStream>);

    impl Client {
        fn new(server: &Server) -> Client {
            let host = server.base.strip_prefix("http://").unwrap().split('/').next().unwrap();
            Client(BufReader::new(TcpStream::connect(host).unwrap()))
        }

        #[track_caller]
        fn fetch(&mut self, url: &str, method: &str, body: &str) -> (String, String, String) {
            let rest = url.strip_prefix("http://").unwrap();
            let path = &rest[rest.find('/').unwrap()..];
            let request = format!("{method} {path} HTTP/1.1\r\nHost: x\r\nContent-Length: {}\r\n\r\n{body}", body.len());
            self.0.get_mut().write_all(request.as_bytes()).unwrap();
            let (mut status, mut headers) = (String::new(), String::new());
            self.0.read_line(&mut status).unwrap();
            loop {
                let line = self.line();
                if line.is_empty() {
                    break;
                }
                headers.push_str(&line);
                headers.push('\n');
            }
            let length = headers.lines().find_map(|line| line.strip_prefix("Content-Length: "));
            let mut body = Vec::new();
            match length {
                Some(length) => body.resize(length.parse().unwrap(), 0),
                // A filtering proxy (AdGuard) may send it in chunks.
                None => loop {
                    let size = usize::from_str_radix(&self.line(), 16).unwrap();
                    let mut chunk = vec![0; size];
                    self.0.read_exact(&mut chunk).unwrap();
                    self.line();
                    if size == 0 {
                        break;
                    }
                    body.extend(chunk);
                },
            }
            if length.is_some() {
                self.0.read_exact(&mut body).unwrap();
            }
            (status, headers, String::from_utf8(body).unwrap())
        }

        fn line(&mut self) -> String {
            let mut line = String::new();
            self.0.read_line(&mut line).unwrap();
            line.trim_end().to_string()
        }
    }

    #[test]
    fn serves_reads_and_writes_the_file() {
        let dir = std::env::temp_dir().join(format!("den-excalidraw-{}", token()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("plan.excalidraw");
        std::fs::write(&file, "{}").unwrap();
        let server = Server::start("dark").unwrap();
        let page = server.page_url(&file);
        assert_eq!(server.page_url(&file), page, "one page per file");
        // All on one connection, kept open between them.
        let mut client = Client::new(&server);

        // The page needs no file, only its name (one Windows can't hold).
        let (status, _, html) = client.fetch(&server.page_url(&dir.join("My <b>.excalidraw")), "GET", "");
        assert!(status.contains("200"));
        let escaped = ["const NAME = \"My ", "\\", "u003cb>\";"].concat();
        assert!(html.contains(&escaped), "the name, escaped");

        let (_, headers, text) = client.fetch(&format!("{page}scene"), "GET", "");
        assert_eq!(text, "{}");
        assert!(headers.contains(&format!("X-Stamp: {}", stamp(&file))));

        let (status, _, new_stamp) = client.fetch(&format!("{page}scene"), "PUT", r#"{"type":"excalidraw"}"#);
        assert!(status.contains("200"));
        assert_eq!(std::fs::read_to_string(&file).unwrap(), r#"{"type":"excalidraw"}"#);
        assert_eq!(new_stamp, stamp(&file));

        server.set_theme("light");
        let (_, _, stat) = client.fetch(&format!("{page}stat"), "GET", "");
        let stat: serde_json::Value = serde_json::from_str(&stat).unwrap();
        assert_eq!((stat["stamp"].as_str(), stat["theme"].as_str()), (Some(new_stamp.as_str()), Some("light")));

        let wrong_token = page.replace(&server.base, &format!("{}x", server.base));
        assert!(client.fetch(&wrong_token, "GET", "").0.contains("404"));
        assert!(client.fetch(&format!("{}/9/scene", server.base), "GET", "").0.contains("404"));
        _ = std::fs::remove_dir_all(dir);
    }
}
