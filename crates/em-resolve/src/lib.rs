//! `em-resolve` — 直播间地址解析。
//!
//! v1 只做 B 站。对外接口是 [`resolve`]：给一个房间号或直播间链接，
//! 拿回房间状态、标题与可直接喂给 mpv 的 [`StreamUrl`]。
//!
//! 后续接其他平台时的扩展点：新增一个 `host` 模块（如 `douyin.rs`），
//! 在 [`resolve`] 里按输入特征分派；[`StreamUrl`] 与 [`Resolved`] 的结构不需要改。

mod bilibili;

pub use bilibili::{
    Codec, Protocol, REFERER, ResolveError, Resolved, StreamUrl, USER_AGENT, normalize_room,
    resolve,
};
