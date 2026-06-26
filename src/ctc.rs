use crate::cache::{Channel, Program};
use crate::config::{ChannelNumberOverrides, ProviderConfig};
use crate::stream::{resolve_stream_url, StreamProxyConfig};
use des::TdesEde3;
use ecb::cipher::block_padding::Pkcs7;
use ecb::cipher::{BlockEncryptMut, KeyInit};
use std::collections::{HashMap, HashSet};
use std::time::{SystemTime, UNIX_EPOCH};

type TdesEde3Enc = ecb::Encryptor<TdesEde3>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginSession {
    pub epg_lb_base: String,
    pub jsession_id: String,
    pub user_token: String,
    pub user_id: String,
}

pub fn fetch_channels(
    provider: &ProviderConfig,
    stream: &StreamProxyConfig,
    overrides: &ChannelNumberOverrides,
) -> Result<Vec<Channel>, String> {
    eprintln!("ctc: fetch_channels start user_id={}", provider.user_id);
    let session = login(provider)?;
    eprintln!("ctc: fetch_channels requesting_frameset");
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
    eprintln!("ctc: fetch_channels parsed_raw count={}", raw.len());
    if raw.is_empty() {
        return Err("frameset_builder returned no channels".to_string());
    }
    let mapping = fetch_mapping(&session).unwrap_or_default();
    eprintln!("ctc: fetch_channels mapping count={}", mapping.len());
    let by_user_channel: HashMap<String, u32> = mapping
        .into_iter()
        .filter_map(|(display, user_ch)| display.parse::<u32>().ok().map(|n| (user_ch, n)))
        .collect();
    eprintln!(
        "ctc: fetch_channels overrides by_channel_code={}",
        overrides.len()
    );
    let numbers = assign_channel_numbers(&raw, &by_user_channel, overrides)?;
    raw.into_iter()
        .enumerate()
        .map(|(idx, ch)| {
            let stream_url =
                resolve_stream_url(&ch.channel_url, stream).map_err(|e| e.to_string())?;
            Ok(Channel {
                number: numbers[idx],
                name: ch.channel_name,
                stream_url,
                channel_code: Some(ch.channel_id),
            })
        })
        .collect()
}

fn assign_channel_numbers(
    channels: &[RawChannel],
    by_user_channel: &HashMap<String, u32>,
    overrides: &ChannelNumberOverrides,
) -> Result<Vec<u32>, String> {
    let mut assigned = vec![None; channels.len()];
    let mut force_fallback = vec![false; channels.len()];
    let mut used = HashSet::new();

    for (idx, ch) in channels.iter().enumerate() {
        if let Some(number) = override_number(ch, overrides) {
            if used.insert(number) {
                assigned[idx] = Some(number);
            } else {
                eprintln!(
                    "ctc: channel_number collision source=override number={} channel={} channel_code={} action=drop_override",
                    number, ch.channel_name, ch.channel_id
                );
                force_fallback[idx] = true;
            }
        }
    }

    for (idx, ch) in channels.iter().enumerate() {
        if assigned[idx].is_some() || force_fallback[idx] {
            continue;
        }
        let Some(number) = by_user_channel.get(&ch.user_channel_id).copied() else {
            continue;
        };
        if used.insert(number) {
            assigned[idx] = Some(number);
        } else {
            eprintln!(
                "ctc: channel_number collision source=backend_mapping number={} channel={} channel_code={} action=drop_backend_mapping",
                number, ch.channel_name, ch.channel_id
            );
        }
    }

    let mut next = 1;
    for number in &mut assigned {
        if number.is_some() {
            continue;
        }
        while used.contains(&next) {
            next += 1;
        }
        used.insert(next);
        *number = Some(next);
    }

    Ok(assigned.into_iter().map(Option::unwrap).collect())
}

fn override_number(ch: &RawChannel, overrides: &ChannelNumberOverrides) -> Option<u32> {
    overrides.get(&ch.channel_id).map(|entry| entry.number)
}

pub fn fetch_programs(
    provider: &ProviderConfig,
    channel_code: &str,
    date_offset: i32,
) -> Result<Vec<Program>, String> {
    eprintln!(
        "ctc: fetch_programs start user_id={} channel={} date_offset={}",
        provider.user_id, channel_code, date_offset
    );
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
    let url = build_prevue_url(root, channel_code, date_offset, &session.user_id);
    eprintln!(
        "ctc: fetch_programs request channel={} date_offset={}",
        channel_code, date_offset
    );
    let body = get(&url, Some(&format!("JSESSIONID={}", session.jsession_id)))?;
    let programs = parse_programs(&body)?;
    eprintln!(
        "ctc: fetch_programs parsed channel={} date_offset={} count={}",
        channel_code,
        date_offset,
        programs.len()
    );
    Ok(programs)
}

pub fn login(provider: &ProviderConfig) -> Result<LoginSession, String> {
    let auth_base = provider.auth_server_url.trim_end_matches('/');
    eprintln!(
        "ctc: login start user_id={} auth_base={auth_base}",
        provider.user_id
    );
    let login_page = get(
        &format!(
            "{auth_base}/auth?UserID={}&Action=Login",
            encode_query_component(&provider.user_id)
        ),
        None,
    )?;
    let encry_token = parse_encry_token(&login_page).ok_or("EncryToken not found")?;
    eprintln!("ctc: login got_encry_token");
    let authenticator = build_authenticator(
        &provider.user_id,
        &provider.password,
        &provider.stb_id,
        &provider.local_ip,
        &provider.local_mac,
        &encry_token,
        random_8_digit(),
    )?;
    eprintln!("ctc: login built_authenticator");
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
    if config.is_empty() {
        return Err("uploadAuthInfo response had no CTCSetConfig entries".to_string());
    }
    let user_token = config
        .get("UserToken")
        .cloned()
        .ok_or("UserToken missing")?;
    eprintln!("ctc: login got_user_token");
    let service = get(
        &format!("{auth_base}/getServiceList"),
        Some(&format!("UserToken={user_token}")),
    )?;
    eprintln!("ctc: login got_service_list");
    let initial_url = parse_document_location(&service).ok_or("getServiceList redirect missing")?;
    eprintln!("ctc: login got_service_redirect");
    let first = get(&initial_url, None)?;
    let balanced_url =
        parse_document_location(&first).ok_or("EPG load-balanced redirect missing")?;
    eprintln!("ctc: login got_load_balanced_redirect");
    let (portal_html, jsession_id, final_url) = get_with_cookie_capture(&balanced_url)?;
    let epg_lb_base = epg_base_from_url(&final_url);
    eprintln!("ctc: login got_epg_session base={epg_lb_base}");
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
    eprintln!("ctc: login complete hidden_fields={}", hidden.len());
    Ok(LoginSession {
        epg_lb_base,
        jsession_id,
        user_token,
        user_id: provider.user_id.clone(),
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

fn build_prevue_url(root: &str, channel_code: &str, date_offset: i32, user_id: &str) -> String {
    format!(
        "{root}frame1194/CHANNEL_PLAYER_UTILS/datas/prevue_list.jsp?channelcode={}&framecode=frame1194&versiondir=CHANNEL_PLAYER_UTILS&dateindex={date_offset}&stbtype=sdr&recommpara={}&ajax=1",
        encode_query_component(channel_code),
        encode_query_component(&format!("userId={user_id}&channelId=1&num=6"))
    )
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
    let resp = req.call().map_err(|e| format!("GET {url} failed: {e}"))?;
    crate::http_body::response_to_string(resp, &http_context("GET", url))
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
    let body = crate::http_body::response_to_string(resp, &http_context("GET", url))?;
    Ok((body, jsession, final_url))
}

fn post_form(url: &str, fields: &[(&str, &str)], cookie: Option<&str>) -> Result<String, String> {
    let mut req = ureq::post(url);
    if let Some(cookie) = cookie {
        req = req.set("Cookie", cookie);
    }
    let resp = req
        .send_form(fields)
        .map_err(|e| format!("POST {url} failed: {e}"))?;
    crate::http_body::response_to_string(resp, &http_context("POST", url))
}

fn http_context(method: &str, url: &str) -> String {
    let without_query = url.split('?').next().unwrap_or(url);
    let path = without_query
        .split_once("://")
        .and_then(|(_, rest)| rest.find('/').map(|pos| &rest[pos..]))
        .unwrap_or(without_query);
    format!("ctc {method} {path}")
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
    let mut rest = html;
    while let Some((_, after_marker)) = rest.split_once("Authentication.CTCSetConfig") {
        if let Some((key, after_key)) = next_quoted(after_marker) {
            if let Some((value, after_value)) = next_quoted(after_key) {
                out.insert(key, value);
                rest = after_value;
                continue;
            }
        }
        rest = after_marker;
    }
    out
}

fn parse_document_location(html: &str) -> Option<String> {
    let after = html.split_once("document.location")?.1.trim_start();
    let after = after.strip_prefix('=')?.trim_start();
    let quote = after.chars().next()?;
    if quote != '\'' && quote != '"' {
        return None;
    }
    let value = &after[quote.len_utf8()..];
    value.split(quote).next().map(str::to_string)
}

fn parse_hidden_inputs(html: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let lower = html.to_ascii_lowercase();
    let mut idx = 0;
    while let Some(pos) = lower[idx..].find("<input") {
        let start = idx + pos;
        let end = html[start..]
            .find('>')
            .map(|offset| start + offset + 1)
            .unwrap_or(html.len());
        let input = &html[start..end];
        let input_lower = &lower[start..end.min(lower.len())];
        let is_hidden = attr(input, "type")
            .map(|value| value.eq_ignore_ascii_case("hidden"))
            .unwrap_or_else(|| {
                input_lower.contains("type=hidden")
                    || input_lower.contains("type=\"hidden\"")
                    || input_lower.contains("type='hidden'")
            });
        if !is_hidden {
            idx = end;
            continue;
        }
        if let (Some(name), Some(value)) = (attr(input, "name"), attr(input, "value")) {
            out.insert(name, value);
        }
        idx = end;
    }
    out
}

fn parse_channels(html: &str) -> Vec<RawChannel> {
    let mut out = Vec::new();
    let mut rest = html;
    while let Some((_, after_marker)) = rest.split_once("jsSetConfig") {
        let Some((kind, after_kind)) = next_quoted(after_marker) else {
            rest = after_marker;
            continue;
        };
        if kind != "Channel" {
            rest = after_kind;
            continue;
        }
        let Some((body, after_body)) = next_quoted(after_kind) else {
            rest = after_kind;
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
        rest = after_body;
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
    let value = parse_lenient_json_object(json_text)
        .ok_or_else(|| "EPG response is not JSON".to_string())?;
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
    let key = format!("{name}=");
    let start = header.find(&key)? + key.len();
    let rest = &header[start..];
    let end = rest.find(';').unwrap_or(rest.len());
    Some(rest[..end].to_string())
}

fn attr(input: &str, name: &str) -> Option<String> {
    let lower = input.to_ascii_lowercase();
    let key = format!("{}=", name.to_ascii_lowercase());
    let pos = lower.find(&key)?;
    let after = &input[pos + key.len()..];
    let first = after.chars().next()?;
    if first == '\'' || first == '"' {
        let value = &after[first.len_utf8()..];
        return value.split(first).next().map(str::to_string);
    }
    let end = after
        .find(|c: char| c.is_whitespace() || c == '>' || c == '/')
        .unwrap_or(after.len());
    Some(after[..end].to_string())
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
    let (scheme, rest) = without_query
        .split_once("://")
        .unwrap_or(("http", without_query));
    let (authority, path) = match rest.find('/') {
        Some(pos) => (&rest[..pos], &rest[pos..]),
        None => (rest, "/"),
    };
    let dir = path.rfind('/').map(|pos| &path[..pos + 1]).unwrap_or("/");
    format!("{scheme}://{authority}{dir}")
}

fn next_quoted(s: &str) -> Option<(String, &str)> {
    let start = s.find(['\'', '"'])?;
    let quote = s[start..].chars().next()?;
    let body = &s[start + quote.len_utf8()..];
    let end = body.find(quote)?;
    Some((body[..end].to_string(), &body[end + quote.len_utf8()..]))
}

fn parse_lenient_json_object(text: &str) -> Option<serde_json::Value> {
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(text) {
        return value.is_object().then_some(value);
    }
    let start = text.find('{')?;
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escape = false;
    for (offset, ch) in text[start..].char_indices() {
        if in_string {
            if escape {
                escape = false;
            } else if ch == '\\' {
                escape = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }
        match ch {
            '"' => in_string = true,
            '{' => depth += 1,
            '}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    let candidate = &text[start..start + offset + ch.len_utf8()];
                    return serde_json::from_str::<serde_json::Value>(candidate)
                        .ok()
                        .filter(|value| value.is_object());
                }
            }
            _ => {}
        }
    }
    None
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
    use crate::config::ChannelNumberOverride;

    #[test]
    fn authenticator_is_hex_and_validates_rand() {
        let value =
            build_authenticator("u", "000000", "s", "1.2.3.4", "aa", "tok", 12345678).unwrap();
        assert!(value.chars().all(|c| c.is_ascii_hexdigit()));
        assert!(build_authenticator("u", "p", "s", "i", "m", "t", 1).is_err());
    }

    #[test]
    fn parses_ctc_channel_blocks() {
        let html = "jsSetConfig( 'Channel' , 'ChannelID=ch1,ChannelName=News,UserChannelID=001,ChannelURL=igmp://239.0.0.1:8000' )";
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
    fn parses_set_config_with_spaces() {
        let config = parse_set_config(
            "Authentication.CTCSetConfig ( 'UserToken' , 'UT9' );\
             Authentication.CTCSetConfig('EPGDomain','http://e/x')",
        );
        assert_eq!(Some(&"UT9".to_string()), config.get("UserToken"));
        assert_eq!(Some(&"http://e/x".to_string()), config.get("EPGDomain"));
    }

    #[test]
    fn parses_hidden_inputs_case_insensitive_and_unquoted() {
        let html = "<INPUT TYPE=hidden NAME='UserToken' VALUE=UT9>\
                    <input value=ignored type=text name=x>\
                    <input value=abc name=UserID type=HIDDEN>";
        let inputs = parse_hidden_inputs(html);
        assert_eq!(Some(&"UT9".to_string()), inputs.get("UserToken"));
        assert_eq!(Some(&"abc".to_string()), inputs.get("UserID"));
        assert!(!inputs.contains_key("x"));
    }

    #[test]
    fn parses_document_location_with_query_equals() {
        assert_eq!(
            Some("http://h/p?UserToken=abc&foo=bar".to_string()),
            parse_document_location(
                "<script>document.location = 'http://h/p?UserToken=abc&foo=bar';</script>"
            )
        );
        assert_eq!(
            Some("http://h/q?x=1".to_string()),
            parse_document_location(r#"document.location="http://h/q?x=1""#)
        );
    }

    #[test]
    fn parses_cookie_from_combined_set_cookie_header() {
        assert_eq!(
            Some("ABC".to_string()),
            parse_cookie_value(
                "foo=bar; Path=/, JSESSIONID=ABC; Path=/iptvepg",
                "JSESSIONID"
            )
        );
    }

    #[test]
    fn http_context_omits_query_values() {
        assert_eq!(
            "ctc GET /iptvepg/function/index.jsp",
            http_context(
                "GET",
                "http://host:33200/iptvepg/function/index.jsp?UserID=secret"
            )
        );
    }

    #[test]
    fn epg_base_keeps_scheme_authority_and_directory() {
        assert_eq!(
            "http://1.2.3.4:33200/iptvepg/function/",
            epg_base_from_url("http://1.2.3.4:33200/iptvepg/function/index.jsp?x=1")
        );
    }

    #[test]
    fn parse_programs_accepts_wrapped_json() {
        let programs = parse_programs(
            r#"prefix {"channelPrevue":[{"prevuecode":"p1","prevuename":"News","begintime":"20260607080000","endtime":"20260607090000"}]} suffix"#,
        )
        .unwrap();
        assert_eq!("p1", programs[0].code);
    }

    #[test]
    fn fetch_program_url_includes_recommpara() {
        let url = build_prevue_url("http://epg/iptvepg/", "ch=1", 0, "user 1");
        assert!(url.contains("channelcode=ch%3D1"));
        assert!(url.contains("recommpara=userId%3Duser%201%26channelId%3D1%26num%3D6"));
    }

    #[test]
    fn channel_number_overrides_take_precedence_and_fallback_avoids_collisions() {
        let channels = vec![
            RawChannel {
                channel_id: "code-1".into(),
                channel_name: "One".into(),
                user_channel_id: "u1".into(),
                channel_url: "http://one".into(),
            },
            RawChannel {
                channel_id: "code-2".into(),
                channel_name: "Two".into(),
                user_channel_id: "u2".into(),
                channel_url: "http://two".into(),
            },
            RawChannel {
                channel_id: "code-3".into(),
                channel_name: "Three".into(),
                user_channel_id: "u3".into(),
                channel_url: "http://three".into(),
            },
        ];
        let backend = HashMap::from([("u1".to_string(), 10), ("u2".to_string(), 5)]);
        let overrides = HashMap::from([(
            "code-1".to_string(),
            ChannelNumberOverride {
                name: Some("One".into()),
                number: 5,
            },
        )]);

        assert_eq!(
            vec![5, 1, 2],
            assign_channel_numbers(&channels, &backend, &overrides).unwrap()
        );
    }

    #[test]
    fn duplicate_override_numbers_drop_later_override_to_fallback() {
        let channels = vec![
            RawChannel {
                channel_id: "code-1".into(),
                channel_name: "One".into(),
                user_channel_id: "u1".into(),
                channel_url: "http://one".into(),
            },
            RawChannel {
                channel_id: "code-2".into(),
                channel_name: "Two".into(),
                user_channel_id: "u2".into(),
                channel_url: "http://two".into(),
            },
        ];
        let overrides = HashMap::from([
            (
                "code-1".to_string(),
                ChannelNumberOverride {
                    name: Some("One".into()),
                    number: 5,
                },
            ),
            (
                "code-2".to_string(),
                ChannelNumberOverride {
                    name: Some("Two".into()),
                    number: 5,
                },
            ),
        ]);

        assert_eq!(
            vec![5, 1],
            assign_channel_numbers(&channels, &HashMap::new(), &overrides).unwrap()
        );
    }

    #[test]
    fn duplicate_override_number_does_not_fall_through_to_backend_mapping() {
        let channels = vec![
            RawChannel {
                channel_id: "code-1".into(),
                channel_name: "One".into(),
                user_channel_id: "u1".into(),
                channel_url: "http://one".into(),
            },
            RawChannel {
                channel_id: "code-2".into(),
                channel_name: "Two".into(),
                user_channel_id: "u2".into(),
                channel_url: "http://two".into(),
            },
        ];
        let backend = HashMap::from([("u2".to_string(), 9)]);
        let overrides = HashMap::from([
            (
                "code-1".to_string(),
                ChannelNumberOverride {
                    name: None,
                    number: 5,
                },
            ),
            (
                "code-2".to_string(),
                ChannelNumberOverride {
                    name: None,
                    number: 5,
                },
            ),
        ]);

        assert_eq!(
            vec![5, 1],
            assign_channel_numbers(&channels, &backend, &overrides).unwrap()
        );
    }

    #[test]
    fn backend_mapping_conflict_with_override_uses_fallback() {
        let channels = vec![
            RawChannel {
                channel_id: "code-1".into(),
                channel_name: "One".into(),
                user_channel_id: "u1".into(),
                channel_url: "http://one".into(),
            },
            RawChannel {
                channel_id: "code-2".into(),
                channel_name: "Two".into(),
                user_channel_id: "u2".into(),
                channel_url: "http://two".into(),
            },
        ];
        let backend = HashMap::from([("u2".to_string(), 5)]);
        let overrides = HashMap::from([(
            "code-1".to_string(),
            ChannelNumberOverride {
                name: None,
                number: 5,
            },
        )]);

        assert_eq!(
            vec![5, 1],
            assign_channel_numbers(&channels, &backend, &overrides).unwrap()
        );
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
