//! `em-core` — 纯逻辑层：配置模型、会话状态、布局几何。
//!
//! 这一层没有任何网络、没有窗口、没有播放器调用，因此可以完整单测，
//! 也是唯一一个在 Windows 与 macOS 上行为必须逐字节一致的 crate。
//!
//! 模块职责：
//! - [`config`]：`config/em.json` 的模型、校验与原子落盘
//! - [`session`]：记录库 / 显示列 / 主格的状态操作与不变量
//! - [`geom`]：矩形运算与「按目标宽高比取内接矩形」，布局引擎（M2）的底座
//!
//! 设计约束见 `PROJECT.md` §4.5 / §4.6：布局是「显示列长度 + 每路宽高比 + 主格」的函数，
//! 没有固定网格，也不存在「第几号格子」这种持久化概念。

pub mod config;
pub mod geom;
pub mod session;

pub use config::{Config, Library, MAX_VISIBLE, Record, WindowState};
pub use session::{Session, ShowError};
