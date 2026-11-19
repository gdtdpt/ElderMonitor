//! 配置模型与持久化（`config/em.json`）。
//!
//! 两层结构直接对应界面模型：
//! - `records`：**记录库**，添加过的全部直播间，条数不限
//! - `visible`：**显示列**，数组顺序即格子顺序，长度 = 当前显示的格子数
//!
//! 音量 / 静音 / 名字 / 宽高比挂在**记录**上，不挂格子——换格子、临时隐藏再显示，
//! 这些设置都不丢。`primary` 是主格记录的 id，`null` 表示均匀铺满。

use std::collections::HashSet;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// 同时显示上限。超出只能停在记录库里，不进显示列。
pub const MAX_VISIBLE: usize = 12;
/// 新记录的默认音量。多路同时出声时音量是叠加的，默认给低一点更实用。
pub const DEFAULT_VOLUME: u8 = 40;
/// 音量上限。
pub const MAX_VOLUME: u8 = 100;
/// 缺省宽高比：16:9。
pub const DEFAULT_ASPECT: f32 = 16.0 / 9.0;
/// 记录库抽屉默认宽度（钉住时占用布局宽度）。
pub const DEFAULT_LIBRARY_WIDTH: f32 = 260.0;

const fn default_aspect() -> f32 {
    DEFAULT_ASPECT
}

const fn default_volume() -> u8 {
    DEFAULT_VOLUME
}

const fn default_library_width() -> f32 {
    DEFAULT_LIBRARY_WIDTH
}

/// 一条直播间记录（属于记录库，与界面格子解耦）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Record {
    /// 记录 id，形如 `r1` / `r2`；只在配置内部使用，不对用户展示
    pub id: String,
    /// B 站房间号（长号或短号，统一以解析结果为准）
    pub room: String,
    /// 显示名。空串表示尚未命名，UI 展示时回退到 `房间 <room>`
    #[serde(default)]
    pub name: String,
    /// 目标宽高比（宽/高）。来自 mpv 的 `width`/`height`，未知时按 16:9 占位
    #[serde(default = "default_aspect")]
    pub aspect: f32,
    /// 音量 0..=100
    #[serde(default = "default_volume")]
    pub volume: u8,
    #[serde(default)]
    pub muted: bool,
    /// 最近观看时间（Unix 秒），记录库排序用
    #[serde(default)]
    pub last_watch: i64,
}

impl Record {
    /// 用于界面的展示名：未命名时回退到房间号。
    pub fn display_name(&self) -> String {
        if self.name.trim().is_empty() {
            format!("房间 {}", self.room)
        } else {
            self.name.clone()
        }
    }
}

/// 记录库面板的两态设置（悬浮不占布局，钉住才占宽）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Library {
    #[serde(default)]
    pub pinned: bool,
    #[serde(default = "default_library_width")]
    pub width: f32,
}

impl Default for Library {
    fn default() -> Self {
        Self {
            pinned: false,
            width: DEFAULT_LIBRARY_WIDTH,
        }
    }
}

impl Library {
    /// 钉住时实际占用的宽度；未钉住对布局不可见。
    pub fn reserved_width(&self) -> f32 {
        if self.pinned {
            self.width.clamp(200.0, 380.0)
        } else {
            0.0
        }
    }
}

/// 窗口位置与全屏状态。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WindowState {
    #[serde(default = "default_window_w")]
    pub w: f32,
    #[serde(default = "default_window_h")]
    pub h: f32,
    #[serde(default)]
    pub x: f32,
    #[serde(default)]
    pub y: f32,
    #[serde(default)]
    pub fullscreen: bool,
    /// 全屏时使用的显示器编号（0 为主显示器）
    #[serde(default)]
    pub monitor: usize,
}

const fn default_window_w() -> f32 {
    1280.0
}

const fn default_window_h() -> f32 {
    720.0
}

impl Default for WindowState {
    fn default() -> Self {
        Self {
            w: default_window_w(),
            h: default_window_h(),
            x: 0.0,
            y: 0.0,
            fullscreen: false,
            monitor: 0,
        }
    }
}

/// 全部持久化状态。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Config {
    #[serde(default)]
    pub records: Vec<Record>,
    #[serde(default)]
    pub visible: Vec<String>,
    #[serde(default)]
    pub primary: Option<String>,
    #[serde(default)]
    pub library: Library,
    #[serde(default)]
    pub window: WindowState,
}

/// 读写配置时的错误。
#[derive(Debug)]
pub enum ConfigError {
    Io(io::Error),
    Parse(serde_json::Error),
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "读写配置失败：{e}"),
            Self::Parse(e) => write!(f, "配置格式错误：{e}"),
        }
    }
}

impl std::error::Error for ConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            Self::Parse(e) => Some(e),
        }
    }
}

impl From<io::Error> for ConfigError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<serde_json::Error> for ConfigError {
    fn from(e: serde_json::Error) -> Self {
        Self::Parse(e)
    }
}

impl Config {
    /// 便携布局下的配置路径：`<exe 所在目录>/config/em.json`。
    pub fn path_near(exe_dir: &Path) -> PathBuf {
        exe_dir.join("config").join("em.json")
    }

    /// 读取配置。文件不存在**不是错误**，返回默认配置（首次启动）。
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        match fs::read_to_string(path) {
            Ok(text) => {
                let mut cfg: Self = serde_json::from_str(&text)?;
                cfg.sanitize();
                Ok(cfg)
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(ConfigError::Io(e)),
        }
    }

    /// 原子落盘：先写 `em.json.tmp` 再 rename，避免掉电/异常退出留下半截 JSON。
    pub fn save(&self, path: &Path) -> Result<(), ConfigError> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let text = serde_json::to_string_pretty(self)?;
        let tmp = path.with_extension("json.tmp");
        {
            let mut f = fs::File::create(&tmp)?;
            f.write_all(text.as_bytes())?;
            f.write_all(b"\n")?;
            f.sync_all()?;
        }
        fs::rename(&tmp, path)?;
        Ok(())
    }

    /// 修掉跨字段不一致。手改过配置文件、或旧版本遗留的数据都靠这里兜住。
    ///
    /// 规则：记录 id 去重 → 修非法宽高比 → 夹音量 → 显示列只保留存在的记录、
    /// 去重、截断到 [`MAX_VISIBLE`] → 主格必须仍在显示列里。
    pub fn sanitize(&mut self) {
        let mut seen_ids = HashSet::new();
        self.records
            .retain(|r| !r.id.trim().is_empty() && seen_ids.insert(r.id.clone()));

        for r in &mut self.records {
            if !(r.aspect.is_finite() && r.aspect > 0.05 && r.aspect < 20.0) {
                r.aspect = DEFAULT_ASPECT;
            }
            r.volume = r.volume.min(MAX_VOLUME);
            r.room = r.room.trim().to_string();
        }

        let known: HashSet<&str> = self.records.iter().map(|r| r.id.as_str()).collect();
        let mut seen_visible = HashSet::new();
        self.visible
            .retain(|id| known.contains(id.as_str()) && seen_visible.insert(id.clone()));
        self.visible.truncate(MAX_VISIBLE);

        let primary_still_visible = match &self.primary {
            Some(primary) => self.visible.iter().any(|v| v == primary),
            None => true,
        };
        if !primary_still_visible {
            self.primary = None;
        }

        if !(self.library.width.is_finite() && self.library.width > 0.0) {
            self.library.width = DEFAULT_LIBRARY_WIDTH;
        }
    }

    pub fn record(&self, id: &str) -> Option<&Record> {
        self.records.iter().find(|r| r.id == id)
    }

    pub fn record_mut(&mut self, id: &str) -> Option<&mut Record> {
        self.records.iter_mut().find(|r| r.id == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Config {
        Config {
            records: vec![
                Record {
                    id: "r1".into(),
                    room: "7734200".into(),
                    name: "主听".into(),
                    aspect: 16.0 / 9.0,
                    volume: 85,
                    muted: false,
                    last_watch: 1,
                },
                Record {
                    id: "r2".into(),
                    room: "21452505".into(),
                    name: String::new(),
                    aspect: 9.0 / 16.0,
                    volume: 25,
                    muted: true,
                    last_watch: 2,
                },
            ],
            visible: vec!["r1".into(), "r2".into()],
            primary: Some("r1".into()),
            library: Library {
                pinned: true,
                width: 300.0,
            },
            window: WindowState::default(),
        }
    }

    fn temp_file(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("em-core-test-{}-{tag}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir.join("em.json")
    }

    #[test]
    fn save_then_load_roundtrips() {
        let path = temp_file("roundtrip");
        let cfg = sample();
        cfg.save(&path).unwrap();
        let back = Config::load(&path).unwrap();
        assert_eq!(cfg, back);
        // 落盘是格式化 JSON，人肉可改
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("\"records\""));
        assert!(text.contains("\"visible\""));
        assert!(!path.with_extension("json.tmp").exists(), "临时文件应已 rename");
    }

    #[test]
    fn missing_file_yields_default() {
        let path = temp_file("missing").with_file_name("nope.json");
        let cfg = Config::load(&path).unwrap();
        assert!(cfg.records.is_empty());
        assert_eq!(cfg.library.width, DEFAULT_LIBRARY_WIDTH);
        assert!(!cfg.window.fullscreen);
    }

    #[test]
    fn sanitize_drops_unknown_visible_ids() {
        let mut cfg = sample();
        cfg.visible.push("ghost".into());
        cfg.sanitize();
        assert_eq!(cfg.visible, vec!["r1".to_string(), "r2".to_string()]);
    }

    #[test]
    fn sanitize_truncates_visible_to_cap() {
        let mut cfg = Config::default();
        for i in 0..20 {
            let id = format!("r{i}");
            cfg.records.push(Record {
                id: id.clone(),
                room: format!("{i}"),
                name: String::new(),
                aspect: DEFAULT_ASPECT,
                volume: DEFAULT_VOLUME,
                muted: false,
                last_watch: 0,
            });
            cfg.visible.push(id);
        }
        cfg.sanitize();
        assert_eq!(cfg.visible.len(), MAX_VISIBLE);
    }

    #[test]
    fn sanitize_clears_primary_not_visible() {
        let mut cfg = sample();
        cfg.primary = Some("r2".into());
        cfg.visible = vec!["r1".into()];
        cfg.sanitize();
        assert_eq!(cfg.primary, None);
    }

    #[test]
    fn sanitize_repairs_volume_and_aspect() {
        let mut cfg = sample();
        cfg.records[0].volume = 200;
        cfg.records[0].aspect = f32::NAN;
        cfg.sanitize();
        assert_eq!(cfg.records[0].volume, MAX_VOLUME);
        assert_eq!(cfg.records[0].aspect, DEFAULT_ASPECT);
    }

    #[test]
    fn sanitize_dedupes_visible() {
        let mut cfg = sample();
        cfg.visible = vec!["r1".into(), "r1".into(), "r2".into()];
        cfg.sanitize();
        assert_eq!(cfg.visible, vec!["r1".to_string(), "r2".to_string()]);
    }

    #[test]
    fn library_reserved_width_only_when_pinned() {
        let mut lib = Library::default();
        assert_eq!(lib.reserved_width(), 0.0);
        lib.pinned = true;
        assert_eq!(lib.reserved_width(), DEFAULT_LIBRARY_WIDTH);
        lib.width = 5000.0;
        assert_eq!(lib.reserved_width(), 380.0, "宽度夹在 200..=380");
    }
}
