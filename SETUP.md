# ElderMonitor — 开发环境搭建（SETUP）

> 只管「把环境装起来」。装好之后怎么写、怎么测、怎么排错，看 **`docs/dev-guide.md`**。
> 权威设计规范见 `PROJECT.md`；使用者文档见 `README.md`。

## 平台说明

| 平台 | 状态 |
|---|---|
| Windows 10/11 x64 | 主开发与主运行平台 |
| macOS 12+ | **同步开发平台**：渲染主线（方案 A render API）跨平台，全部里程碑双平台一起调试 |

## 首次开工顺序（两台机器通用）

1. `git clone` 仓库
2. 装 Rust 工具链（需要 **rustc ≥ 1.85**，edition 2024；Windows 见 §1，macOS 见 macOS §1）
3. **获取 libmpv**：`tools\fetch-mpv.ps1` / `bash tools/fetch-mpv.sh`
4. **构建**：`tools\build.ps1` / `bash tools/build.sh`（含全部单测）
5. **跑冒烟**：`dist\em-smoke.exe <房间号>`，确认能出声、能打印实际分辨率

第 3 步不做**不会导致编译失败**——libmpv 是运行时动态加载的（§4.4），
只会在运行时报「无法加载 libmpv」并列出找过的路径。这是本项目与「编译期链接 mpv」的方案最大的区别。

预计时间：装工具链 10 分钟，fetch-mpv 1~3 分钟（30MB 归档），首次全量编译 1~2 分钟。

## Windows

### 1. Rust 工具链

```powershell
# rustup（官方安装器）
winget install Rustlang.Rustup
rustup default stable-x86_64-pc-windows-msvc
rustc --version
```

MSVC 目标需要 VS 2022（含 C++ 桌面开发工作负载）。

### 2. libmpv（用脚本获取，别手动折腾）

```powershell
tools\fetch-mpv.ps1 -DryRun      # 先看会拿到哪个版本，不下载
tools\fetch-mpv.ps1              # 下载最新 mpv-dev 并解压到 vendor\mpv
tools\fetch-mpv.ps1 -Force       # 已存在也重下
```

脚本做的事：查 GitHub 最新 release → 取 `mpv-dev-x86_64-*.7z` → 解压 → 落盘

```
vendor/mpv/libmpv-2.dll          运行期加载（打包进 dist）
vendor/mpv/include/mpv/*.h       编译期绑定
vendor/mpv/VERSION.txt           记录版本 + SHA256 打印
```

**网络不通时**（公司代理 / 无外网）：

```powershell
# 1) 走代理
tools\fetch-mpv.ps1 -Proxy http://127.0.0.1:7890

# 2) 或手动下载归档后再喂给脚本
#    https://github.com/shinchiro/mpv-winbuild-cmake/releases → mpv-dev-x86_64-*.7z
tools\fetch-mpv.ps1 -Archive D:\downloads\mpv-dev-x86_64-xxx.7z
```

解压优先用 7-Zip；没装则回退到系统自带 `tar.exe`（bsdtar 能读 7z）。两者都不行时脚本会提示 `winget install 7-Zip`。

**已验证的实际情况（2026-11-19 实测）**
- 最新发布：`20261111`，归档 `mpv-dev-x86_64-20261111-git-bcde6b93cc.7z`（30.3 MB）
- 解压后 `libmpv-2.dll` **约 117 MB**（dev 构建自带符号，属正常；`strip`/`objcopy` 实测无效，体积就是代码本体）
- 导出表完整：`mpv_create` / `mpv_client_api_version` / `mpv_render_context_create`（方案 A 依赖的渲染 API 在）
- ⚠️ **不要**用 `mpv-x86_64-*.7z`（播放器归档）：里面只有 `mpv.exe`，**没有** `libmpv-2.dll`，脚本会报「解压后没找到」
- zip 分发时这个 dll 会是包体主要体积来源

**注意**：`vendor/` 已在 `.gitignore` 里，dll 不进仓库；每台机器各自跑一次脚本。
不要把 dll 放进 PATH——路径由构建脚本管理。

### 3. 构建与运行

```powershell
tools\build.ps1                     # release 构建 + 全部单测 + 产物整理到 dist\
tools\build.ps1 -Profile debug      # 快速构建（调试用）
tools\build.ps1 -SkipTest           # 只构建
tools\build.ps1 -NoDist             # 只编译，不整理 dist
tools\build.ps1 -Smoke 7734200      # 构建后跑冒烟：解析该房间并出声 12 秒
```

等价的裸命令（脚本出问题时用）：

```powershell
cargo build --release
cargo test --workspace
copy target\release\em-smoke.exe dist\
copy vendor\mpv\libmpv-2.dll dist\
```

产物形态（M0）：

```
dist\
 ├─ em-smoke.exe        2.4 MB   M0 自测 CLI（M1 起会多出 ElderMonitor.exe）
 ├─ libmpv-2.dll      116.1 MB   播放内核
 └─ config\                     空目录，运行时写入 em.json
```

### 4. 跑冒烟（M0 的验收动作）

```powershell
.\dist\em-smoke.exe 7734200                     # 播 12 秒
.\dist\em-smoke.exe 7734200 --seconds 30        # 播久一点，看会不会断流
.\dist\em-smoke.exe --check-lib                 # 只查 dll 能否加载，并列出找过的路径
.\dist\em-smoke.exe 7734200 --no-play           # 只解析，不播
```

正常输出（2026-11-19 实测）：

```
[1/3] 解析 7734200
      房间   7734200 · 主播 未知
      候选   flv/avc qn=250 · flv/hevc qn=250 · hls/avc qn=250 ...
      状态   直播中 → 选中 flv/avc qn=250（请求档位 10000 = 原画）
      注意   接口把画质降到了 qn=250（未登录状态下可能如此），实际分辨率以下面 mpv 报告为准
[2/3] 加载 libmpv
      client API 2.6 · libmpv-2.dll
[3/3] 播放 12s（vo=null，仅音频，音量 40）
      事件   audio-reconfig（音频轨已建立）
      事件   file-loaded
      实际   视频 1280x720 · 音频 aac · 播放进度 11.7s
      结论   PASS
```

退出码：`0` 通过 · `1` 用法错误 · `2` 解析失败 · `3` 未开播 · `4` 播放器错误 · `5` 播放未达标。

**「出声」需要耳朵确认**：`audio-reconfig` + `audio-codec-name=aac` 只能证明音频轨建立并解码，
不能证明系统音量/输出设备正常。第一次在新机器上跑，请人工听一次。

### 5. 测试直播间

自测需要一个**稳定在播**的 B 站房间号（例如 7734200 这类赛事/官方常驻直播间）。
房间号不要写进仓库——`config/` 已 gitignore。

### 6. 常见问题

| 现象 | 原因 | 处理 |
|---|---|---|
| `无法加载 libmpv ... 已尝试：` | 没跑过 fetch 脚本，或 dll 不在 dist 里 | `tools\fetch-mpv.ps1`，然后重新 `build.ps1` |
| `curl: HTTP error 403` | 直链缺 `url_info.extra`（签名参数） | 看输出的「地址」长度，正常 700+ 字符；短就是解析层被改坏了 |
| `code=-3 / 未开播` | 房间没开播或已下播 | 换一个在播房间；轮播（live_status=2）也按未开播处理 |
| `性能`/`CPU` 相关疑问 | M0 只播音频 | 多路解码压力要到 M2 才测 |
| 中文输出乱码 | 控制台代码页 | `em-smoke` 启动时已把代码页设为 65001；若仍乱码，先 `chcp 65001` |

## macOS

### 1. 工具链

```bash
xcode-select --install
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
tools/fetch-mpv.sh            # brew 安装 mpv，并把 dylib + 头文件复制到 vendor/mpv
```

`tools/fetch-mpv.sh` 做的事：
- `brew install mpv`（已装则跳过）
- 复制 `$(brew --prefix mpv)/include/mpv` → `vendor/mpv/include/mpv`
- 复制 `libmpv.2.dylib` → `vendor/mpv/libmpv.2.dylib`，并用 `install_name_tool` 把 install name 改成 `@loader_path/libmpv.2.dylib`（打包时不必依赖 brew 路径）
- 版本写进 `vendor/mpv/VERSION.txt`

手动等价命令（脚本坏了时用）：

```bash
brew install mpv
PREFIX=$(brew --prefix mpv)
mkdir -p vendor/mpv/include && cp -R "$PREFIX/include/mpv" vendor/mpv/include/
cp "$PREFIX/lib/libmpv.2.dylib" vendor/mpv/libmpv.2.dylib
```

### 2. 注意

- 渲染主线（方案 A）跨平台，macOS 从 M1 起与 Windows 同步调试
- macOS OpenGL context 走 CGL；mpv 渲染 API 原生支持（IINA 同款路线）
- M0 的 `em-smoke` CLI（解析 + 音频播放）在 mac 上即可验证 em-resolve 与 em-player，
  不需要等窗口层
- 方案 B（child window）仅是 Windows 应急回退，mac 上编译不过属预期，忽略之
- 不需要单独建 mac 工程，同一 workspace，`cargo build` 即可
- 脚本用 `bash tools/fetch-mpv.sh` / `bash tools/build.sh` 调用（仓库在 Windows 上提交，可能没有可执行位）

### 3. 构建与冒烟

```bash
bash tools/build.sh                    # release + 测试 + dist/
bash tools/build.sh debug              # 快速构建
bash tools/build.sh release --no-test  # 只构建
bash tools/build.sh release 7734200    # 构建后跑冒烟
```

产物：`dist/em-smoke` + `dist/libmpv.2.dylib` + `dist/config/`。

```bash
./dist/em-smoke 7734200              # 播 12 秒
./dist/em-smoke --check-lib          # 只查 dylib 能否加载，列出找过的路径
```

dylib 查找顺序与 Windows 相同（`EM_MPV` → 可执行文件同目录 → 向上找 `vendor/mpv/` →
系统路径 → `/opt/homebrew/lib` → `/usr/local/lib`），所以从 `target/debug` 直接跑也能找到。

> **M0 在 macOS 上尚未实跑**（开发机是 Windows）。脚本只做了 `bash -n` 语法检查。
> 首次在 mac 上跑，若 `fetch-mpv.sh` 报 formula 路径不对（新版 brew 可能把 libmpv 拆出独立 formula），
> 把报错发出来改脚本即可；`build.sh` 的问题同理。

## 代码库约定（对齐用户其他项目）

- 提交信息只写「变动内容总结」，禁止过程叙述
- 版本号只留给真机验证通过的构建；过程包用 `-devN` 后缀
- `git push` / 发 release 由用户手动执行
- 带中文的 `.ps1` 必须存成 **UTF-8 with BOM**，否则 PowerShell 5.1 按 ANSI 解码会报语法错
- `[CmdletBinding()]` 脚本的参数默认值里**不要用 `$PSScriptRoot`**（`-File` 调用时为空串）
- 文档分三条线：
  - `README.md` 只面向使用者（安装、操作、常见问题）
  - `docs/dev-guide.md` 面向开发者 / agent：仓库结构、命令、测试、排错、里程碑进度
  - `PROJECT.md` 是设计与规范的**单一事实源**；`SETUP.md`（本文件）只管环境搭建
