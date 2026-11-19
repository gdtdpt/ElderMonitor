//! `em-ui` — 窗口、渲染与交互。
//!
//! **M0 阶段这是占位 crate**，不包含代码。它存在的原因有两个：
//! 1. 固定 workspace 结构与依赖方向（`em-ui → em-player → em-resolve → em-core`）
//! 2. 让构建脚本从第一天起就能按「workspace 全量构建」的方式工作
//!
//! M1 起的实现范围（见 `PROJECT.md` §4.1 / §5 / §9）：
//! - winit 窗口 + OpenGL 上下文（Windows 走 WGL、macOS 走 CGL）
//! - mpv 渲染 API：每个 `em_player::Player` 建一个 `mpv_render_context`，
//!   先做「单路直渲」，M2 再做「多路 FBO 纹理 + 布局合成」
//! - 控件：顶栏、每格控制条（音量/静音/状态）、记录库两态面板
//!
//! 渲染层与播放控制层是分离的：切到方案 B（子窗口）只改本 crate，
//! `em-player` 一行不动。
