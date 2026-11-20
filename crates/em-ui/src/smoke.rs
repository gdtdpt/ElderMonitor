//! M1 单路直渲冒烟：开窗口 → mpv render API 上屏 → 读回像素自检。
//!
//! 这是 `PROJECT.md` §5 里 **M1 风险闸门** 的验收代码：GL interop 通不通、
//! libmpv 的 `vo=libmpv` + glutin/wgl 是否兼容，全看这一步能否出非黑画面。
//!
//! 线程模型（照 render.h 约定）：GL context 的创建与 `render` 都在
//! winit 事件线程；mpv 的 update callback 在 mpv 内部线程，只做
//! `proxy.send_event` 唤醒，不碰任何 GL。
//!
//! glutin 0.32 的模型是 **Surface（缓冲）与 Context（状态）分离**：
//! `make_current(&surface)` 把两者绑起来，swap/resize 操作走 Surface。

use std::ffi::CString;
use std::num::NonZeroU32;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use glutin::config::ConfigTemplateBuilder;
use glutin::context::{ContextAttributesBuilder, PossiblyCurrentContext};
use glutin::display::GetGlDisplay;
use glutin::prelude::{GlDisplay, NotCurrentGlContext};
use glutin::surface::{GlSurface, Surface, SurfaceAttributesBuilder, WindowSurface};
use glutin_winit::DisplayBuilder;
use glow::HasContext;
use raw_window_handle::HasWindowHandle;
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::window::{Window, WindowId};

use em_player::{Player, PlayerOptions, RenderContext};
use em_resolve::StreamUrl;

/// 冒烟结果。
#[derive(Debug, Clone)]
pub struct SmokeReport {
    /// 实际渲染的帧数
    pub frames_rendered: u32,
    /// mpv 新帧回调触发次数
    pub update_callbacks: u32,
    /// 结束帧平均亮度（0~255）
    pub mean_luma: f32,
    /// 结束帧尺寸
    pub width: u32,
    pub height: u32,
    /// 截帧 PNG 落盘位置
    pub png_path: PathBuf,
}

impl SmokeReport {
    /// 通过标准：画面非黑（平均亮度 > 5/255）且回调与渲染都在动。
    pub fn is_pass(&self) -> bool {
        self.mean_luma > 5.0 && self.update_callbacks > 0 && self.frames_rendered > 0
    }
}

impl std::fmt::Display for SmokeReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "帧数 {} / 回调 {} / 亮度 {:.1}/255 / {}x{} / 截帧 {}",
            self.frames_rendered,
            self.update_callbacks,
            self.mean_luma,
            self.width,
            self.height,
            self.png_path.display()
        )
    }
}

#[derive(Debug)]
enum UiEvent {
    /// mpv 有新帧，请求一次 redraw
    Wake,
}

struct Running {
    window: Window,
    /// Drop 顺序（声明序）：surface 先于 context 释放；
    /// `render_ctx` 必须先于 `player`，也声明在前。
    surface: Surface<WindowSurface>,
    gl_ctx: PossiblyCurrentContext,
    gl: glow::Context,
    render_ctx: RenderContext,
    /// 只为 Drop 顺序而持有（必须晚于 render_ctx 释放），不直接读取。
    _player: Player,
    _display: Arc<glutin::display::Display>,
    update_counter: Arc<AtomicU32>,
    start: Instant,
    deadline: Duration,
    frames: u32,
    next_report: Instant,
    finished: bool,
}

impl Running {
    fn maybe_report(&mut self) {
        let now = Instant::now();
        if now < self.next_report {
            return;
        }
        let secs = self.start.elapsed().as_secs_f32();
        println!(
            "[gui] {secs:5.1}s  已渲染 {} 帧  mpv 回调 {} 次",
            self.frames,
            self.update_counter.load(Ordering::Relaxed)
        );
        self.next_report = now + Duration::from_secs(1);
    }
}

struct App {
    proxy: EventLoopProxy<UiEvent>,
    stream: StreamUrl,
    seconds: u32,
    volume: u8,
    png_path: PathBuf,
    running: Option<Running>,
    error: Option<String>,
    report: Option<SmokeReport>,
}

impl App {
    fn fail(&mut self, event_loop: &ActiveEventLoop, msg: String) {
        self.error = Some(msg);
        event_loop.exit();
    }

    fn finish(&mut self, event_loop: &ActiveEventLoop) {
        let Some(st) = self.running.as_mut() else { return };
        if st.finished {
            return;
        }
        st.finished = true;

        let size = st.window.inner_size();
        let (w, h) = (size.width, size.height);
        if w == 0 || h == 0 {
            self.error = Some("窗口尺寸为 0，无法截帧".into());
            event_loop.exit();
            return;
        }

        // 读回最后渲染的一帧（GL 的行序是从下到上，写 PNG 前翻转）
        let mut pixels = vec![0u8; (w * h * 3) as usize];
        unsafe {
            st.gl.read_pixels(
                0,
                0,
                w as i32,
                h as i32,
                glow::RGB,
                glow::UNSIGNED_BYTE,
                glow::PixelPackData::Slice(Some(&mut pixels)),
            )
        };
        let stride = (w * 3) as usize;
        let mut flipped = vec![0u8; pixels.len()];
        for y in 0..h as usize {
            let src = (h as usize - 1 - y) * stride;
            flipped[y * stride..(y + 1) * stride]
                .copy_from_slice(&pixels[src..src + stride]);
        }

        let mean: f32 =
            flipped.iter().map(|&b| u32::from(b) as f32).sum::<f32>() / flipped.len() as f32;

        if let Err(e) = crate::png::write_rgb_png(&self.png_path, w, h, &flipped) {
            self.error = Some(format!("写 PNG 失败（{}）：{e}", self.png_path.display()));
            event_loop.exit();
            return;
        }

        self.report = Some(SmokeReport {
            frames_rendered: st.frames,
            update_callbacks: st.update_counter.load(Ordering::Relaxed),
            mean_luma: mean,
            width: w,
            height: h,
            png_path: self.png_path.clone(),
        });
        event_loop.exit();
    }
}

impl ApplicationHandler<UiEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        let attrs = Window::default_attributes()
            .with_title("ElderMonitor M1 smoke")
            .with_inner_size(winit::dpi::LogicalSize::new(960.0, 540.0));

        // WGL 下窗口先建（DisplayBuilder 内部处理），config 由 picker 挑第一个
        let (window, config) = DisplayBuilder::new()
            .with_window_attributes(Some(attrs))
            .build(
                event_loop,
                ConfigTemplateBuilder::new(),
                |mut configs| configs.next().expect("没有可用的 GL 配置"),
            )
            .expect("DisplayBuilder 建窗口失败");
        let window = window.expect("传入了 window attributes，应返回窗口");
        let display = Arc::new(config.display());

        let raw_handle = window.window_handle().expect("window handle").as_raw();
        let surface_attrs = SurfaceAttributesBuilder::<WindowSurface>::new().build(
            raw_handle,
            NonZeroU32::new(960).unwrap(),
            NonZeroU32::new(540).unwrap(),
        );
        let surface = unsafe {
            config
                .display()
                .create_window_surface(&config, &surface_attrs)
                .expect("create_window_surface 失败")
        };

        let ctx_attrs = ContextAttributesBuilder::new()
            .build(Some(window.window_handle().expect("window handle").as_raw()));
        let not_current = unsafe {
            config
                .display()
                .create_context(&config, &ctx_attrs)
                .expect("create_context 失败")
        };
        let gl_ctx = not_current
            .make_current(&surface)
            .expect("make_current 失败（GL 应在本线程）");

        // glow 的构造要读 GL_VERSION，必须在 context current 之后；
        // glutin 0.32 的 get_proc_address 收 &CStr，glow 正好有 CStr 版 loader
        let glow_display = Arc::clone(&display);
        let gl = unsafe {
            glow::Context::from_loader_function_cstr(move |name: &std::ffi::CStr| {
                glow_display.get_proc_address(name)
            })
        };

        // mpv：vo=libmpv（渲染 API 的前提），进渲染前建 render context
        let lib = match em_player::Lib::load_default() {
            Ok(lib) => lib,
            Err(e) => return self.fail(event_loop, e.to_string()),
        };
        let opts = PlayerOptions {
            vo: Some("libmpv".into()),
            // render() 里不再做最长 1s 的目标时间等待，渲染节奏交给 vsync
            extra: vec![("video-timing-offset".into(), "0".into())],
            ..Default::default()
        };
        let player = match Player::new(lib, &opts) {
            Ok(p) => p,
            Err(e) => return self.fail(event_loop, e.to_string()),
        };
        if let Err(e) = player.set_volume(self.volume) {
            return self.fail(event_loop, e.to_string());
        }

        let mpv_display = Arc::clone(&display);
        let mut render_ctx = match RenderContext::create(
            Arc::clone(player.lib()),
            &player,
            // mpv 传的是 C 字符串名；这里转一次 &str → CString（只在初始化/换流时少量调用）
            Box::new(move |name: &str| {
                let cname = CString::new(name).expect("GL 函数名不应含 NUL");
                mpv_display.get_proc_address(&cname)
            }),
        ) {
            Ok(rc) => rc,
            Err(e) => return self.fail(event_loop, e.to_string()),
        };

        // 新帧回调：mpv 内部线程 → 只发事件唤醒
        let update_counter = Arc::new(AtomicU32::new(0));
        let cb_counter = Arc::clone(&update_counter);
        let cb_proxy = self.proxy.clone();
        render_ctx.set_update_callback(Box::new(move || {
            cb_counter.fetch_add(1, Ordering::Relaxed);
            let _ = cb_proxy.send_event(UiEvent::Wake);
        }));

        if let Err(e) = player.load(&self.stream) {
            return self.fail(event_loop, e.to_string());
        }

        println!(
            "[gui] 窗口 960x540 已创建，播放中（音量 {}，运行 {} 秒）",
            self.volume, self.seconds
        );
        event_loop.set_control_flow(ControlFlow::Wait);

        self.running = Some(Running {
            window,
            surface,
            gl_ctx,
            gl,
            render_ctx,
            _player: player,
            _display: display,
            update_counter,
            start: Instant::now(),
            deadline: Duration::from_secs(u64::from(self.seconds)),
            frames: 0,
            next_report: Instant::now() + Duration::from_secs(1),
            finished: false,
        });
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, event: UiEvent) {
        if let Some(st) = &self.running {
            if !st.finished {
                match event {
                    UiEvent::Wake => st.window.request_redraw(),
                }
            }
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        let Some(st) = self.running.as_mut() else { return };
        match event {
            WindowEvent::Resized(size) => {
                if size.width > 0 && size.height > 0 {
                    st.surface.resize(
                        &st.gl_ctx,
                        NonZeroU32::new(size.width).expect("宽度非零"),
                        NonZeroU32::new(size.height).expect("高度非零"),
                    );
                    unsafe { st.gl.viewport(0, 0, size.width as i32, size.height as i32) };
                }
            }
            WindowEvent::RedrawRequested => {
                if st.finished {
                    return;
                }
                let size = st.window.inner_size();
                if size.width == 0 || size.height == 0 {
                    return;
                }
                st.surface.resize(
                    &st.gl_ctx,
                    NonZeroU32::new(size.width).expect("宽度非零"),
                    NonZeroU32::new(size.height).expect("高度非零"),
                );
                unsafe { st.gl.viewport(0, 0, size.width as i32, size.height as i32) };

                // 渲染当前帧（无新帧则重画上一帧）→ 交换 → 报告
                if let Err(e) = st.render_ctx.render(
                    size.width as i32,
                    size.height as i32,
                    true, // 默认 framebuffer 上下颠倒
                ) {
                    let msg = e.to_string();
                    self.finish(event_loop);
                    self.error = Some(format!("渲染失败：{msg}"));
                    return;
                }
                if st.surface.swap_buffers(&st.gl_ctx).is_err() {
                    self.finish(event_loop);
                    self.error = Some("swap_buffers 失败".into());
                    return;
                }
                st.render_ctx.report_swap();
                st.frames += 1;
                st.maybe_report();

                if st.start.elapsed() >= st.deadline {
                    self.finish(event_loop);
                }
            }
            WindowEvent::CloseRequested => {
                self.finish(event_loop);
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        // 兜底：流卡住、回调不来时也要能按时结束
        if let Some(st) = self.running.as_mut() {
            if !st.finished && st.start.elapsed() >= st.deadline {
                self.finish(event_loop);
            }
        }
        if self.error.is_some() || self.report.is_some() {
            event_loop.exit();
        }
    }
}

/// M1 冒烟入口：渲染 `stream` 一路 `seconds` 秒后截帧自检。
///
/// 会短暂弹出一个 960x540 的窗口。
pub fn smoke_single(
    stream: StreamUrl,
    seconds: u32,
    volume: u8,
    png_path: PathBuf,
) -> Result<SmokeReport, String> {
    let event_loop = EventLoop::<UiEvent>::with_user_event()
        .build()
        .map_err(|e| format!("创建事件循环失败：{e}"))?;
    let proxy = event_loop.create_proxy();
    let mut app = App {
        proxy,
        stream,
        seconds,
        volume,
        png_path,
        running: None,
        error: None,
        report: None,
    };
    event_loop
        .run_app(&mut app)
        .map_err(|e| format!("事件循环异常退出：{e}"))?;

    if let Some(err) = app.error {
        return Err(err);
    }
    app.report.ok_or_else(|| "未产生渲染结果".to_string())
}
