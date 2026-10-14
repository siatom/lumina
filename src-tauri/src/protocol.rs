//! Custom URI scheme `lumina://` used as the fast transport for image bytes.
//!
//! Requests look like (Windows):
//!   `http://lumina.localhost/?path=<encoded>&mode=full|thumb&max=256`
//! and on macOS/Linux:
//!   `lumina://localhost/?path=<encoded>&mode=full|thumb&max=256`
//!
//! Why a custom protocol instead of base64-over-IPC: the webview streams the
//! response body straight into the `<img>` decoder, so a 40 MB photo never
//! becomes a 53 MB base64 string, never crosses the JSON IPC boundary, and
//! thumbnails can be downscaled on the Rust side before they are sent.

use std::borrow::Cow;
use std::collections::HashMap;
use std::path::Path;

use tauri::http::{Request, Response, StatusCode};

use crate::imaging;

pub const SCHEME: &str = "lumina";

/// Base URL the frontend must prefix requests with, per platform.
///
/// Exposed to JS through the `get_protocol_base` command so the frontend never
/// has to hard-code the platform-specific host form.
pub fn protocol_base() -> String {
    if cfg!(windows) {
        format!("http://{SCHEME}.localhost")
    } else {
        format!("{SCHEME}://localhost")
    }
}

/// Protocol handler. Registered once in `lib.rs`.
pub fn handle<R: tauri::Runtime>(
    _ctx: tauri::UriSchemeContext<'_, R>,
    request: Request<Vec<u8>>,
) -> Response<Cow<'static, [u8]>> {
    match route(&request) {
        Ok((bytes, mime)) => Response::builder()
            .status(StatusCode::OK)
            .header("Content-Type", mime)
            .header("Access-Control-Allow-Origin", "*")
            .body(Cow::Owned(bytes))
            .unwrap_or_else(|_| error_response(StatusCode::INTERNAL_SERVER_ERROR, "response build failed")),
        Err(msg) => error_response(StatusCode::BAD_REQUEST, &msg),
    }
}

fn error_response(status: StatusCode, msg: &str) -> Response<Cow<'static, [u8]>> {
    Response::builder()
        .status(status)
        .header("Content-Type", "text/plain; charset=utf-8")
        .body(Cow::Owned(msg.as_bytes().to_vec()))
        .expect("static error response is always valid")
}

/// Decode a request into `(bytes, mime)`.
fn route(request: &Request<Vec<u8>>) -> Result<(Vec<u8>, &'static str), String> {
    let query = request.uri().query().unwrap_or("");
    let params = parse_query(query);

    let path = params.get("path").ok_or("missing `path` parameter")?;
    let path = Path::new(path);

    match params.get("mode").map(String::as_str).unwrap_or("full") {
        "thumb" => {
            let max = params
                .get("max")
                .and_then(|s| s.parse::<u32>().ok())
                .unwrap_or(256)
                .clamp(16, 4096);
            let bytes = imaging::make_thumbnail(path, max).map_err(|e| e.to_string())?;
            Ok((bytes, "image/png"))
        }
        _ => imaging::read_for_display(path).map_err(|e| e.to_string()),
    }
}

/// Parse an `application/x-www-form-urlencoded` style query string.
fn parse_query(query: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for pair in query.split('&') {
        if pair.is_empty() {
            continue;
        }
        let mut it = pair.splitn(2, '=');
        let key = it.next().unwrap_or("");
        let value = it.next().unwrap_or("");
        map.insert(percent_decode(key), percent_decode(value));
    }
    map
}

/// Decode `%XX` escapes and `+` (space). Returns the input on malformed escapes.
fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                match (hex_val(bytes[i + 1]), hex_val(bytes[i + 2])) {
                    (Some(hi), Some(lo)) => {
                        out.push(hi * 16 + lo);
                        i += 3;
                    }
                    _ => {
                        out.push(b'%');
                        i += 1;
                    }
                }
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_percent_and_plus() {
        assert_eq!(percent_decode("a%20b%2Fc"), "a b/c");
        assert_eq!(percent_decode("a+b"), "a b");
        assert_eq!(percent_decode("C%3A%5CUsers%5Cme%5Cpic.png"), "C:\\Users\\me\\pic.png");
        assert_eq!(percent_decode("100%"), "100%");
    }

    #[test]
    fn parses_query_pairs() {
        let m = parse_query("path=C%3A%5Ca.png&mode=thumb&max=128");
        assert_eq!(m.get("path").unwrap(), "C:\\a.png");
        assert_eq!(m.get("mode").unwrap(), "thumb");
        assert_eq!(m.get("max").unwrap(), "128");
    }
}
