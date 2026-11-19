//! `em-smoke` — M0 自测 CLI。
//!
//! 目的：在**没有窗口、没有 UI** 的情况下验证「解析层 + 播放内核」整条链路。
//! 新机器上第一次跑通项目就是跑这个：能出声，说明 libmpv 加载、B 站解析、
//! 请求头、解码全部正常，可以往上做 M1 的渲染了。
//!
//! 用法见 `--help`，或 `docs/dev-guide.md`。

use std::process::ExitCode;
use std::time::{Duration, Instant};

use em_player::{EndReason, Event, Lib, Player, PlayerOptions};
use em_resolve::{ResolveError, resolve};

const HELP: &str = "\
em-smoke — ElderMonitor M0 自测

用法：
  em-smoke <房间号|直播间链接> [选项]
  em-smoke --check-lib

选项：
  --seconds <秒>   播放时长，默认 12
  --volume <0-100> 音量，默认 40
  --mute           静音启动（仍会解码，用来测「只看画面」的场景）
  --no-play        只解析，不加载播放器
  --check-lib      只检查 libmpv 能否加载，并列出查找过的路径
  -h, --help       显示本帮助

退出码：
  0 通过    1 用法错误   2 解析失败   3 未开播
  4 播放器错误          5 播放未达标（没加载/没音频/中途断流）

环境变量：
  EM_MPV=<动态库完整路径>   指定 libmpv，覆盖自动查找
";

#[derive(Debug)]
struct Args {
    target: Option<String>,
    seconds: f64,
    volume: u8,
    muted: bool,
    check_lib: bool,
    no_play: bool,
}

fn main() -> ExitCode {
    #[cfg(windows)]
    ensure_utf8_console();

    // 帮助单独处理：正常退出，不当成错误
    if std::env::args().any(|a| a == "-h" || a == "--help") {
        println!("{HELP}");
        return ExitCode::SUCCESS;
    }

    let args = match parse_args(std::env::args().skip(1)) {
        Ok(args) => args,
        Err(msg) => {
            eprintln!("{msg}\n\n{HELP}");
            return ExitCode::from(1);
        }
    };

    if args.check_lib {
        return check_lib();
    }

    let Some(target) = args.target.as_deref() else {
        eprintln!("缺少房间号或链接。\n\n{HELP}");
        return ExitCode::from(1);
    };

    let Some(code) = run(target, &args) else {
        return ExitCode::from(1);
    };
    code
}

fn run(target: &str, args: &Args) -> Option<ExitCode> {
    // ---------- 1. 解析 ----------
    println!("[1/3] 解析 {target}");
    let resolved = match resolve(target) {
        Ok(r) => r,
        Err(e) => {
            println!("      失败   {e}");
            if matches!(e, ResolveError::Http(_)) {
                println!("      提示   检查网络/代理；接口不可达时也会走到这里");
            }
            return Some(ExitCode::from(2));
        }
    };

    println!(
        "      房间   {} · 主播 {}",
        resolved.room_id,
        if resolved.uname.is_empty() {
            "未知"
        } else {
            &resolved.uname
        }
    );
    if !resolved.title.is_empty() {
        println!("      标题   {}", resolved.title);
    }

    let cands: Vec<String> = resolved
        .candidates
        .iter()
        .map(|c| format!("{}/{} qn={}", c.protocol, c.codec, c.qn))
        .collect();
    if !cands.is_empty() {
        println!("      候选   {}", cands.join(" · "));
    }

    if !resolved.live {
        println!("      状态   未开播（这不是错误：界面应显示灰格并定时重试）");
        return Some(ExitCode::from(3));
    }

    let Some(stream) = resolved.stream.clone() else {
        println!("      状态   直播中但没有可用流");
        return Some(ExitCode::from(2));
    };

    println!(
        "      状态   直播中 → 选中 {}/{} qn={}（请求档位 10000 = 原画）",
        stream.protocol, stream.codec, stream.qn
    );
    println!("      地址   {}", truncate(&stream.url, 96));

    if stream.qn < 400 {
        println!(
            "      注意   接口把画质降到了 qn={}（未登录状态下可能如此），实际分辨率以下面 mpv 报告为准",
            stream.qn
        );
    }

    if args.no_play {
        println!("[2/3] 跳过（--no-play）");
        println!("[3/3] 跳过（--no-play）");
        return Some(ExitCode::SUCCESS);
    }

    // ---------- 2. 加载 libmpv ----------
    println!("[2/3] 加载 libmpv");
    let lib = match Lib::load_default() {
        Ok(lib) => lib,
        Err(e) => {
            println!("      失败   {e}");
            return Some(ExitCode::from(4));
        }
    };
    let (api_major, api_minor) = lib.client_api_version();
    println!(
        "      client API {api_major}.{api_minor} · {}",
        if cfg!(windows) {
            "libmpv-2.dll"
        } else {
            "libmpv.2.dylib"
        }
    );

    // ---------- 3. 播放 ----------
    println!(
        "[3/3] 播放 {:.0}s（vo=null，仅音频，音量 {}{}）",
        args.seconds,
        args.volume,
        if args.muted { "，静音" } else { "" }
    );

    let player = match Player::new(lib, &PlayerOptions::audio_only()) {
        Ok(p) => p,
        Err(e) => {
            println!("      失败   {e}");
            return Some(ExitCode::from(4));
        }
    };    for w in player.warnings() {
        println!("      警告   {w}");
    }

    if let Err(e) = player.set_volume(args.volume) {
        println!("      警告   设置音量失败：{e}");
    }
    if args.muted {
        let _ = player.set_muted(true);
    }

    if let Err(e) = player.load(&stream) {
        println!("      失败   加载流：{e}");
        return Some(ExitCode::from(4));
    }

    let deadline = Instant::now() + Duration::from_secs_f64(args.seconds);
    let mut file_loaded = false;
    let mut audio_events = 0usize;
    let mut ended: Option<(i32, i32)> = None;
    let mut logs: Vec<String> = Vec::new();

    while Instant::now() < deadline {
        let remain = deadline
            .saturating_duration_since(Instant::now())
            .as_secs_f64()
            .min(1.0);
        match player.wait_event(remain) {
            Event::FileLoaded => {
                if !file_loaded {
                    file_loaded = true;
                    println!("      事件   file-loaded");
                }
            }
            Event::AudioReconfig => {
                audio_events += 1;
                if audio_events == 1 {
                    println!("      事件   audio-reconfig（音频轨已建立）");
                }
            }
            Event::EndFile { reason, error } => {
                println!(
                    "      事件   end-file（{}，error={}）",
                    EndReason::from_code(reason),
                    error
                );
                ended = Some((reason, error));
                break;
            }
            Event::Log {
                level,
                prefix,
                text,
            } => {
                if (level == "error" || level == "warn") && logs.len() < 8 {
                    logs.push(format!("[{level}] {prefix}: {text}"));
                }
            }
            Event::Shutdown => break,
            _ => {}
        }
    }

    // 采样实际元数据
    let width = player.get_i64("width").unwrap_or(0);
    let height = player.get_i64("height").unwrap_or(0);
    let audio_codec = player.get_string("audio-codec-name").unwrap_or_default();
    let sample_rate = player.get_i64("audio-samplerate").unwrap_or(0);
    let time_pos = player.get_f64("time-pos").unwrap_or(0.0);
    drop(player);

    if !logs.is_empty() {
        println!("      日志   （mpv warn/error，最多 8 条）");
        for l in &logs {
            println!("             {l}");
        }
    }

    println!(
        "      实际   视频 {} · 音频 {}{} · 播放进度 {:.1}s",
        if width > 0 && height > 0 {
            format!("{width}x{height}")
        } else {
            "无".to_string()
        },
        if audio_codec.is_empty() {
            "无".to_string()
        } else {
            audio_codec.clone()
        },
        if sample_rate > 0 {
            format!(" {sample_rate}Hz")
        } else {
            String::new()
        },
        time_pos
    );

    // ---------- 结论 ----------
    let has_media =
        file_loaded && (time_pos > 0.0 || width > 0 || audio_events > 0 || !audio_codec.is_empty());
    let ok = match ended {
        Some((reason, _)) => {
            // 直播流在观察窗口内 EOF/报错 → 判定失败（M1 要做断流自续）
            !EndReason::from_code(reason).needs_reconnect()
        }
        None => has_media,
    };

    if ok {
        println!("      结论   PASS");
        Some(ExitCode::SUCCESS)
    } else {
        println!("      结论   FAIL（未加载 / 无音视频 / 观察期内断流）");
        Some(ExitCode::from(5))
    }
}

fn check_lib() -> ExitCode {
    println!("检查 libmpv");
    let candidates = em_player::candidate_paths();
    println!("查找顺序：");
    for (i, p) in candidates.iter().enumerate() {
        println!(
            "  {}. {} {}",
            i + 1,
            if p.is_file() { "存在  " } else { "缺失  " },
            p.display()
        );
    }

    match Lib::load_default() {
        Ok(lib) => {
            let (major, minor) = lib.client_api_version();
            println!("结果：可加载，client API {major}.{minor}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            println!("结果：{e}");
            ExitCode::from(4)
        }
    }
}

fn parse_args<I: Iterator<Item = String>>(mut it: I) -> Result<Args, String> {
    let mut args = Args {
        target: None,
        seconds: 12.0,
        volume: 40,
        muted: false,
        check_lib: false,
        no_play: false,
    };

    while let Some(arg) = it.next() {
        match arg.as_str() {
            "-h" | "--help" => return Err(String::new()),
            "--check-lib" => args.check_lib = true,
            "--no-play" => args.no_play = true,
            "--mute" => args.muted = true,
            "--seconds" => {
                let v = it.next().ok_or("--seconds 缺少参数")?;
                let n: f64 = v.parse().map_err(|_| "--seconds 需要数字")?;
                if !(0.0..=3600.0).contains(&n) {
                    return Err("--seconds 需在 0..=3600".into());
                }
                args.seconds = n;
            }
            "--volume" => {
                let v = it.next().ok_or("--volume 缺少参数")?;
                args.volume = v.parse().map_err(|_| "--volume 需要 0..=100 的整数")?;
                if args.volume > 100 {
                    return Err("--volume 不能超过 100".into());
                }
            }
            other if other.starts_with('-') => {
                return Err(format!("未知选项：{other}"));
            }
            other => {
                if args.target.is_some() {
                    return Err("只能指定一个房间".into());
                }
                args.target = Some(other.to_string());
            }
        }
    }

    Ok(args)
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let head: String = s.chars().take(max).collect();
        format!("{head}…")
    }
}

/// Windows 控制台默认代码页多为 936，直接输出 UTF-8 中文会乱码。
#[cfg(windows)]
fn ensure_utf8_console() {
    unsafe extern "system" {
        fn SetConsoleOutputCP(code_page: u32) -> i32;
        fn SetConsoleCP(code_page: u32) -> i32;
    }
    unsafe {
        SetConsoleOutputCP(65001);
        SetConsoleCP(65001);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Args, String> {
        parse_args(args.iter().map(|s| s.to_string()))
    }

    #[test]
    fn parses_positional_target() {
        let a = parse(&["7734200"]).unwrap();
        assert_eq!(a.target.as_deref(), Some("7734200"));
        assert_eq!(a.volume, 40);
        assert_eq!(a.seconds, 12.0);
    }

    #[test]
    fn parses_flags() {
        let a = parse(&["--volume", "85", "--seconds", "30", "--mute", "6"]).unwrap();
        assert_eq!(a.volume, 85);
        assert_eq!(a.seconds, 30.0);
        assert!(a.muted);
        assert_eq!(a.target.as_deref(), Some("6"));
    }

    #[test]
    fn rejects_bad_input() {
        assert!(parse(&["--volume", "101"]).is_err());
        assert!(parse(&["--seconds", "-1"]).is_err());
        assert!(parse(&["--nope"]).is_err());
        assert!(parse(&["1", "2"]).is_err());
        assert!(parse(&["--volume"]).is_err());
    }

    #[test]
    fn truncates_long_urls() {
        assert_eq!(truncate("abc", 10), "abc");
        assert_eq!(truncate("abcdefghij", 5), "abcde…");
    }
}
