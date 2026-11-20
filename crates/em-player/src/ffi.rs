//! libmpv 的 C ABI 声明（只声明本项目用得到的部分）。
//!
//! 与 `vendor/mpv/include/mpv/client.h` 一一对应；数值常量取自该头文件，
//! 不是凭记忆写的。改这里之前先核对头文件。

#![allow(non_camel_case_types)]
// 这里刻意镜像头文件的完整枚举：M1 起会用到 PROPERTY_CHANGE / CLIENT_MESSAGE 等，
// 少写一个反而容易在后续补写时写错数值。
#![allow(dead_code)]

use std::os::raw::{c_char, c_int, c_void};

/// 不透明句柄。Rust 侧只持有指针，不构造它的值。
pub type mpv_handle = c_void;

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct mpv_event {
    pub event_id: c_int,
    pub error: c_int,
    pub reply_userdata: u64,
    pub data: *mut c_void,
}

#[repr(C)]
pub struct mpv_event_log_message {
    pub prefix: *const c_char,
    pub level: *const c_char,
    pub text: *const c_char,
    pub log_level: c_int,
}

#[repr(C)]
pub struct mpv_event_end_file {
    pub reason: c_int,
    pub error: c_int,
    pub playlist_entry_id: i64,
    pub playlist_insert_id: i64,
    pub playlist_insert_num_entries: c_int,
}

// ---- mpv_event_id ----
pub const MPV_EVENT_NONE: c_int = 0;
pub const MPV_EVENT_SHUTDOWN: c_int = 1;
pub const MPV_EVENT_LOG_MESSAGE: c_int = 2;
pub const MPV_EVENT_START_FILE: c_int = 6;
pub const MPV_EVENT_END_FILE: c_int = 7;
pub const MPV_EVENT_FILE_LOADED: c_int = 8;
pub const MPV_EVENT_CLIENT_MESSAGE: c_int = 16;
pub const MPV_EVENT_VIDEO_RECONFIG: c_int = 17;
pub const MPV_EVENT_AUDIO_RECONFIG: c_int = 18;
pub const MPV_EVENT_PLAYBACK_RESTART: c_int = 21;
pub const MPV_EVENT_PROPERTY_CHANGE: c_int = 22;
pub const MPV_EVENT_QUEUE_OVERFLOW: c_int = 24;

// ---- mpv_format ----
pub const MPV_FORMAT_FLAG: c_int = 3;
pub const MPV_FORMAT_INT64: c_int = 4;
pub const MPV_FORMAT_DOUBLE: c_int = 5;

// ---- mpv_end_file_reason ----
pub const MPV_END_FILE_REASON_EOF: c_int = 0;
pub const MPV_END_FILE_REASON_STOP: c_int = 2;
pub const MPV_END_FILE_REASON_QUIT: c_int = 3;
pub const MPV_END_FILE_REASON_ERROR: c_int = 4;
pub const MPV_END_FILE_REASON_REDIRECT: c_int = 5;

// ================= render.h / render_gl.h =================
// 与 `vendor/mpv/include/mpv/render.h`、`render_gl.h` 一一对应。

/// 不透明渲染上下文（render.h）。
pub type mpv_render_context = c_void;

/// mpv_render_param（render.h）：`{ type; void *data; }`。
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct mpv_render_param {
    pub param_type: c_int,
    pub data: *mut c_void,
}

// ---- mpv_render_param_type（render.h，截取本项目用到的项）----
pub const MPV_RENDER_PARAM_INVALID: c_int = 0;
pub const MPV_RENDER_PARAM_API_TYPE: c_int = 1;
pub const MPV_RENDER_PARAM_OPENGL_INIT_PARAMS: c_int = 2;
pub const MPV_RENDER_PARAM_OPENGL_FBO: c_int = 3;
pub const MPV_RENDER_PARAM_FLIP_Y: c_int = 4;
pub const MPV_RENDER_PARAM_DEPTH: c_int = 5;

/// render.h：`MPV_RENDER_API_TYPE_OPENGL`。
pub const MPV_RENDER_API_TYPE_OPENGL: &[u8] = b"opengl\0";

// ---- mpv_render_update_flag（render.h）----
pub const MPV_RENDER_UPDATE_FRAME: u64 = 1 << 0;

/// mpv_opengl_fbo（render_gl.h）：描述一个 GL 渲染目标。
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct mpv_opengl_fbo {
    pub fbo: c_int,
    pub w: c_int,
    pub h: c_int,
    pub internal_format: c_int,
}

/// mpv_opengl_init_params（render_gl.h）。
///
/// `get_proc_address` 由调用方实现（本项目用 glutin 的
/// `GlDisplay::get_proc_address`），libmpv 只通过它解析 GL 函数，
/// 不自己链接 GL。
#[repr(C)]
pub struct mpv_opengl_init_params {
    pub get_proc_address:
        Option<unsafe extern "C" fn(ctx: *mut c_void, name: *const c_char) -> *mut c_void>,
    pub get_proc_address_ctx: *mut c_void,
}
