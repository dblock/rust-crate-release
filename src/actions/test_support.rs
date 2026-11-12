use serde_json::{Value, json};
use std::io::{Cursor, Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

pub type TestResult<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;

pub fn zip_files(files: &[(String, Vec<u8>)]) -> TestResult<Vec<u8>> {
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (name, bytes) in files {
        zip.start_file(name, zip::write::SimpleFileOptions::default())?;
        zip.write_all(bytes)?;
    }
    Ok(zip.finish()?.into_inner())
}

pub struct Server {
    pub endpoint: String,
    pub metadata: Arc<Mutex<Value>>,
    pub bytes: Arc<Mutex<Vec<u8>>>,
    pub requests: Arc<Mutex<Vec<String>>>,
    stopped: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Server {
    pub fn new(bytes: Vec<u8>) -> TestResult<Self> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        listener.set_nonblocking(true)?;
        let endpoint = format!("http://{}", listener.local_addr()?);
        let metadata = Arc::new(Mutex::new(json!({
            "id":123,"name":"rcr-release-42-1","expired":false,
            "size_in_bytes":bytes.len(),"workflow_run":{"id":42},
        })));
        let bytes = Arc::new(Mutex::new(bytes));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let stopped = Arc::new(AtomicBool::new(false));
        let mut server = Self {
            endpoint: endpoint.clone(),
            metadata: metadata.clone(),
            bytes: bytes.clone(),
            requests: requests.clone(),
            stopped: stopped.clone(),
            thread: None,
        };
        let thread = std::thread::spawn(move || {
            while !stopped.load(Ordering::Acquire) {
                let (mut stream, _) = match listener.accept() {
                    Ok(connection) => connection,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5));
                        continue;
                    }
                    Err(_) => break,
                };
                // Windows inherits the listener's nonblocking mode.
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                stream
                    .set_write_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut request = Vec::new();
                while !request.ends_with(b"\r\n\r\n") && request.len() < 16384 {
                    let mut byte = [0];
                    if stream.read(&mut byte).unwrap_or(0) == 0 {
                        break;
                    }
                    request.push(byte[0]);
                }
                let request = String::from_utf8_lossy(&request).into_owned();
                requests.lock().unwrap().push(request.clone());
                let target = request.split_whitespace().nth(1).unwrap_or("");
                let (status, headers, body) = if target.ends_with("/zip") {
                    (
                        "302 Found",
                        format!("Location: {endpoint}/download?sig=private-location\r\n"),
                        Vec::new(),
                    )
                } else if target == "/download?sig=private-location" {
                    ("200 OK", String::new(), bytes.lock().unwrap().clone())
                } else if target.ends_with("/artifacts/123") {
                    (
                        "200 OK",
                        "Content-Type: application/json\r\n".into(),
                        serde_json::to_vec(&*metadata.lock().unwrap()).unwrap(),
                    )
                } else {
                    ("404 Not Found", String::new(), Vec::new())
                };
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status}\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(&body);
            }
        });
        server.thread = Some(thread);
        Ok(server)
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            thread.join().unwrap();
        }
    }
}
