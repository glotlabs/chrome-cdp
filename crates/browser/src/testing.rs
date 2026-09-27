//! A fake Chrome for Testing index and download server, for tests of
//! `install` here and in the CLI (feature `test-support`).
//!
//! It serves the two index files and one zip per version and product. Each
//! zip holds a shell script that answers `--version` like Chrome for
//! Testing, plus a symlink, so an install can be checked end to end.

#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::collections::BTreeMap;
use std::io::{BufRead as _, BufReader, Write as _};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;

use zip::write::SimpleFileOptions;

use crate::cft::Product;

/// One path the server answers.
#[derive(Debug, Clone)]
pub struct Response {
    pub status: u16,
    pub body: Vec<u8>,
    /// The `Content-Length` to claim; the body's length if `None`. A larger
    /// one makes a truncated download.
    pub claimed_len: Option<usize>,
}

impl Response {
    pub fn ok(body: impl Into<Vec<u8>>) -> Self {
        Self {
            status: 200,
            body: body.into(),
            claimed_len: None,
        }
    }
}

/// The running server. It stops with the test process.
#[derive(Debug, Clone)]
pub struct FakeCft {
    /// `http://127.0.0.1:<port>`; point `CHROME_CDP_CFT_URL` here.
    pub url: String,
    routes: Arc<Mutex<BTreeMap<String, Response>>>,
    requests: Arc<Mutex<Vec<String>>>,
}

/// Channel versions the fake index lists: stable, beta, dev, canary.
pub const CHANNELS: [(&str, &str); 4] = [
    ("Stable", "154.0.8037.57"),
    ("Beta", "155.0.8059.12"),
    ("Dev", "156.0.8072.0"),
    ("Canary", "156.0.8075.0"),
];

/// Versions only in the known-good index (not on any channel).
pub const OLDER: &[&str] = &["153.0.8001.0", "154.0.8037.3"];

impl FakeCft {
    /// Serves an index with [`CHANNELS`] and [`OLDER`] for `platform`, with
    /// both products of each.
    pub fn start(platform: &str) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let server = Self {
            url,
            routes: Arc::default(),
            requests: Arc::default(),
        };
        let (routes, requests) = (server.routes.clone(), server.requests.clone());
        thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let (routes, requests) = (routes.clone(), requests.clone());
                thread::spawn(move || serve(stream, &routes, &requests));
            }
        });
        server.publish(platform);
        server
    }

    /// Sets what `path` answers.
    pub fn route(&self, path: &str, response: Response) {
        self.routes
            .lock()
            .unwrap()
            .insert(path.to_owned(), response);
    }

    /// The paths requested so far, in order.
    pub fn requests(&self) -> Vec<String> {
        self.requests.lock().unwrap().clone()
    }

    /// The zip path of a version and product.
    pub fn zip_path(version: &str, product: Product, platform: &str) -> String {
        format!("/{version}/{platform}/{}.zip", product.dir(platform))
    }

    fn publish(&self, platform: &str) {
        let versions: Vec<&str> = OLDER
            .iter()
            .copied()
            .chain(CHANNELS.iter().map(|(_, v)| *v))
            .collect();
        for version in &versions {
            for product in [Product::Chrome, Product::HeadlessShell] {
                self.route(
                    &Self::zip_path(version, product, platform),
                    Response::ok(chrome_zip(version, product, platform)),
                );
            }
        }
        let entry = |version: &str| {
            let list = |product: Product| {
                format!(
                    r#"[{{"platform":"{platform}","url":"{}{}"}},{{"platform":"win64","url":"x"}}]"#,
                    self.url,
                    Self::zip_path(version, product, platform)
                )
            };
            format!(
                r#""version":"{version}","revision":"1","downloads":{{"chrome":{},"chrome-headless-shell":{}}}"#,
                list(Product::Chrome),
                list(Product::HeadlessShell)
            )
        };
        let channels: Vec<String> = CHANNELS
            .iter()
            .map(|(name, v)| format!(r#""{name}":{{"channel":"{name}",{}}}"#, entry(v)))
            .collect();
        self.route(
            "/last-known-good-versions-with-downloads.json",
            Response::ok(format!(
                r#"{{"timestamp":"t","channels":{{{}}}}}"#,
                channels.join(",")
            )),
        );
        let known: Vec<String> = versions
            .iter()
            .map(|v| format!("{{{}}}", entry(v)))
            .collect();
        self.route(
            "/known-good-versions-with-downloads.json",
            Response::ok(format!(
                r#"{{"timestamp":"t","versions":[{}]}}"#,
                known.join(",")
            )),
        );
    }
}

fn serve(
    stream: TcpStream,
    routes: &Mutex<BTreeMap<String, Response>>,
    requests: &Mutex<Vec<String>>,
) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut line = String::new();
    if reader.read_line(&mut line).is_err() {
        return;
    }
    let path = line.split_whitespace().nth(1).unwrap_or("/").to_owned();
    loop {
        let mut header = String::new();
        match reader.read_line(&mut header) {
            Ok(0) | Err(_) => break,
            Ok(_) if header == "\r\n" => break,
            Ok(_) => {}
        }
    }
    requests.lock().unwrap().push(path.clone());
    let response = routes
        .lock()
        .unwrap()
        .get(&path)
        .cloned()
        .unwrap_or(Response {
            status: 404,
            body: b"not found".to_vec(),
            claimed_len: None,
        });
    let mut stream = stream;
    let head = format!(
        "HTTP/1.1 {} X\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        response.status,
        response.claimed_len.unwrap_or(response.body.len())
    );
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(&response.body);
}

/// A zip laid out like Chrome for Testing's: `<product>-<platform>/` with
/// the binary (a script answering `--version`), a library, and a symlink.
pub fn chrome_zip(version: &str, product: Product, platform: &str) -> Vec<u8> {
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let dir = product.dir(platform);
    let binary = product.binary(platform);
    let exec = SimpleFileOptions::default().unix_permissions(0o755);
    let plain = SimpleFileOptions::default().unix_permissions(0o644);
    zip.add_directory(format!("{dir}/"), SimpleFileOptions::default())
        .unwrap();
    zip.start_file(&binary, exec).unwrap();
    let name = match product {
        Product::Chrome => "Google Chrome for Testing",
        Product::HeadlessShell => "Google Chrome for Testing (headless shell)",
    };
    write!(zip, "#!/bin/sh\necho '{name} {version} '\n").unwrap();
    zip.start_file(format!("{dir}/libfake.so"), plain).unwrap();
    zip.write_all(b"not really a library").unwrap();
    zip.add_symlink(format!("{dir}/libfake-link.so"), "libfake.so", plain)
        .unwrap();
    zip.finish().unwrap().into_inner()
}
