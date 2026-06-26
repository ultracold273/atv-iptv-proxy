use encoding_rs::{Encoding, UTF_16BE, UTF_16LE, UTF_8};
use std::io::Read;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BodyEncoding {
    pub label: String,
    pub source: &'static str,
    pub had_errors: bool,
}

pub fn response_to_string(resp: ureq::Response, context: &str) -> Result<String, String> {
    let content_type = resp.header("Content-Type").map(str::to_string);
    let mut bytes = Vec::new();
    resp.into_reader()
        .read_to_end(&mut bytes)
        .map_err(|e| format!("{context} body read failed: {e}"))?;
    let (text, encoding) = decode_bytes(&bytes, content_type.as_deref());
    eprintln!(
        "http_body: context={} bytes={} content_type={} encoding={} source={} had_errors={}",
        context,
        bytes.len(),
        content_type.as_deref().unwrap_or(""),
        encoding.label,
        encoding.source,
        encoding.had_errors
    );
    Ok(text)
}

pub fn decode_bytes(bytes: &[u8], content_type: Option<&str>) -> (String, BodyEncoding) {
    if let Some((encoding, offset)) = encoding_from_bom(bytes) {
        return decode_with_encoding(bytes, offset, encoding, "bom");
    }

    if let Some(label) = content_type.and_then(charset_from_content_type) {
        if let Some(encoding) = Encoding::for_label(label.as_bytes()) {
            return decode_with_encoding(bytes, 0, encoding, "content-type");
        }
        eprintln!("http_body: unknown charset label={label}");
    }

    if let Ok(text) = std::str::from_utf8(bytes) {
        return (
            text.to_string(),
            BodyEncoding {
                label: "utf-8".to_string(),
                source: "utf8-valid",
                had_errors: false,
            },
        );
    }

    let gb18030 = Encoding::for_label(b"gb18030").expect("gb18030 encoding must exist");
    decode_with_encoding(bytes, 0, gb18030, "utf8-invalid-fallback")
}

fn decode_with_encoding(
    bytes: &[u8],
    offset: usize,
    encoding: &'static Encoding,
    source: &'static str,
) -> (String, BodyEncoding) {
    let (text, _, had_errors) = encoding.decode(&bytes[offset..]);
    (
        text.into_owned(),
        BodyEncoding {
            label: encoding.name().to_ascii_lowercase(),
            source,
            had_errors,
        },
    )
}

fn encoding_from_bom(bytes: &[u8]) -> Option<(&'static Encoding, usize)> {
    if bytes.starts_with(&[0xef, 0xbb, 0xbf]) {
        Some((UTF_8, 3))
    } else if bytes.starts_with(&[0xff, 0xfe]) {
        Some((UTF_16LE, 2))
    } else if bytes.starts_with(&[0xfe, 0xff]) {
        Some((UTF_16BE, 2))
    } else {
        None
    }
}

fn charset_from_content_type(content_type: &str) -> Option<String> {
    content_type.split(';').find_map(|part| {
        let (name, value) = part.trim().split_once('=')?;
        name.trim().eq_ignore_ascii_case("charset").then(|| {
            value
                .trim()
                .trim_matches('"')
                .trim_matches('\'')
                .to_ascii_lowercase()
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_charset_from_content_type() {
        assert_eq!(
            Some("gbk".to_string()),
            charset_from_content_type("application/json; charset=GBK")
        );
        assert_eq!(
            Some("utf-8".to_string()),
            charset_from_content_type("text/html; charset=\"UTF-8\"")
        );
    }

    #[test]
    fn decodes_utf8_when_valid_without_header() {
        let (text, encoding) = decode_bytes("新闻".as_bytes(), None);
        assert_eq!("新闻", text);
        assert_eq!("utf-8", encoding.label);
        assert_eq!("utf8-valid", encoding.source);
        assert!(!encoding.had_errors);
    }

    #[test]
    fn decodes_declared_gbk() {
        let gbk = Encoding::for_label(b"gbk").unwrap();
        let (bytes, _, _) = gbk.encode("新闻");
        let (text, encoding) = decode_bytes(&bytes, Some("application/json; charset=gbk"));
        assert_eq!("新闻", text);
        assert_eq!("gbk", encoding.label);
        assert_eq!("content-type", encoding.source);
        assert!(!encoding.had_errors);
    }

    #[test]
    fn falls_back_to_gb18030_when_utf8_is_invalid() {
        let gb18030 = Encoding::for_label(b"gb18030").unwrap();
        let (bytes, _, _) = gb18030.encode("新闻");
        let (text, encoding) = decode_bytes(&bytes, None);
        assert_eq!("新闻", text);
        assert_eq!("gb18030", encoding.label);
        assert_eq!("utf8-invalid-fallback", encoding.source);
    }
}
