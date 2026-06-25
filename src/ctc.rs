use crate::cache::{Channel, Program};
use crate::config::ProviderConfig;
use crate::stream::{resolve_stream_url, StreamProxyConfig};
use des::TdesEde3;
use ecb::cipher::block_padding::Pkcs7;
use ecb::cipher::{BlockEncryptMut, KeyInit};
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

type TdesEde3Enc = ecb::Encryptor<TdesEde3>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginSession {
    pub epg_lb_base: String,
    pub jsession_id: String,
    pub user_token: String,
}

pub fn fetch_channels(
    provider: &ProviderConfig,
    stream: &StreamProxyConfig,
) -> Result<Vec<Channel>, String> {
    let session = login(provider)?;
    let frameset = post_form(
        &format!("{}frameset_builder.jsp", session.epg_lb_base),
        &[
            ("BUILD_ACTION", "FRAMESET_BUILDER"),
            (
                "MAIN_WIN_SRC",
                "/iptvepg/frame310/first_channel_play.jsp?tempno=777",
            ),
            ("NEED_UPDATE_STB", "1"),
        ],
        Some(&format!("JSESSIONID={}", session.jsession_id)),
    )?;
    let raw = parse_channels(&frameset);
    let mapping = fetch_mapping(&session).unwrap_or_default();
    let by_user_channel: HashMap<String, u32> = mapping
        .into_iter()
        .filter_map(|(display, user_ch)| display.parse::<u32>().ok().map(|n| (user_ch, n)))
        .collect();
    raw.into_iter()
        .enumerate()
        .map(|(idx, ch)| {
            let number = by_user_channel
                .get(&ch.user_channel_id)
                .copied()
                .unwrap_or((idx + 1) as u32);
            let stream_url =
                resolve_stream_url(&ch.channel_url, stream).map_err(|e| e.to_string())?;
            Ok(Channel {
                number,
                name: ch.channel_name,
                stream_url,
                channel_code: Some(ch.channel_id),
            })
        })
        .collect()
}

pub fn fetch_programs(
    provider: &ProviderConfig,
    channel_code: &str,
    date_offset: i32,
) -> Result<Vec<Program>, String> {
    let session = login(provider)?;
    fetch_programs_with_session(&session, channel_code, date_offset)
}

pub fn fetch_programs_with_session(
    session: &LoginSession,
    channel_code: &str,
    date_offset: i32,
) -> Result<Vec<Program>, String> {
    let root = session
        .epg_lb_base
        .strip_suffix("function/")
        .unwrap_or(&session.epg_lb_base);
    let url = format!(
        "{root}frame1194/CHANNEL_PLAYER_UTILS/datas/prevue_list.jsp?channelcode={}&framecode=frame1194&versiondir=CHANNEL_PLAYER_UTILS&dateindex={date_offset}&stbtype=sdr&ajax=1",
        encode_query_component(channel_code)
    );
    let body = get(&url, Some(&format!("JSESSIONID={}", session.jsession_id)))?;
    parse_programs(&body)
}

pub fn login(provider: &ProviderConfig) -> Result<LoginSession, String> {
    let auth_base = provider.auth_server_url.trim_end_matches('/');
    let login_page = get(
        &format!("{auth_base}/auth?UserID={}&Action=Login", provider.user_id),
        None,
    )?;
    let encry_token = parse_encry_token(&login_page).ok_or("EncryToken not found")?;
    let authenticator = build_authenticator(
        &provider.user_id,
        &provider.password,
        &provider.stb_id,
        &provider.local_ip,
        &provider.local_mac,
        &encry_token,
        random_8_digit(),
    )?;
    let upload = post_form(
        &format!("{auth_base}/uploadAuthInfo"),
        &[
            ("UserID", &provider.user_id),
            ("Authenticator", &authenticator),
            ("AccessMethod", "dhcp"),
            ("AccessUserName", &provider.user_id),
        ],
        None,
    )?;
    let config = parse_set_config(&upload);
    let user_token = config
        .get("UserToken")
        .cloned()
        .ok_or("UserToken missing")?;
    let service = get(
        &format!("{auth_base}/getServiceList"),
        Some(&format!("UserToken={user_token}")),
    )?;
    let initial_url = parse_document_location(&service).ok_or("getServiceList redirect missing")?;
    let first = get(&initial_url, None)?;
    let balanced_url =
        parse_document_location(&first).ok_or("EPG load-balanced redirect missing")?;
    let (portal_html, jsession_id, final_url) = get_with_cookie_capture(&balanced_url)?;
    let epg_lb_base = epg_base_from_url(&final_url);
    let hidden = parse_hidden_inputs(&portal_html);
    let form: Vec<(&str, &str)> = hidden
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    let _ = post_form(
        &format!("{epg_lb_base}funcportalauth.jsp"),
        &form,
        Some(&format!("JSESSIONID={jsession_id}")),
    );
    Ok(LoginSession {
        epg_lb_base,
        jsession_id,
        user_token,
    })
}

fn fetch_mapping(session: &LoginSession) -> Result<HashMap<String, String>, String> {
    let root = session
        .epg_lb_base
        .strip_suffix("function/")
        .unwrap_or(&session.epg_lb_base);
    let body = get(
        &format!("{root}frame224/datas/get_channel_info_mapping.jsp"),
        Some(&format!("JSESSIONID={}", session.jsession_id)),
    )?;
    Ok(parse_mixno_mapping(&body))
}

pub fn build_authenticator(
    user_id: &str,
    password: &str,
    stb_id: &str,
    ip: &str,
    mac: &str,
    encry_token: &str,
    rand: u64,
) -> Result<String, String> {
    if !(10_000_000..=99_999_999).contains(&rand) {
        return Err(format!("rand must be 8 digits, got {rand}"));
    }
    let plaintext = format!("{rand}${encry_token}${user_id}${stb_id}${ip}${mac}$$CTC");
    let mut key = [b'0'; 24];
    for (i, b) in password.as_bytes().iter().take(24).enumerate() {
        key[i] = *b;
    }
    let encrypted =
        TdesEde3Enc::new(&key.into()).encrypt_padded_vec_mut::<Pkcs7>(plaintext.as_bytes());
    Ok(to_hex(&encrypted))
}

fn random_8_digit() -> u64 {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64;
    10_000_000 + nanos % 90_000_000
}

fn get(url: &str, cookie: Option<&str>) -> Result<String, String> {
    let mut req = ureq::get(url);
    if let Some(cookie) = cookie {
        req = req.set("Cookie", cookie);
    }
    req.call()
        .map_err(|e| format!("GET {url} failed: {e}"))?
        .into_string()
        .map_err(|e| format!("GET {url} body failed: {e}"))
}

fn get_with_cookie_capture(url: &str) -> Result<(String, String, String), String> {
    let resp = ureq::get(url)
        .call()
        .map_err(|e| format!("GET {url} failed: {e}"))?;
    let final_url = resp.get_url().to_string();
    let jsession = resp
        .headers_names()
        .into_iter()
        .filter(|h| h.eq_ignore_ascii_case("set-cookie"))
        .filter_map(|h| resp.header(&h).map(str::to_string))
        .find_map(|h| parse_cookie_value(&h, "JSESSIONID"))
        .ok_or("JSESSIONID cookie missing")?;
    let body = resp
        .into_string()
        .map_err(|e| format!("GET {url} body failed: {e}"))?;
    Ok((body, jsession, final_url))
}

fn post_form(url: &str, fields: &[(&str, &str)], cookie: Option<&str>) -> Result<String, String> {
    let mut req = ureq::post(url);
    if let Some(cookie) = cookie {
        req = req.set("Cookie", cookie);
    }
    req.send_form(fields)
        .map_err(|e| format!("POST {url} failed: {e}"))?
        .into_string()
        .map_err(|e| format!("POST {url} body failed: {e}"))
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RawChannel {
    channel_id: String,
    channel_name: String,
    user_channel_id: String,
    channel_url: String,
}

fn parse_encry_token(html: &str) -> Option<String> {
    between(html, "Authentication.CTCGetAuthInfo('", "')")
}

fn parse_set_config(html: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for part in html.split("Authentication.CTCSetConfig").skip(1) {
        if let Some(body) = between(part, "('", "')") {
            if let Some((k, v)) = body.split_once("','") {
                out.insert(k.to_string(), v.to_string());
            }
        }
    }
    out
}

fn parse_document_location(html: &str) -> Option<String> {
    html.split("document.location")
        .nth(1)
        .and_then(|s| s.split('=').nth(1))
        .and_then(|s| {
            let s = s.trim();
            let quote = s.chars().next()?;
            if quote == '\'' || quote == '"' {
                s[1..].split(quote).next().map(str::to_string)
            } else {
                None
            }
        })
}

fn parse_hidden_inputs(html: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for input in html
        .split('<')
        .filter(|s| s.to_ascii_lowercase().starts_with("input"))
    {
        if !input.to_ascii_lowercase().contains("type=\"hidden\"")
            && !input.to_ascii_lowercase().contains("type='hidden'")
        {
            continue;
        }
        if let (Some(name), Some(value)) = (attr(input, "name"), attr(input, "value")) {
            out.insert(name, value);
        }
    }
    out
}

fn parse_channels(html: &str) -> Vec<RawChannel> {
    let mut out = Vec::new();
    for part in html.split("jsSetConfig").skip(1) {
        let Some(body) = between(part, "'Channel', '", "')") else {
            continue;
        };
        let mut kv = HashMap::new();
        for piece in body.split(',') {
            if let Some((k, v)) = piece.split_once('=') {
                kv.insert(k.trim().to_string(), v.trim().trim_matches('"').to_string());
            }
        }
        if !kv.is_empty() {
            out.push(RawChannel {
                channel_id: kv.remove("ChannelID").unwrap_or_default(),
                channel_name: kv.remove("ChannelName").unwrap_or_default(),
                user_channel_id: kv.remove("UserChannelID").unwrap_or_default(),
                channel_url: kv.remove("ChannelURL").unwrap_or_default(),
            });
        }
    }
    out
}

fn parse_mixno_mapping(json_text: &str) -> HashMap<String, String> {
    let raw = serde_json::from_str::<serde_json::Value>(json_text)
        .ok()
        .and_then(|v| {
            v.get("channelMixnoMapping")
                .and_then(|v| v.as_str())
                .map(str::to_string)
        })
        .unwrap_or_default();
    raw.split(',')
        .filter_map(|piece| piece.split_once(':'))
        .map(|(display, user)| {
            (
                display.trim_matches('"').to_string(),
                user.trim_matches('"').to_string(),
            )
        })
        .collect()
}

fn parse_programs(json_text: &str) -> Result<Vec<Program>, String> {
    let value = serde_json::from_str::<serde_json::Value>(json_text)
        .map_err(|e| format!("EPG JSON parse failed: {e}"))?;
    let entries = value
        .get("channelPrevue")
        .and_then(|v| v.as_array())
        .ok_or("EPG channelPrevue missing")?;
    Ok(entries.iter().filter_map(program_from_value).collect())
}

fn program_from_value(value: &serde_json::Value) -> Option<Program> {
    let start = normalize_ctc_time(value.get("begintime")?.as_str()?)?;
    let end = normalize_ctc_time(value.get("endtime")?.as_str()?)?;
    Some(Program {
        code: value
            .get("prevuecode")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        name: value
            .get("prevuename")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        start,
        end,
        is_live: value.get("isLive").and_then(|v| v.as_str()) == Some("1"),
        is_replayable: value.get("isBack").and_then(|v| v.as_str()) == Some("1")
            || value.get("isRecord").and_then(|v| v.as_str()) == Some("1"),
    })
}

fn normalize_ctc_time(raw: &str) -> Option<String> {
    if raw.len() == 14 && raw.chars().all(|c| c.is_ascii_digit()) {
        return Some(format!(
            "{}-{}-{}T{}:{}:{}+08:00",
            &raw[0..4],
            &raw[4..6],
            &raw[6..8],
            &raw[8..10],
            &raw[10..12],
            &raw[12..14]
        ));
    }
    if raw.len() == 19 {
        let bytes = raw.as_bytes();
        let dotted = bytes[4] == b'.' && bytes[7] == b'.' && bytes[10] == b' ';
        if dotted {
            return Some(format!(
                "{}-{}-{}T{}+08:00",
                &raw[0..4],
                &raw[5..7],
                &raw[8..10],
                &raw[11..19]
            ));
        }
    }
    if raw.contains('T') {
        return Some(raw.to_string());
    }
    None
}

fn encode_query_component(value: &str) -> String {
    let mut out = String::new();
    for b in value.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn parse_cookie_value(header: &str, name: &str) -> Option<String> {
    header.split(';').find_map(|part| {
        let (k, v) = part.trim().split_once('=')?;
        (k == name).then(|| v.to_string())
    })
}

fn attr(input: &str, name: &str) -> Option<String> {
    for quote in ['"', '\''] {
        let prefix = format!("{name}={quote}");
        if let Some(value) = input
            .split(&prefix)
            .nth(1)
            .and_then(|s| s.split(quote).next())
        {
            return Some(value.to_string());
        }
    }
    None
}

fn between(text: &str, start: &str, end: &str) -> Option<String> {
    text.split(start)
        .nth(1)?
        .split(end)
        .next()
        .map(str::to_string)
}

fn epg_base_from_url(url: &str) -> String {
    let without_query = url.split('?').next().unwrap_or(url);
    let idx = without_query
        .rfind('/')
        .map(|i| i + 1)
        .unwrap_or(without_query.len());
    without_query[..idx].to_string()
}

fn to_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authenticator_is_hex_and_validates_rand() {
        let value =
            build_authenticator("u", "000000", "s", "1.2.3.4", "aa", "tok", 12345678).unwrap();
        assert!(value.chars().all(|c| c.is_ascii_hexdigit()));
        assert!(build_authenticator("u", "p", "s", "i", "m", "t", 1).is_err());
    }

    #[test]
    fn parses_ctc_channel_blocks() {
        let html = "jsSetConfig('Channel', 'ChannelID=ch1,ChannelName=News,UserChannelID=001,ChannelURL=igmp://239.0.0.1:8000')";
        let channels = parse_channels(html);
        assert_eq!("ch1", channels[0].channel_id);
        assert_eq!("News", channels[0].channel_name);
    }

    #[test]
    fn parses_mapping() {
        let mapping = parse_mixno_mapping(r#"{"channelMixnoMapping":"001:100,002:200"}"#);
        assert_eq!(Some(&"100".to_string()), mapping.get("001"));
    }

    #[test]
    fn parses_programs_and_normalizes_timestamps() {
        let programs = parse_programs(
            r#"{"channelPrevue":[
              {"prevuecode":"p1","prevuename":"News","begintime":"20260607080000","endtime":"20260607090000","isLive":"1","isBack":"0","isRecord":"1"},
              {"prevuecode":"p2","prevuename":"Bad","begintime":"bad","endtime":"20260607100000"}
            ]}"#,
        )
        .unwrap();

        assert_eq!(1, programs.len());
        assert_eq!("p1", programs[0].code);
        assert_eq!("2026-06-07T08:00:00+08:00", programs[0].start);
        assert!(programs[0].is_live);
        assert!(programs[0].is_replayable);
    }

    #[test]
    fn normalizes_dotted_timestamp() {
        assert_eq!(
            Some("2026-03-14T00:56:00+08:00".to_string()),
            normalize_ctc_time("2026.03.14 00:56:00")
        );
    }
}
