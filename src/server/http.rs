use serde::Serialize;
use std::collections::HashMap;

#[derive(Debug)]
pub(super) struct Request {
    pub(super) method: String,
    pub(super) path: String,
    pub(super) query: HashMap<String, String>,
    pub(super) headers: HashMap<String, String>,
    pub(super) body: String,
}

impl Request {
    pub(super) fn parse(raw: &str) -> Result<Self, String> {
        let (head, body) = raw.split_once("\r\n\r\n").unwrap_or((raw, ""));
        let mut lines = head.lines();
        let first = lines
            .next()
            .ok_or_else(|| "missing request line".to_string())?;
        let mut parts = first.split_whitespace();
        let method = parts
            .next()
            .ok_or_else(|| "missing method".to_string())?
            .to_string();
        let target = parts.next().ok_or_else(|| "missing target".to_string())?;
        let (path_raw, query_raw) = target.split_once('?').unwrap_or((target, ""));
        let path = path_raw.to_string();
        let mut headers = HashMap::new();
        for line in lines {
            if let Some((k, v)) = line.split_once(':') {
                headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
            }
        }
        Ok(Self {
            method,
            path,
            query: parse_form(query_raw),
            headers,
            body: body.to_string(),
        })
    }
}

pub(super) fn parse_form(body: &str) -> HashMap<String, String> {
    body.split('&')
        .filter_map(|part| part.split_once('='))
        .map(|(k, v)| (percent_decode(k), percent_decode(v)))
        .collect()
}

fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'+' {
            out.push(b' ');
            i += 1;
        } else if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = &value[i + 1..i + 3];
            if let Ok(v) = u8::from_str_radix(hex, 16) {
                out.push(v);
                i += 3;
            } else {
                out.push(bytes[i]);
                i += 1;
            }
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub(super) fn json<T: Serialize>(status: u16, value: &T) -> String {
    response(
        status,
        "application/json",
        serde_json::to_string(value).unwrap(),
    )
}

pub(super) fn json_error(status: u16, code: &str, message: &str) -> String {
    json(
        status,
        &serde_json::json!({"error": {"code": code, "message": message}}),
    )
}

pub(super) fn response(status: u16, content_type: &str, body: String) -> String {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        404 => "Not Found",
        500 => "Internal Server Error",
        503 => "Service Unavailable",
        _ => "OK",
    };
    format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

pub(super) fn response_status(response: &str) -> &str {
    response
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .unwrap_or("unknown")
}
