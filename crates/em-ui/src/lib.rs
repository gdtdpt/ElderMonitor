//! `em-ui` — 窗口、渲染与交互。
//!
//! 渲染层与播放控制层是分离的：切到方案 B（子窗口）只改本 crate，
//! `em-player` 一行不动（`PROJECT.md` §4.1 的设计约束）。
//!
//! 当前实现（M1）：单路直渲。`smoke::smoke_single` 是 M1 的验收入口——
//! 开一个窗口、用 mpv 渲染 API 把真实直播画上屏、读回像素自检。

pub mod png;
pub mod smoke;

pub use smoke::{smoke_single, SmokeReport};
