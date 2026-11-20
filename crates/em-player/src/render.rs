//! mpv 渲染 API 封装（render.h）——方案 A（§4.1）的基础。
//!
//! 线程约定（照 render.h 原文）：
//! - `create` / `render` 调用时，GL context 必须 current，且创建与渲染必须用
//!   **同一个 context**。本项目约定：窗口线程（winit 主线程）创建并持有。
//! - update callback 由 libmpv 内部线程调用，里面**不能**碰 GL，
//!   只允许唤醒渲染线程（本项目用 `Box<dyn Fn() + Send + Sync>` 包装）。
//!
//! Drop 顺序：必须先 Drop `RenderContext` 再 Drop `Player`
//! （render context 引用 mpv 句柄）。`smoke_single` 与后续 UI 层把两者
//! 放在同一个结构体里，字段声明顺序天然保证这一点。

use std::ffi::c_void;
use std::os::raw::{c_char, c_int};
use std::sync::Arc;

use crate::ffi;
use crate::{Player, PlayerError};

/// 新帧回调。libmpv 内部线程调用，必须非阻塞。
pub type UpdateCallback = Box<dyn Fn() + Send + Sync>;

/// 一个 mpv 实例的 OpenGL 渲染上下文。
pub struct RenderContext {
    ctx: *mut ffi::mpv_render_context,
    lib: Arc<crate::Lib>,
    /// 双 Box：外层 Box 随 struct move 无所谓，内层 `Box<dyn Fn>` 的头部
    /// 稳定在堆上——mpv 存的指针指向它，struct 怎么搬家都不受影响。
    callback: Option<Box<UpdateCallback>>,
    /// GL 函数解析器，同样必须活得和 ctx 一样长（mpv 存的是裸指针）。
    /// 唯一用途是持有——下划线前缀防「never read」告警。
    _proc_resolver: Option<Box<ProcResolver>>,
}

// render context 的创建与使用都在同一线程（GL context current 的那个）。
// `callback` 是 Send+Sync 的，指针本身的共享不跨线程——Send 对回调闭包成立即可。
unsafe impl Send for RenderContext {}

extern "C" fn update_trampoline(user: *mut c_void) {
    // SAFETY：user 指向 UpdateCallback Box 的堆上内容，生命周期由
    // RenderContext 持有并在 Drop 中先注销回调再回收（见 Drop 实现）。
    let f = unsafe { &*(user as *const UpdateCallback) };
    f();
}

extern "C" fn get_proc_trampoline(ctx: *mut c_void, name: *const c_char) -> *mut c_void {
    // SAFETY：ctx 指向 ProcResolver 的堆上 Box，生命周期同上。
    let resolver = unsafe { &*(ctx as *const ProcResolver) };
    let cstr = if name.is_null() {
        return std::ptr::null_mut();
    } else {
        unsafe { std::ffi::CStr::from_ptr(name) }
    };
    let name = cstr.to_string_lossy().into_owned();
    (resolver.0)(&name) as *mut c_void
}

/// GL 函数解析器（由窗口层提供，包一层以便传过 FFI）。
struct ProcResolver(Box<dyn Fn(&str) -> *const std::ffi::c_void + Send + Sync>);

impl RenderContext {
    /// 创建渲染上下文。
    ///
    /// `get_proc` 是 GL 函数解析器（glutin：`display.get_proc_address`）。
    /// 调用线程的 GL context 必须 current，且之后所有 `render` 都在同一线程。
    ///
    /// ⚠️ `get_proc` 会被移到堆上并在 RenderContext 存活期间持有——mpv 把
    /// `get_proc_address_ctx` 裸指针存进内部状态，回调发生在 create 之后
    /// 的任意渲染时刻，所以解析器**绝不能是栈上临时值**（踩过一次：段错误）。
    pub fn create(
        lib: Arc<crate::Lib>,
        player: &Player,
        get_proc: Box<dyn Fn(&str) -> *const std::ffi::c_void + Send + Sync>,
    ) -> Result<Self, PlayerError> {
        // Box 两层：外层 Box 给 mpv 一个稳定的堆地址，struct 字段持有外层
        let resolver: Box<ProcResolver> = Box::new(ProcResolver(get_proc));
        let resolver_ptr: *const ProcResolver = &*resolver;
        let init_params = ffi::mpv_opengl_init_params {
            get_proc_address: Some(get_proc_trampoline),
            get_proc_address_ctx: resolver_ptr as *mut c_void,
        };

        let api_type = ffi::MPV_RENDER_API_TYPE_OPENGL.as_ptr() as *const c_char;
        let mut params = [
            ffi::mpv_render_param {
                param_type: ffi::MPV_RENDER_PARAM_API_TYPE,
                data: api_type as *mut c_void,
            },
            ffi::mpv_render_param {
                param_type: ffi::MPV_RENDER_PARAM_OPENGL_INIT_PARAMS,
                data: &init_params as *const ffi::mpv_opengl_init_params as *mut c_void,
            },
            ffi::mpv_render_param {
                param_type: ffi::MPV_RENDER_PARAM_INVALID,
                data: std::ptr::null_mut(),
            },
        ];

        let mut ctx: *mut ffi::mpv_render_context = std::ptr::null_mut();
        let rc = unsafe {
            (lib.api.render_context_create)(
                &mut ctx,
                player.raw_handle(),
                params.as_mut_ptr(),
            )
        };
        if rc < 0 {
            return Err(PlayerError::Mpv {
                call: "mpv_render_context_create",
                code: rc,
                message: format!("错误码 {rc}（常见原因：vo 未设为 libmpv，或 GL context 不在当前线程）"),
            });
        }

        Ok(Self {
            ctx,
            lib,
            callback: None,
            _proc_resolver: Some(resolver),
        })
    }

    /// 注册新帧回调（先注销旧的）。回调在 libmpv 内部线程运行，务必非阻塞。
    ///
    /// ⚠️ 与 `create` 的 resolver 同理：mpv 存的是裸指针。指针必须指向
    /// **稳定的堆/字段地址**——指向栈上局部变量会在返回后变成悬空指针
    /// （第一次回调碰巧还能跑，之后必段错误，踩过一次）。
    /// 这里把 Box 指针本身存进 `self.callback` 字段（Option<Box<T>> 无判别
    /// 额外字节），再把指向该字段的指针交给 mpv。
    pub fn set_update_callback(&mut self, callback: UpdateCallback) {
        self.callback = Some(Box::new(callback));
        let f: *const UpdateCallback = &**self.callback.as_ref().unwrap();
        unsafe {
            (self.lib.api.render_context_set_update_callback)(
                self.ctx,
                Some(update_trampoline),
                f as *mut c_void,
            );
        }
    }

    /// 读更新标志（位集见 `ffi::MPV_RENDER_UPDATE_FRAME`）。
    pub fn update(&self) -> u64 {
        unsafe { (self.lib.api.render_context_update)(self.ctx) }
    }

    /// 渲染到默认 framebuffer（fbo=0）。调用线程 GL context 必须 current。
    /// 默认 framebuffer 是上下颠倒的，`flip_y` 传 `true`。
    pub fn render(&self, w: i32, h: i32, flip_y: bool) -> Result<(), PlayerError> {
        let fbo = ffi::mpv_opengl_fbo {
            fbo: 0,
            w,
            h,
            internal_format: 0,
        };
        let flip: c_int = i32::from(flip_y);
        let mut params = [
            ffi::mpv_render_param {
                param_type: ffi::MPV_RENDER_PARAM_OPENGL_FBO,
                data: &fbo as *const ffi::mpv_opengl_fbo as *mut c_void,
            },
            ffi::mpv_render_param {
                param_type: ffi::MPV_RENDER_PARAM_FLIP_Y,
                data: &flip as *const c_int as *mut c_void,
            },
            ffi::mpv_render_param {
                param_type: ffi::MPV_RENDER_PARAM_INVALID,
                data: std::ptr::null_mut(),
            },
        ];
        let rc = unsafe { (self.lib.api.render_context_render)(self.ctx, params.as_mut_ptr()) };
        if rc < 0 {
            return Err(PlayerError::Mpv {
                call: "mpv_render_context_render",
                code: rc,
                message: format!("错误码 {rc}"),
            });
        }
        Ok(())
    }

    /// 交换后报告（帮助 mpv 做帧计时；调了就一直调，不能时调时不调）。
    pub fn report_swap(&self) {
        unsafe { (self.lib.api.render_context_report_swap)(self.ctx) }
    }

    /// 注销 update 回调。Drop 自动调用，早于 ctx 释放。
    fn clear_callback(&mut self) {
        unsafe {
            (self.lib.api.render_context_set_update_callback)(
                self.ctx,
                None,
                std::ptr::null_mut(),
            );
        }
        self.callback = None;
    }
}

impl Drop for RenderContext {
    fn drop(&mut self) {
        if self.ctx.is_null() {
            return;
        }
        // 顺序必须是：先注销回调（否则内部线程还会摸已释放的回调），
        // 再 free context，最后回收回调 Box。
        self.clear_callback();
        unsafe { (self.lib.api.render_context_free)(self.ctx) };
        self.ctx = std::ptr::null_mut();
    }
}