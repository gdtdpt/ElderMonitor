# ElderMonitor 开发者手册（dev-guide）

> 面向开发者 / agent。环境搭建看 `SETUP.md`，设计规范看 `PROJECT.md`（单一事实源）。
> 使用者文档是 `README.md`——**不要**把开发者内容写进 README。
>
> 当前进度：**M0 完成**（2026-11-19，Windows 实测；macOS 待首跑）→ 下一个里程碑是 M1 单路直渲。

---

## 1. 这个项目在做什么

整屏排列多个 B 站直播间、每路独立音量。核心是三层：

```
em-resolve  房间号 → flv 直链 + Referer       （网络）
em-player   flv 直链 → libmpv 实例 → 声音/画面 （解码）
em-ui       多路 → 布局 → 叠加控件             （M1 起）
```

布局不是固定网格：格子数 = 显示列长度，位置由「每路宽高比 + 是否主格 + 记录库是否钉住」
算出来（算法见 `PROJECT.md` §4.5）。

## 2. 目录结构

```
C:\Users\<你>\Develop\ElderMonitor\
 ├─ crates/
 │   ├─ em-core/          纯逻辑：配置、会话、几何。无网络无窗口，随便单测
 │   ├─ em-resolve/       B 站解析：normalize_room / resolve / join_url
 │   ├─ em-player/        libmpv 封装：动态加载、实例、属性、事件
 │   ├─ em-ui/            M0 为空占位；M1 起放 winit + OpenGL + 控件
 │   └─ em-smoke/         bin：em-smoke，M0 的验收工具
 ├─ tools/
 │   ├─ fetch-mpv.ps1/.sh   下载 libmpv 到 vendor/mpv（dll 不进 git）
 │   └─ build.ps1/.sh       构建 + 测试 + 整理 dist
 ├─ vendor/mpv/           gitignore。libmpv-2.dll + include/mpv/*.h
 ├─ dist/                 gitignore。可直接运行的产物
 ├─ docs/dev-guide.md     本文件
 ├─ PROJECT.md  SETUP.md  README.md
 └─ Cargo.toml            workspace，edition 2024，MSRV 1.85
```

依赖方向单向：`em-ui → em-player → em-resolve → em-core`，`em-smoke → em-player + em-resolve`。
**不要**为了省事让 `em-core` 去依赖网络或窗口——它必须能在任何机器上纯计算、纯单测。

## 3. 五分钟跑起来

```powershell
git clone https://github.com/gdtdpt/ElderMonitor.git
cd ElderMonitor
tools\fetch-mpv.ps1          # 30MB 归档 → vendor\mpv\libmpv-2.dll（117MB）
tools\build.ps1              # release + 47 个测试 + dist\
.\dist\em-smoke.exe 7734200  # 应该出声，最后一行 PASS
```

macOS 把两条命令换成 `bash tools/fetch-mpv.sh` 和 `bash tools/build.sh`。

第一次跑就失败的话，按这个顺序查：

| 卡在哪 | 看什么 |
|---|---|
| `fetch-mpv` | 网络/代理（`-Proxy`）、7-Zip 是否存在、是否误用了 `mpv-x86_64-*.7z`（那个没有 dll） |
| `build` | `rustc --version` 是否 ≥ 1.85；报错贴全文 |
| `em-smoke` 报无法加载 libmpv | 用 `--check-lib` 看它找过哪些路径 |
| `em-smoke` 报 403 | 直链缺 `url_info.extra`，看 `/4` 节 |

## 4. 常用命令

```powershell
# 构建 / 测试
cargo build                                   # 全 workspace（debug）
cargo build --release
cargo test --workspace                        # 47 个测试
cargo test -p em-core                         # 只跑某一层
cargo test -p em-resolve -- normalize         # 按名字过滤
cargo clippy --workspace --all-targets        # 建议提交前跑

# 只跑某个 crate 的二进制
cargo run -p em-smoke -- 7734200 --seconds 20

# 冒烟（三种粒度）
.\dist\em-smoke.exe --check-lib               # 只验证 dll
.\dist\em-smoke.exe 7734200 --no-play         # 只验证解析（不打网络音频）
.\dist\em-smoke.exe 7734200 --seconds 30      # 完整链路 + 观察是否断流
```

`em-smoke` 退出码：`0` 通过、`1` 用法错误、`2` 解析失败、`3` 未开播、`4` 播放器错误、`5` 播放未达标。

## 5. 代码地图

### em-core

| 文件 | 关键项 |
|---|---|
| `src/config.rs` | `Config { records, visible, primary, library, window }`、`Record`、`Library::reserved_width()`、`Config::sanitize()`（修跨字段不一致）、`load/save`（**原子写**：tmp + rename） |
| `src/session.rs` | `Session`：`add_record`（按 room 去重）、`show/hide/toggle_show`（12 路上限）、`replace_slot`（已在别格则交换）、`set_primary`、`remove_record`、`set_volume`（夹 0..=100） |
| `src/geom.rs` | `Rect`（`fit_aspect` / `split_*` / `columns` / `rows`）、`aspect_from_size` |

不变量（改 `Session` 时必须守住）：`visible` ⊆ `records`、`visible` 无重复且 ≤ 12、
`primary ∈ visible`、同一 `room` 只有一条记录。`Session::new` 会先跑一次 `sanitize`。

### em-resolve

| 项 | 说明 |
|---|---|
| `normalize_room` | 房间号 / 链接 / 带标题的粘贴文本 → `u64` |
| `resolve` | 请求 → 解析 → 候选排序 → `Resolved { room_id, live, title, uname, candidates, stream }` |
| `join_url` | `host + base_url + extra`，对 `?`/`&` 前缀容错。**改这里先看 `/6` 的 403 坑** |
| `rank` | 协议（flv>hls）→ qn（高优先）→ 编码（avc>hevc） |
| 常量 | `REQUEST_QN=10000`、`REFERER`、`USER_AGENT`、`HTTP_TIMEOUT=15s` |

未开播（`live_status != 1`）返回 `live=false` + `stream=None`，**不是错误**——UI 该显示灰格。

### em-player

| 项 | 说明 |
|---|---|
| `ffi.rs` | 只声明用到的 C ABI，数值**照 `vendor/mpv/include/mpv/client.h` 抄的**，改前先核对头文件 |
| `Lib::load_default` | 候选路径查找（`EM_MPV` → exe 同目录 → 向上找 `vendor/mpv` → cwd → 系统路径 → Homebrew） |
| `Player::new` | 设选项 → `mpv_initialize` → 开 `warn` 日志。被拒的选项进 `warnings()` |
| `Player::load` | 先设 `referrer`/`user-agent`，再 `loadfile <url> replace` |
| `Player::wait_event` | 把 `mpv_event` 裸指针转成 `Event` 枚举（`FileLoaded`/`EndFile{reason}`/`Log`…） |
| `Drop` | `terminate_destroy`。**别用 `mem::forget`**，会残留解码线程与连接 |

`EndReason::needs_reconnect()` 是 M1 断流自续的判据（`Eof` / `Error` → 重新解析）。

### em-smoke

参数解析（手写，无 clap）→ 解析 → 加载库 → 建 `Player(audio_only)` → 事件循环 →
采样 `width`/`height`/`audio-codec-name`/`time-pos` → 判定 PASS/FAIL。
Windows 启动时会把控制台代码页设成 65001，否则中文乱码。

## 6. 已知坑（都是实测踩过的，别重踩）

1. **`base_url` 不含签名参数**。接口返回的只是 `/live-bvc/...xxx.flv?`（87 字符），
   真正的 `expires` / `oi` / `qn` 全在 `url_info[].extra`。漏了它 → CDN 403。
   判据：正常地址约 700+ 字符。
2. **`http-header-fields` 不能用来带 UA**。它是 StringList，按逗号分割，而 UA 里就有逗号。
   用 mpv 的 `referrer` / `user-agent` 属性。
3. **PowerShell 5.1 会把原生命令的 stderr 当 ErrorRecord**。`build.ps1` 里的 `Invoke-Native`
   就是为此写的；不要直接 `& cargo build` 配 `$ErrorActionPreference='Stop'`。
4. **别用 `mpv-x86_64-*.7z`** 取 dll，那个归档只有 `mpv.exe`。要用 `mpv-dev-x86_64-*.7z`。
5. **dll 117MB 是真实的**，`strip` / `objcopy --strip-all` 一字节不减。
6. **未登录只有 720p**。请求 `qn=10000` 返回 `current_qn=250`，不报错、静默降级。
   判断实际画质只能看 mpv 的 `width/height`。
7. **`libmpv` 是运行时加载**，所以编译通过 ≠ 能跑。新机器一定要 `--check-lib` 一次。

## 7. 测试策略

| 层 | 做法 |
|---|---|
| `em-core` | 纯单测，覆盖不变量与边界（现 32 个）。**任何新逻辑先进这一层** |
| `em-resolve` | 纯函数单测（`normalize_room`、`join_url`、`rank`）+ 真机手测；**单测不发网络** |
| `em-player` | 只测能测的（end reason 映射、候选路径）；`loads_libmpv_when_available` 在 dll 缺失时**自动跳过**，所以新机器没跑 fetch 时 `cargo test` 也能绿 |
| 全链路 | `em-smoke`（真房间、真出声） |

约定：新增解析/布局逻辑必须先补单测；UI 层（M1 起）用「启动即截图」自闭环，不来回找用户截图。

## 8. 下一步（M1 单路直渲）

目标：一个窗口显示一路直播画面，10 分钟不断流。

1. `em-ui` 加依赖：`winit`（窗口 + 事件循环）、`glutin`（创建 OpenGL context，Windows 走 WGL、
   macOS 走 NSOpenGL/CGL）、`glow`（GL 函数指针）。**版本在动手时确认，别抄本文的版本号**
2. `em-player` 补渲染 API 的 FFI：`mpv_render_context_create` / `mpv_render_context_render` /
   `mpv_render_context_report_swap` / `mpv_render_context_set_update_callback`，
   参数结构 `mpv_opengl_init_params`、`MPV_RENDER_PARAM_OPENGL_INIT_PARAMS`
   （`vendor/mpv/include/mpv/render_gl.h` 有完整声明）
3. 把 `mpv_render_context` 绑到当前 GL context，渲染回调里请求重绘
4. 断流自续：监听 `EndFile` → `needs_reconnect()` → 重新 `resolve` → `Player::load`
5. 验收：两台机器各跑 10 分钟，画面不断、声音不断

**M1 是风险闸门**：interop 走不通就按 `PROJECT.md` §5 切方案 B（Windows-only）。

## 9. 提交与发布

- 提交信息只写「变动了什么」，不写过程和理由（理由进 `PROJECT.md` §6 决策表）
- 一个故障只消耗一个正式版本号；验证没通过不进位，中间包用 `-devN`
- 版本号一经发布不可复用（先 `git ls-remote --tags origin refs/tags/vX.Y.Z` 查远端）
- `git push` 和建 release 由用户手动执行
- 里程碑验收流程：构建 → 单测 → UI 冒烟（截图自闭环）→ **用户验收通过** → 才 commit/tag
