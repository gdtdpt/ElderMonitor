//! `em-player` — libmpv 封装。
//!
//! 设计要点（也是踩坑记录）：
//!
//! 1. **运行时动态加载，不链接导入库**。`libmpv-2.dll`（117MB）不参与链接，
//!    用 `libloading` 在启动时按候选路径查找。好处：没有 .lib / .dll.a 的
//!    MSVC-mingw 兼容问题、macOS 上同一套代码、dll 缺失时能给出可读的错误
//!    而不是「程序无法启动」。
//!
//! 2. **Referer 用 mpv 的 `referrer` 属性，不用 `http-header-fields`**。
//!    后者是 StringList，按逗号分割取值，而 UA 里就带逗号
//!    （`Mozilla/5.0 (Windows NT 10.0; Win64; x64) ...`），会被切坏。
//!    `referrer` / `user-agent` 是 mpv 原生选项，一个字符串一个值，无歧义。
//!
//! 3. 每路直播 = 一个 [`Player`]（一个 mpv 实例），音量/静音是实例属性，
//!    天然互相独立（`PROJECT.md` §4.4）。

mod ffi;
mod render;

pub use render::{RenderContext, UpdateCallback};

use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int, c_ulong, c_void};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use libloading::Library;

use em_resolve::StreamUrl;

/// 各平台的动态库文件名。
const DLL_NAME: &str = if cfg!(windows) {
    "libmpv-2.dll"
} else {
    "libmpv.2.dylib"
};

type FnCreate = unsafe extern "C" fn() -> *mut ffi::mpv_handle;
type FnInitialize = unsafe extern "C" fn(*mut ffi::mpv_handle) -> c_int;
type FnTerminateDestroy = unsafe extern "C" fn(*mut ffi::mpv_handle);
type FnSetOptionString =
    unsafe extern "C" fn(*mut ffi::mpv_handle, *const c_char, *const c_char) -> c_int;
type FnSetProperty =
    unsafe extern "C" fn(*mut ffi::mpv_handle, *const c_char, c_int, *mut c_void) -> c_int;
type FnSetPropertyString =
    unsafe extern "C" fn(*mut ffi::mpv_handle, *const c_char, *const c_char) -> c_int;
type FnGetProperty =
    unsafe extern "C" fn(*mut ffi::mpv_handle, *const c_char, c_int, *mut c_void) -> c_int;
type FnGetPropertyString = unsafe extern "C" fn(*mut ffi::mpv_handle, *const c_char) -> *mut c_char;
type FnCommand = unsafe extern "C" fn(*mut ffi::mpv_handle, *const *const c_char) -> c_int;
type FnWaitEvent = unsafe extern "C" fn(*mut ffi::mpv_handle, f64) -> *mut ffi::mpv_event;
type FnErrorString = unsafe extern "C" fn(c_int) -> *const c_char;
type FnClientApiVersion = unsafe extern "C" fn() -> c_ulong;
type FnFree = unsafe extern "C" fn(*mut c_void);
type FnRequestLogMessages = unsafe extern "C" fn(*mut ffi::mpv_handle, *const c_char) -> c_int;

// ---- render API（render.h；M1 起使用）----
type FnRenderContextCreate = unsafe extern "C" fn(
    *mut *mut ffi::mpv_render_context,
    *mut ffi::mpv_handle,
    *mut ffi::mpv_render_param,
) -> c_int;
type FnRenderContextFree = unsafe extern "C" fn(*mut ffi::mpv_render_context);
type FnRenderContextRender =
    unsafe extern "C" fn(*mut ffi::mpv_render_context, *mut ffi::mpv_render_param) -> c_int;
type FnRenderContextSetUpdateCallback = unsafe extern "C" fn(
    *mut ffi::mpv_render_context,
    Option<unsafe extern "C" fn(*mut c_void)>,
    *mut c_void,
);
type FnRenderContextUpdate = unsafe extern "C" fn(*mut ffi::mpv_render_context) -> u64;
type FnRenderContextReportSwap = unsafe extern "C" fn(*mut ffi::mpv_render_context);

/// 播放器层错误。
#[derive(Debug)]
pub enum PlayerError {
    /// 找不到 / 加载不了动态库
    Library { tried: Vec<String>, detail: String },
    /// mpv 调用返回负值
    Mpv {
        call: &'static str,
        code: i32,
        message: String,
    },
    /// 字符串里含 NUL，无法转成 C 字符串
    BadString(String),
}

impl std::fmt::Display for PlayerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Library { tried, detail } => {
                writeln!(f, "无法加载 libmpv（{detail}）。已尝试：")?;
                for p in tried {
                    writeln!(f, "  - {p}")?;
                }
                write!(
                    f,
                    "请运行 tools/fetch-mpv.ps1（Windows）或 tools/fetch-mpv.sh（macOS）"
                )
            }
            Self::Mpv {
                call,
                code,
                message,
            } => write!(f, "{call} 失败：{code} {message}"),
            Self::BadString(s) => write!(f, "非法字符串：{s}"),
        }
    }
}

impl std::error::Error for PlayerError {}

/// 已加载的 libmpv 与其函数表。用 `Arc` 共享给多个 [`Player`]。
pub struct Lib {
    /// 字段顺序即 Drop 顺序：函数表先失效，然后才卸载库
    api: Api,
    _lib: Library,
}

struct Api {
    create: FnCreate,
    initialize: FnInitialize,
    terminate_destroy: FnTerminateDestroy,
    set_option_string: FnSetOptionString,
    set_property: FnSetProperty,
    set_property_string: FnSetPropertyString,
    get_property: FnGetProperty,
    get_property_string: FnGetPropertyString,
    command: FnCommand,
    wait_event: FnWaitEvent,
    error_string: FnErrorString,
    client_api_version: FnClientApiVersion,
    free: FnFree,
    request_log_messages: FnRequestLogMessages,
    // render API
    render_context_create: FnRenderContextCreate,
    render_context_free: FnRenderContextFree,
    render_context_render: FnRenderContextRender,
    render_context_set_update_callback: FnRenderContextSetUpdateCallback,
    render_context_update: FnRenderContextUpdate,
    render_context_report_swap: FnRenderContextReportSwap,
}

impl Lib {
    /// 按候选路径查找并加载 libmpv。
    ///
    /// 查找顺序：`EM_MPV` 环境变量 → exe 同目录 → 从 exe 位置向上找
    /// `vendor/mpv/`（覆盖 `target/debug|release`）→ 当前目录的 `vendor/mpv/`
    /// → 系统默认搜索路径（macOS 额外试 Homebrew 的两个前缀）。
    pub fn load_default() -> Result<Arc<Self>, PlayerError> {
        let candidates = candidate_paths();
        let mut tried = Vec::new();
        for path in &candidates {
            if !path.is_file() {
                tried.push(format!("{}（不存在）", path.display()));
                continue;
            }
            match Self::load_path(path) {
                Ok(lib) => return Ok(lib),
                Err(e) => tried.push(format!("{}（{e}）", path.display())),
            }
        }

        // 最后再试系统搜索路径（dll 在 PATH / 系统库目录的情况）
        match unsafe { Library::new(DLL_NAME) } {
            Ok(lib) => {
                let api = unsafe { Api::load(&lib) }?;
                let lib = Self { api, _lib: lib };
                return Ok(Arc::new(lib));
            }
            Err(e) => tried.push(format!("{DLL_NAME}（系统搜索路径：{e}）")),
        }

        Err(PlayerError::Library {
            tried,
            detail: "未找到可用的 libmpv".to_string(),
        })
    }

    /// 加载指定路径的动态库。
    pub fn load_path(path: &Path) -> Result<Arc<Self>, PlayerError> {
        let lib = unsafe { Library::new(path) }.map_err(|e| PlayerError::Library {
            tried: vec![path.display().to_string()],
            detail: e.to_string(),
        })?;
        let api = unsafe { Api::load(&lib) }?;
        Ok(Arc::new(Self { api, _lib: lib }))
    }

    /// libmpv 客户端 API 版本，形如 `(2, 3)`。
    // `c_ulong` 在 Windows 是 u32、在 macOS/Linux 是 u64，所以这两个 `as u32` 是跨平台必需，
    // 只是 Windows 上看起来多余。
    #[allow(clippy::unnecessary_cast)]
    pub fn client_api_version(&self) -> (u32, u32) {
        let v = unsafe { (self.api.client_api_version)() };
        ((v >> 16) as u32, (v & 0xFFFF) as u32)
    }

    /// 动态库路径（排错用）。
    pub fn path_hint(&self) -> &'static str {
        DLL_NAME
    }
}

impl Api {
    unsafe fn load(lib: &Library) -> Result<Self, PlayerError> {
        unsafe fn sym<T: Copy>(lib: &Library, name: &[u8]) -> Result<T, PlayerError> {
            let s = unsafe { lib.get::<T>(name) }.map_err(|e| PlayerError::Library {
                tried: vec![String::from_utf8_lossy(name).to_string()],
                detail: format!("缺少导出符号：{e}"),
            })?;
            Ok(*s)
        }

        Ok(Self {
            create: unsafe { sym(lib, b"mpv_create\0")? },
            initialize: unsafe { sym(lib, b"mpv_initialize\0")? },
            terminate_destroy: unsafe { sym(lib, b"mpv_terminate_destroy\0")? },
            set_option_string: unsafe { sym(lib, b"mpv_set_option_string\0")? },
            set_property: unsafe { sym(lib, b"mpv_set_property\0")? },
            set_property_string: unsafe { sym(lib, b"mpv_set_property_string\0")? },
            get_property: unsafe { sym(lib, b"mpv_get_property\0")? },
            get_property_string: unsafe { sym(lib, b"mpv_get_property_string\0")? },
            command: unsafe { sym(lib, b"mpv_command\0")? },
            wait_event: unsafe { sym(lib, b"mpv_wait_event\0")? },
            error_string: unsafe { sym(lib, b"mpv_error_string\0")? },
            client_api_version: unsafe { sym(lib, b"mpv_client_api_version\0")? },
            free: unsafe { sym(lib, b"mpv_free\0")? },
            request_log_messages: unsafe { sym(lib, b"mpv_request_log_messages\0")? },
            render_context_create: unsafe { sym(lib, b"mpv_render_context_create\0")? },
            render_context_free: unsafe { sym(lib, b"mpv_render_context_free\0")? },
            render_context_render: unsafe { sym(lib, b"mpv_render_context_render\0")? },
            render_context_set_update_callback: unsafe {
                sym(lib, b"mpv_render_context_set_update_callback\0")?
            },
            render_context_update: unsafe { sym(lib, b"mpv_render_context_update\0")? },
            render_context_report_swap: unsafe { sym(lib, b"mpv_render_context_report_swap\0")? },
        })
    }
}

/// 动态库候选路径（按优先级）。
pub fn candidate_paths() -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    let mut push = |p: PathBuf| {
        if !out.contains(&p) {
            out.push(p);
        }
    };

    if let Some(p) = std::env::var_os("EM_MPV") {
        push(PathBuf::from(p));
    }

    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            push(dir.join(DLL_NAME));
            // 从 exe 位置向上最多 5 层找 vendor/mpv：覆盖 target/debug 与 target/release
            let mut cur = Some(dir);
            for _ in 0..5 {
                let Some(d) = cur else { break };
                push(d.join("vendor").join("mpv").join(DLL_NAME));
                cur = d.parent();
            }
        }
    }

    if let Ok(cwd) = std::env::current_dir() {
        push(cwd.join("vendor").join("mpv").join(DLL_NAME));
    }

    #[cfg(target_os = "macos")]
    {
        push(PathBuf::from("/opt/homebrew/lib/libmpv.2.dylib"));
        push(PathBuf::from("/usr/local/lib/libmpv.2.dylib"));
    }

    out
}

/// mpv 事件（已从 C 结构体里的裸指针转成拥有所有权的 Rust 值）。
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// 超时 / 无事件
    None,
    Shutdown,
    StartFile,
    /// 文件已加载（能拿到宽高、编码等元数据）
    FileLoaded,
    /// 播放结束。`reason` 见 [`EndReason`]
    EndFile { reason: i32, error: i32 },
    AudioReconfig,
    VideoReconfig,
    PlaybackRestart,
    Log {
        level: String,
        prefix: String,
        text: String,
    },
    Other(i32),
}

/// `EndFile` 的原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndReason {
    Eof,
    Stop,
    Quit,
    Error,
    Redirect,
    Unknown(i32),
}

impl EndReason {
    pub fn from_code(code: i32) -> Self {
        match code {
            ffi::MPV_END_FILE_REASON_EOF => Self::Eof,
            ffi::MPV_END_FILE_REASON_STOP => Self::Stop,
            ffi::MPV_END_FILE_REASON_QUIT => Self::Quit,
            ffi::MPV_END_FILE_REASON_ERROR => Self::Error,
            ffi::MPV_END_FILE_REASON_REDIRECT => Self::Redirect,
            other => Self::Unknown(other),
        }
    }

    /// 断流重连的判据：EOF 与 Error 都需要重新解析地址后重载。
    pub fn needs_reconnect(&self) -> bool {
        matches!(self, Self::Eof | Self::Error)
    }
}

impl std::fmt::Display for EndReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Eof => f.write_str("EOF（流被关闭）"),
            Self::Stop => f.write_str("主动停止"),
            Self::Quit => f.write_str("退出"),
            Self::Error => f.write_str("播放错误"),
            Self::Redirect => f.write_str("重定向"),
            Self::Unknown(c) => write!(f, "未知原因（{c}）"),
        }
    }
}

/// 构造播放器时的选项。
#[derive(Debug, Clone, Default)]
pub struct PlayerOptions {
    /// 指定 `vo`。M0 的 em-smoke 用 `Some("null")` 只出声不出画面
    pub vo: Option<String>,
    /// 指定 `ao`（一般留空走系统默认）
    pub ao: Option<String>,
    /// 额外 mpv 选项，最后应用（可覆盖上面的默认值）
    pub extra: Vec<(String, String)>,
}

impl PlayerOptions {
    /// 只播放音频，不创建视频输出窗口（em-smoke 用）。
    pub fn audio_only() -> Self {
        Self {
            vo: Some("null".to_string()),
            ..Default::default()
        }
    }
}

/// 一路播放实例。
pub struct Player {
    handle: *mut ffi::mpv_handle,
    lib: Arc<Lib>,
    warnings: Vec<String>,
}

impl Player {
    /// 创建并初始化一个 mpv 实例。
    pub fn new(lib: Arc<Lib>, opts: &PlayerOptions) -> Result<Self, PlayerError> {
        let handle = unsafe { (lib.api.create)() };
        if handle.is_null() {
            return Err(PlayerError::Mpv {
                call: "mpv_create",
                code: -1,
                message: "返回空句柄".into(),
            });
        }

        let mut player = Self {
            handle,
            lib,
            warnings: Vec::new(),
        };

        // ---- initialize 之前必须设好的选项 ----
        // config=no：不读用户的 mpv.conf，行为可预期（否则各处机器表现不一致）
        player.set_option("config", "no");
        player.set_option("terminal", "no");
        player.set_option("input-default-bindings", "no");
        player.set_option("input-vo-keyboard", "no");
        player.set_option("osc", "no");
        player.set_option("ytdl", "no"); // 不调用外部 yt-dlp
        player.set_option("hwdec", "auto-safe");

        // ---- 直播低延迟调优（尽力而为，失败只记录不阻断） ----
        player.set_option("cache", "yes");
        player.set_option("demuxer-readahead-secs", "1");
        player.set_option("demuxer-max-bytes", "32MiB");
        player.set_option("cache-secs", "5");
        player.set_option("network-timeout", "10");

        if let Some(vo) = &opts.vo {
            player.set_option("vo", vo);
        }
        if let Some(ao) = &opts.ao {
            player.set_option("ao", ao);
        }
        for (k, v) in &opts.extra {
            player.set_option(k, v);
        }

        let rc = unsafe { (player.lib.api.initialize)(player.handle) };
        if rc < 0 {
            let message = player.error_string(rc);
            unsafe { (player.lib.api.terminate_destroy)(player.handle) };
            return Err(PlayerError::Mpv {
                call: "mpv_initialize",
                code: rc,
                message,
            });
        }

        // 打开 warn 级日志：解析失败、403 之类的线索都在这里
        let level = c"warn";
        unsafe { (player.lib.api.request_log_messages)(player.handle, level.as_ptr()) };

        Ok(player)
    }

    /// 创建过程中被容忍的选项错误（mpv 版本差异时会出现）。
    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    /// 内部句柄，仅给本 crate 的 [`RenderContext`] 用。
    ///
    /// 生命周期约定：`RenderContext::create` 拿到的指针只在 `Player` 存活期间有效，
    /// 且 `RenderContext` 必须先于 `Player` Drop（由使用方保证，见 render.rs 的文档）。
    pub(crate) fn raw_handle(&self) -> *mut ffi::mpv_handle {
        self.handle
    }

    /// 播放器持有的 libmpv 函数表（RenderContext 创建需要）。
    pub fn lib(&self) -> &Arc<Lib> {
        &self.lib
    }

    /// 加载一条流。会先设置 `referrer` / `user-agent`，再 `loadfile`。
    pub fn load(&self, stream: &StreamUrl) -> Result<(), PlayerError> {
        if let Some(referer) = stream.header("Referer") {
            self.set_property_string("referrer", referer)?;
        }
        if let Some(ua) = stream.header("User-Agent") {
            self.set_property_string("user-agent", ua)?;
        }
        self.command(&["loadfile", &stream.url, "replace"])
    }

    /// 停止当前播放并释放网络连接（v1 的「挂断」语义）。
    pub fn stop(&self) -> Result<(), PlayerError> {
        self.command(&["stop"])
    }

    /// 音量 0..=100。
    pub fn set_volume(&self, volume: u8) -> Result<(), PlayerError> {
        self.set_property_f64("volume", f64::from(volume.min(100)))
    }

    pub fn set_muted(&self, muted: bool) -> Result<(), PlayerError> {
        let flag: c_int = i32::from(muted);
        let name = CString::new("mute").map_err(|_| PlayerError::BadString("mute".into()))?;
        let rc = unsafe {
            (self.lib.api.set_property)(
                self.handle,
                name.as_ptr(),
                ffi::MPV_FORMAT_FLAG,
                &flag as *const c_int as *mut c_void,
            )
        };
        self.check("mpv_set_property(mute)", rc)
    }

    pub fn set_option(&mut self, name: &str, value: &str) {
        let (Ok(cname), Ok(cvalue)) = (CString::new(name), CString::new(value)) else {
            self.warnings
                .push(format!("选项 {name} 含 NUL，已跳过"));
            return;
        };
        let rc = unsafe { (self.lib.api.set_option_string)(self.handle, cname.as_ptr(), cvalue.as_ptr()) };
        if rc < 0 {
            self.warnings.push(format!(
                "选项 {name}={value} 被拒绝：{}",
                self.error_string(rc)
            ));
        }
    }

    pub fn set_property_string(&self, name: &str, value: &str) -> Result<(), PlayerError> {
        let cname = CString::new(name).map_err(|_| PlayerError::BadString(name.into()))?;
        let cvalue = CString::new(value).map_err(|_| PlayerError::BadString(value.into()))?;
        let rc =
            unsafe { (self.lib.api.set_property_string)(self.handle, cname.as_ptr(), cvalue.as_ptr()) };
        self.check("mpv_set_property_string", rc)
    }

    pub fn set_property_f64(&self, name: &str, value: f64) -> Result<(), PlayerError> {
        let cname = CString::new(name).map_err(|_| PlayerError::BadString(name.into()))?;
        let v = value;
        let rc = unsafe {
            (self.lib.api.set_property)(
                self.handle,
                cname.as_ptr(),
                ffi::MPV_FORMAT_DOUBLE,
                &v as *const f64 as *mut c_void,
            )
        };
        self.check("mpv_set_property", rc)
    }

    pub fn get_f64(&self, name: &str) -> Option<f64> {
        let cname = CString::new(name).ok()?;
        let mut out: f64 = 0.0;
        let rc = unsafe {
            (self.lib.api.get_property)(
                self.handle,
                cname.as_ptr(),
                ffi::MPV_FORMAT_DOUBLE,
                &mut out as *mut f64 as *mut c_void,
            )
        };
        (rc >= 0).then_some(out)
    }

    pub fn get_i64(&self, name: &str) -> Option<i64> {
        let cname = CString::new(name).ok()?;
        let mut out: i64 = 0;
        let rc = unsafe {
            (self.lib.api.get_property)(
                self.handle,
                cname.as_ptr(),
                ffi::MPV_FORMAT_INT64,
                &mut out as *mut i64 as *mut c_void,
            )
        };
        (rc >= 0).then_some(out)
    }

    /// 读字符串属性。属性不存在返回 `None`（不视为错误）。
    pub fn get_string(&self, name: &str) -> Option<String> {
        let cname = CString::new(name).ok()?;
        let p = unsafe { (self.lib.api.get_property_string)(self.handle, cname.as_ptr()) };
        if p.is_null() {
            return None;
        }
        let s = unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned();
        unsafe { (self.lib.api.free)(p as *mut c_void) };
        Some(s)
    }

    /// 执行 mpv 命令（`args` 不需要带结尾的 NULL，这里补）。
    pub fn command(&self, args: &[&str]) -> Result<(), PlayerError> {
        let owned: Vec<CString> = args
            .iter()
            .map(|a| CString::new(*a))
            .collect::<Result<_, _>>()
            .map_err(|_| PlayerError::BadString(args.join(" ")))?;
        let mut ptrs: Vec<*const c_char> = owned.iter().map(|c| c.as_ptr()).collect();
        ptrs.push(std::ptr::null());
        let rc = unsafe { (self.lib.api.command)(self.handle, ptrs.as_ptr()) };
        self.check("mpv_command", rc)
    }

    /// 等待一个事件。`timeout` 秒内无事件返回 [`Event::None`]。
    pub fn wait_event(&self, timeout: f64) -> Event {
        let ev = unsafe { (self.lib.api.wait_event)(self.handle, timeout) };
        if ev.is_null() {
            return Event::None;
        }
        let ev = unsafe { &*ev };
        match ev.event_id {
            ffi::MPV_EVENT_NONE => Event::None,
            ffi::MPV_EVENT_SHUTDOWN => Event::Shutdown,
            ffi::MPV_EVENT_START_FILE => Event::StartFile,
            ffi::MPV_EVENT_FILE_LOADED => Event::FileLoaded,
            ffi::MPV_EVENT_AUDIO_RECONFIG => Event::AudioReconfig,
            ffi::MPV_EVENT_VIDEO_RECONFIG => Event::VideoReconfig,
            ffi::MPV_EVENT_PLAYBACK_RESTART => Event::PlaybackRestart,
            ffi::MPV_EVENT_END_FILE => {
                if ev.data.is_null() {
                    Event::EndFile {
                        reason: ffi::MPV_END_FILE_REASON_EOF,
                        error: ev.error,
                    }
                } else {
                    let d = unsafe { &*(ev.data as *const ffi::mpv_event_end_file) };
                    Event::EndFile {
                        reason: d.reason,
                        error: d.error,
                    }
                }
            }
            ffi::MPV_EVENT_LOG_MESSAGE => {
                if ev.data.is_null() {
                    return Event::Other(ffi::MPV_EVENT_LOG_MESSAGE);
                }
                let m = unsafe { &*(ev.data as *const ffi::mpv_event_log_message) };
                Event::Log {
                    level: cstr_or_empty(m.level),
                    prefix: cstr_or_empty(m.prefix),
                    text: cstr_or_empty(m.text).trim_end().to_string(),
                }
            }
            other => Event::Other(other),
        }
    }

    fn error_string(&self, code: i32) -> String {
        let p = unsafe { (self.lib.api.error_string)(code) };
        if p.is_null() {
            return format!("错误码 {code}");
        }
        unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
    }

    fn check(&self, call: &'static str, rc: i32) -> Result<(), PlayerError> {
        if rc >= 0 {
            Ok(())
        } else {
            Err(PlayerError::Mpv {
                call,
                code: rc,
                message: self.error_string(rc),
            })
        }
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        // terminate_destroy 会等播放线程退出；在窗口关闭/切换直播时必须走这里，
        // 否则网络连接与解码线程会残留。
        if !self.handle.is_null() {
            unsafe { (self.lib.api.terminate_destroy)(self.handle) };
            self.handle = std::ptr::null_mut();
        }
    }
}

fn cstr_or_empty(p: *const c_char) -> String {
    if p.is_null() {
        String::new()
    } else {
        unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn end_reason_maps_codes() {
        assert_eq!(EndReason::from_code(0), EndReason::Eof);
        assert_eq!(EndReason::from_code(2), EndReason::Stop);
        assert_eq!(EndReason::from_code(4), EndReason::Error);
        assert_eq!(EndReason::from_code(99), EndReason::Unknown(99));
        assert!(EndReason::Eof.needs_reconnect());
        assert!(EndReason::Error.needs_reconnect());
        assert!(!EndReason::Stop.needs_reconnect());
    }

    #[test]
    fn candidate_paths_contains_vendor_entry() {
        let paths = candidate_paths();
        assert!(!paths.is_empty());
        assert!(
            paths.iter().any(|p| p.ends_with(DLL_NAME)),
            "候选路径里应有以动态库名结尾的项"
        );
    }

    /// 只在 dll 已就位时跑：验证真正能加载 + 查 API 版本。
    /// dll 是 gitignore 的，CI/新机器没跑过 fetch-mpv 时自动跳过。
    #[test]
    fn loads_libmpv_when_available() {
        match Lib::load_default() {
            Ok(lib) => {
                let (major, minor) = lib.client_api_version();
                assert!(major >= 2, "客户端 API 主版本应 >= 2，实际 {major}.{minor}");
            }
            Err(e) => {
                eprintln!("跳过：{e}");
            }
        }
    }
}
