//! B 站直播流解析。
//!
//! 接口：`GET /xlive/web-room/v2/index/getRoomPlayInfo`
//!
//! **实测结论（2026-11-19，未登录、无 Cookie、无 wbi 签名）：返回 `code=0`。**
//! 因此 v1 不实现 wbi 签名——少一个会随 B 站改版失效的环节。
//! 若将来接口开始要求签名（风控升级），在这里补 wbi 即可，上层无感；
//! 参考实现见 TideCap / streamget 的 `wbi` 模块（含 mixinKey 混淆表）。
//!
//! 两个必须记住的坑：
//! 1. flv 直链**必须带 `Referer: https://live.bilibili.com/`**，否则 CDN 返回 403
//! 2. 请求 `qn=10000`（原画）不代表返回就是原画：接口可能静默降级，
//!    实际画质以 `current_qn` 与 mpv 报告的 `width/height` 为准

use std::fmt;
use std::time::Duration;

use serde::Deserialize;

/// 直播页地址，作为 `Referer` 用。
pub const REFERER: &str = "https://live.bilibili.com/";
/// 常规桌面 UA。B 站对空 UA / 爬虫 UA 有额外风控。
pub const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
(KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";

const API: &str = "https://api.live.bilibili.com/xlive/web-room/v2/index/getRoomPlayInfo";
/// 请求的画质档。10000 = 原画；实际档位见 `StreamUrl::qn`。
const REQUEST_QN: u32 = 10000;
const HTTP_TIMEOUT: Duration = Duration::from_secs(15);

/// 直播流容器/协议。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Protocol {
    /// HTTP-FLV。延迟 1~3 秒，首选。
    Flv,
    /// HLS（m3u8）。延迟 5~15 秒，兜底。
    Hls,
}

impl fmt::Display for Protocol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Flv => f.write_str("flv"),
            Self::Hls => f.write_str("hls"),
        }
    }
}

/// 视频编码。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Codec {
    Avc,
    Hevc,
    Other,
}

impl fmt::Display for Codec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Avc => f.write_str("avc"),
            Self::Hevc => f.write_str("hevc"),
            Self::Other => f.write_str("other"),
        }
    }
}

/// 一条可直接交给 mpv 的流。
#[derive(Debug, Clone, PartialEq)]
pub struct StreamUrl {
    pub url: String,
    pub protocol: Protocol,
    /// 容器名（`flv` / `ts` / `fmp4`），仅用于展示与排错
    pub format: String,
    /// 接口实际返回的画质档（可能是降级后的值）
    pub qn: u32,
    pub codec: Codec,
    /// 必须随请求带上的头（至少 Referer 与 User-Agent）
    pub headers: Vec<(String, String)>,
}

impl StreamUrl {
    /// 取某个请求头的值（大小写不敏感）。
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// 解析结果。未开播时 `live == false` 且 `stream == None`——**这不是错误**，
/// 界面应显示灰格而不是报错。
#[derive(Debug, Clone)]
pub struct Resolved {
    pub room_id: u64,
    pub live: bool,
    pub title: String,
    pub uname: String,
    /// 全部候选（按优选度排序），排错时用
    pub candidates: Vec<StreamUrl>,
    /// 选中的流。开播但没有可用流时为 `None`
    pub stream: Option<StreamUrl>,
}

#[derive(Debug, Clone)]
pub enum ResolveError {
    /// 输入里找不到房间号
    BadInput(String),
    /// 网络层失败
    Http(String),
    /// 接口返回 code != 0
    Api { code: i64, message: String },
    /// 开播但没有可用流（通常是接口异常或该房间仍需登录）
    NoStream { room_id: u64 },
}

impl fmt::Display for ResolveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BadInput(s) => write!(f, "无法识别房间号：{s}"),
            Self::Http(s) => write!(f, "请求 B 站接口失败：{s}"),
            Self::Api { code, message } => write!(f, "B 站接口返回错误：code={code} {message}"),
            Self::NoStream { room_id } => write!(f, "房间 {room_id} 正在直播但没有可用流"),
        }
    }
}

impl std::error::Error for ResolveError {}

/// 从房间号 / 直播间链接 / 带标题的粘贴文本里提取房间号。
///
/// 支持：`7734200`、`https://live.bilibili.com/7734200?x=1`、
/// `live.bilibili.com/7734200`、`https://live.bilibili.com/blanc/7734200`。
pub fn normalize_room(input: &str) -> Result<u64, ResolveError> {
    let text = input.trim();
    if text.is_empty() {
        return Err(ResolveError::BadInput("输入为空".into()));
    }

    // 先看是不是纯数字
    if let Ok(n) = text.parse::<u64>() {
        if n > 0 {
            return Ok(n);
        }
    }

    // URL：取 `bilibili.com/` 之后的第一段数字
    let lower = text.to_ascii_lowercase();
    if let Some(idx) = lower.find("bilibili.com") {
        let rest = &text[idx + "bilibili.com".len()..];
        let rest = rest.trim_start_matches('/');
        if let Some(n) = first_number(rest) {
            return Ok(n);
        }
    }

    // 兜底：文本里第一段数字（应对「【标题】 7734200」这类粘贴）
    first_number(text).ok_or_else(|| ResolveError::BadInput(text.chars().take(60).collect()))
}

/// 找到字符串里第一段连续数字（去掉前导 0 后必须 > 0）。
fn first_number(s: &str) -> Option<u64> {
    let mut digits = String::new();
    for ch in s.chars() {
        if ch.is_ascii_digit() {
            digits.push(ch);
        } else if !digits.is_empty() {
            break;
        }
    }
    let n: u64 = digits.parse().ok()?;
    (n > 0).then_some(n)
}

/// 解析一个房间：房间号或链接都行。
pub fn resolve(input: &str) -> Result<Resolved, ResolveError> {
    let room_id = normalize_room(input)?;
    let url = format!(
        "{API}?room_id={room_id}&protocol=0,1&format=0,1,2&codec=0,1&qn={REQUEST_QN}&platform=web&ptype=8&dolby=5&panorama=1"
    );

    let text = http_get(&url)?;
    let resp: ApiResp =
        serde_json::from_str(&text).map_err(|e| ResolveError::Http(format!("响应不是预期 JSON：{e}")))?;

    if resp.code != 0 {
        return Err(ResolveError::Api {
            code: resp.code,
            message: resp.message,
        });
    }

    let data = resp.data.unwrap_or_default();
    let room_id = if data.room_id > 0 {
        data.room_id
    } else {
        room_id
    };
    let live = data.live_status == 1;

    let mut candidates = Vec::new();
    if let Some(info) = &data.playurl_info {
        if let Some(playurl) = &info.playurl {
            for stream in &playurl.stream {
                let protocol = match stream.protocol_name.as_str() {
                    "http_stream" => Protocol::Flv,
                    "http_hls" => Protocol::Hls,
                    _ => continue,
                };
                for format in &stream.format {
                    for codec in &format.codec {
                        if codec.base_url.is_empty() {
                            continue;
                        }
                        let info = codec.url_info.first();
                        let host = info.map(|u| u.host.as_str()).unwrap_or_default();
                        let extra = info.map(|u| u.extra.as_str()).unwrap_or_default();
                        let url = join_url(host, &codec.base_url, extra);
                        if !url.starts_with("http") {
                            continue;
                        }
                        candidates.push(StreamUrl {
                            url,
                            protocol,
                            format: format.format_name.clone(),
                            qn: codec.current_qn,
                            codec: parse_codec(&codec.codec_name),
                            headers: default_headers(),
                        });
                    }
                }
            }
        }
    }

    // 优选：容器/协议（flv 优先）→ 画质档（高优先）→ 编码（AVC 兼容性最好）
    candidates.sort_by_key(|s| std::cmp::Reverse(rank(s)));

    let stream = candidates.first().cloned();
    let title = data
        .room_info
        .map(|r| r.title)
        .unwrap_or_default();
    let uname = data
        .anchor_info
        .and_then(|a| a.base_info)
        .map(|b| b.uname)
        .unwrap_or_default();

    if live && stream.is_none() {
        return Err(ResolveError::NoStream { room_id });
    }

    Ok(Resolved {
        room_id,
        live,
        title,
        uname,
        candidates,
        stream,
    })
}

fn rank(s: &StreamUrl) -> (u8, u32, u8) {
    let proto = match s.protocol {
        Protocol::Flv => 2,
        Protocol::Hls => 1,
    };
    let codec = match s.codec {
        Codec::Avc => 2,
        Codec::Hevc => 1,
        Codec::Other => 0,
    };
    (proto, s.qn, codec)
}

fn parse_codec(name: &str) -> Codec {
    match name {
        "avc" => Codec::Avc,
        "hevc" => Codec::Hevc,
        _ => Codec::Other,
    }
}

fn default_headers() -> Vec<(String, String)> {
    vec![
        ("Referer".to_string(), REFERER.to_string()),
        ("User-Agent".to_string(), USER_AGENT.to_string()),
    ]
}

/// 拼出可访问的直链：`host + base_url + extra`。
///
/// **坑**：接口返回的 `base_url` 只到 `.flv?` 为止，签名/时效参数全在
/// `url_info[].extra` 里。少了 extra 一定是 403——2026-11-19 实测踩过。
fn join_url(host: &str, base_url: &str, extra: &str) -> String {
    // base_url 偶尔已是完整地址（带 host）
    let mut url = if base_url.starts_with("http") {
        base_url.to_string()
    } else {
        format!("{host}{base_url}")
    };

    if extra.is_empty() {
        return url;
    }

    // extra 可能自带 ? 或 &（不同接口版本不一致），避免拼出 `??` / `?&`
    let extra = extra.trim_start_matches(['?', '&']);
    if url.ends_with('?') || url.ends_with('&') {
        url.push_str(extra);
    } else if url.contains('?') {
        url.push('&');
        url.push_str(extra);
    } else {
        url.push('?');
        url.push_str(extra);
    }
    url
}

fn http_get(url: &str) -> Result<String, ResolveError> {
    let resp = ureq::get(url)
        .set("User-Agent", USER_AGENT)
        .set("Referer", REFERER)
        .set("Origin", "https://live.bilibili.com")
        .set("Accept", "application/json, text/plain, */*")
        .timeout(HTTP_TIMEOUT)
        .call();

    match resp {
        Ok(r) => r
            .into_string()
            .map_err(|e| ResolveError::Http(format!("读取响应失败：{e}"))),
        Err(ureq::Error::Status(code, _)) => {
            Err(ResolveError::Http(format!("HTTP {code}")))
        }
        Err(e) => Err(ResolveError::Http(e.to_string())),
    }
}

// ---------- 接口响应模型（只声明用得到的字段） ----------

#[derive(Debug, Deserialize)]
struct ApiResp {
    code: i64,
    #[serde(default)]
    message: String,
    #[serde(default)]
    data: Option<RoomData>,
}

#[derive(Debug, Default, Deserialize)]
struct RoomData {
    #[serde(default)]
    room_id: u64,
    /// 0 = 未开播, 1 = 直播中, 2 = 轮播
    #[serde(default)]
    live_status: i64,
    #[serde(default)]
    playurl_info: Option<PlayurlInfo>,
    #[serde(default)]
    room_info: Option<RoomInfo>,
    #[serde(default)]
    anchor_info: Option<AnchorInfo>,
}

#[derive(Debug, Deserialize)]
struct RoomInfo {
    #[serde(default)]
    title: String,
}

#[derive(Debug, Deserialize)]
struct AnchorInfo {
    #[serde(default)]
    base_info: Option<AnchorBaseInfo>,
}

#[derive(Debug, Deserialize)]
struct AnchorBaseInfo {
    #[serde(default)]
    uname: String,
}

#[derive(Debug, Deserialize)]
struct PlayurlInfo {
    #[serde(default)]
    playurl: Option<Playurl>,
}

#[derive(Debug, Deserialize)]
struct Playurl {
    #[serde(default)]
    stream: Vec<StreamInfo>,
}

#[derive(Debug, Deserialize)]
struct StreamInfo {
    #[serde(default)]
    protocol_name: String,
    #[serde(default)]
    format: Vec<FormatInfo>,
}

#[derive(Debug, Deserialize)]
struct FormatInfo {
    #[serde(default)]
    format_name: String,
    #[serde(default)]
    codec: Vec<CodecInfo>,
}

#[derive(Debug, Deserialize)]
struct CodecInfo {
    #[serde(default)]
    codec_name: String,
    #[serde(default)]
    current_qn: u32,
    #[serde(default)]
    base_url: String,
    #[serde(default)]
    url_info: Vec<UrlInfo>,
}

#[derive(Debug, Deserialize)]
struct UrlInfo {
    #[serde(default)]
    host: String,
    /// 查询串（签名、expires、qn 等都在这里）
    #[serde(default)]
    extra: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_accepts_bare_number() {
        assert_eq!(normalize_room("7734200").unwrap(), 7_734_200);
        assert_eq!(normalize_room("  21452505  ").unwrap(), 21_452_505);
    }

    #[test]
    fn normalize_accepts_urls() {
        assert_eq!(
            normalize_room("https://live.bilibili.com/7734200?broadcast_type=0").unwrap(),
            7_734_200
        );
        assert_eq!(normalize_room("live.bilibili.com/7734200").unwrap(), 7_734_200);
        assert_eq!(
            normalize_room("https://live.bilibili.com/blanc/7734200").unwrap(),
            7_734_200
        );
        assert_eq!(
            normalize_room("http://LIVE.BILIBILI.COM/6/").unwrap(),
            6
        );
    }

    #[test]
    fn normalize_accepts_pasted_text_with_title() {
        assert_eq!(
            normalize_room("【直播】看看这个 https://live.bilibili.com/12345?x=1").unwrap(),
            12_345
        );
    }

    #[test]
    fn normalize_rejects_garbage() {
        assert!(matches!(normalize_room(""), Err(ResolveError::BadInput(_))));
        assert!(matches!(normalize_room("   "), Err(ResolveError::BadInput(_))));
        assert!(matches!(normalize_room("没有数字"), Err(ResolveError::BadInput(_))));
        // 0 不是合法房间号
        assert!(matches!(normalize_room("0"), Err(ResolveError::BadInput(_))));
    }

    #[test]
    fn ranking_prefers_flv_then_quality_then_avc() {
        let make = |protocol, qn, codec| StreamUrl {
            url: "http://x".into(),
            protocol,
            format: "f".into(),
            qn,
            codec,
            headers: vec![],
        };
        let flv_low = make(Protocol::Flv, 250, Codec::Avc);
        let flv_high = make(Protocol::Flv, 10_000, Codec::Avc);
        let hls_high = make(Protocol::Hls, 10_000, Codec::Avc);
        let flv_hevc = make(Protocol::Flv, 10_000, Codec::Hevc);

        assert!(rank(&flv_low) > rank(&hls_high), "低画质 flv 仍优于高画质 hls");
        assert!(rank(&flv_high) > rank(&flv_low));
        assert!(rank(&flv_high) > rank(&flv_hevc));
    }

    #[test]
    fn header_lookup_is_case_insensitive() {
        let s = StreamUrl {
            url: "http://x".into(),
            protocol: Protocol::Flv,
            format: "flv".into(),
            qn: 250,
            codec: Codec::Avc,
            headers: vec![("Referer".into(), REFERER.into())],
        };
        assert_eq!(s.header("referer"), Some(REFERER));
        assert_eq!(s.header("Referer"), Some(REFERER));
        assert_eq!(s.header("User-Agent"), None);
    }

    #[test]
    fn join_url_appends_extra_query() {
        // 实测形态：base_url 以 `?` 结尾，extra 不带前导符号
        let url = join_url(
            "https://d1--x.bilivideo.com",
            "/live-bvc/1/live_1.flv?",
            "expires=1&qn=250&pt=web",
        );
        assert_eq!(
            url,
            "https://d1--x.bilivideo.com/live-bvc/1/live_1.flv?expires=1&qn=250&pt=web"
        );
    }

    #[test]
    fn join_url_handles_all_query_shapes() {
        // base_url 无 `?`
        assert_eq!(
            join_url("https://h", "/a.flv", "x=1"),
            "https://h/a.flv?x=1"
        );
        // base_url 已有参数
        assert_eq!(
            join_url("https://h", "/a.flv?y=2", "x=1"),
            "https://h/a.flv?y=2&x=1"
        );
        // extra 自带 `?` / `&`，不能拼出 `??` / `?&`
        assert_eq!(
            join_url("https://h", "/a.flv?", "?x=1"),
            "https://h/a.flv?x=1"
        );
        assert_eq!(
            join_url("https://h", "/a.flv?", "&x=1"),
            "https://h/a.flv?x=1"
        );
        // 没有 extra：保持原样，不加多余的 `?`
        assert_eq!(join_url("https://h", "/a.flv?", ""), "https://h/a.flv?");
        // base_url 自带 host 时忽略 host 参数
        assert_eq!(
            join_url("https://ignored", "https://real/a.flv", "x=1"),
            "https://real/a.flv?x=1"
        );
    }
}
