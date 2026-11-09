<div align="center">

# Lumina

**光。让看图这件事，回到它本该有的样子。**

轻量、克制的跨平台图片查看器 · Rust + Tauri 2 + TypeScript

</div>

---

## 理念

> 光圈打开，光落进传感器；快门合上，那一刻被留下来。
> 我们做 Lumina，是想让「看」这一步不再有阻力——不打扰，不卡顿，把注意力还给照片本身。

市面上的看图工具，要么重得需要等它启动，要么花哨得遮住了画面。Lumina 的取舍只有四个字：

- **轻** — 纯 Rust 依赖，不引入任何原生工具链；发布包小而干净。
- **快** — 打开即显示，滚动即出图；大图不卡主线程，目录再大也不发怵。
- **净** — 界面克制、留白充分，深色为底；需要时工具栏与信息面板才出现。
- **真** — 能直通的原字节绝不重编码；EXIF 方向正确，元信息完整，所见即所得。

它不是功能最多的那个，而是最想让你忘记它存在的那一个。

---

## 特性

### 看得清：格式与信息

| 能力 | 说明 |
| --- | --- |
| 常见格式 | `png` `jpg/jpeg` `gif` `webp` `bmp` `tif/tiff` `ico` |
| 矢量图 | `svg` —— 元信息与缩略图由 `resvg`（纯 Rust）按 96 DPI 栅格化，原图交给 WebView 原生渲染 |
| 动图 | GIF / 动图 WebP 原生播放，并可暂停、逐帧查看 |
| 多页 | 多页 TIFF 逐页翻阅（支持 8/16 位 RGB / RGBA / Gray / GrayA） |
| 元信息 | 尺寸、文件大小、格式；相机 / 镜头 / 拍摄时间 / 曝光 / 光圈 / ISO / 焦距 / 作者 / 版权 / 描述，以及 GPS 经纬度与海拔 |
| 方向 | 自动应用 EXIF Orientation——竖拍照片的缩略图与大图方向始终一致 |

> AVIF / HEIC 经评估后**有意推迟**：其纯 Rust 解码器尚不成熟，引入原生依赖会破坏「轻」的原则。取舍详见 [`docs/INTERFACES.md`](docs/INTERFACES.md)。

### 看得顺：为速度而设计

大图不看图，看图的是人。所以每一条路径都为「快」让路：

- **自定义协议 `lumina://` 流式传图** —— 图片字节直接喂给浏览器的图片解码器，**不走 base64、不过 JSON**。一个 40 MB 的 JPEG，base64 方案要变成 53 MB 的字符串再经历序列化；Lumina 让它保持 40 MB，且零质量损失。
- **原字节直通** —— WebView 能渲染的格式（含 JPEG / PNG / GIF / WebP / BMP / ICO / SVG）直接返回原始文件字节，**不解码、不重编码**；TIFF 等不支持的格式在 Rust 侧解码后转码为 PNG。
- **缩略图在 Rust 侧降采样** —— 先把大图缩到 192 px 再传输，绝不让原始大图进城。
- **进程内 LRU 缓存** —— 缩略图按 `(路径, 修订, 尺寸)` 缓存（512 条），来回滚动不再重复解码；带版本令牌的请求回 `Cache-Control: immutable`。
- **懒加载 + 并发池** —— 缩略图用 `IntersectionObserver` 只加载进入视口的部分，并限制最多 6 个并发请求，快速滚动也不会打满 CPU。
- **相邻预加载** —— 打开一张图时顺手预热前后邻居，上一张 / 下一张近乎瞬时。

### 改得动：轻量编辑

查看器不该逼你打开另一个软件。旋转、翻转、裁剪、另存、格式转换都在原地完成：

- **即时预览**：旋转 / 翻转用 CSS 变换实时呈现，**所见即所得**。
- **裁剪**：在图上直接框选，或输入精确的源图坐标；预览与后端按**同一顺序**（`裁剪 → 翻转 → 旋转 → 缩放`）执行，因此屏幕上的选区就是最终写出的结果。
- **文字识别**：在图上识别简体中文和英文，可框选局部区域、点选文字，并复制全部或选中内容。识别在本机运行；OCR 模型首次使用时按需下载并缓存。
- **另存 / 转换**：输出到 `png` / `jpg` / `jpeg` / `gif` / `webp` / `bmp` / `tif` / `tiff` / `ico`，JPEG 可调质量。
- **调整分辨率**：按指定像素尺寸或百分比缩放，可保持比例、选择插值算法，并另存为新文件或确认后覆盖原图。
- **文件操作**：重命名、删除（二次确认，不可恢复）、复制路径。

> 裁剪与旋转/翻转编辑写入**新文件**；分辨率调整可另存或在二次确认后覆盖原图。后端会在写出前完成参数校验。

### 用着舒服：界面与操作

- 顶栏工具组 + 可收纳的**缩略图侧栏** + **信息面板** + 状态栏（文件名 / 尺寸 / 格式 / 序号 / 缩放比）。
- **幻灯片**播放（可选间隔）、**全屏**浏览、**深色 / 浅色**主题（默认跟随系统外观）。
- **拖放**图片或文件夹即可打开。
- 完整键盘快捷键，贴合 ImageGlass 的操作直觉。

### 接得上系统：文件关联与专属图标

设置面板（`Ctrl+,`）里可以把 Lumina 注册为图片的默认打开方式，**并让每种格式在资源管理器中显示自己的图标**——PNG、JPEG、GIF、WebP、BMP、TIFF、ICO、SVG 各一个字形，配色各不相同。

- 关联写在 **`HKCU`**（当前用户），**不需要管理员权限**，可随时一键撤销。
- 每个扩展名一个独立 ProgID，因此图标能按格式区分（共用一个 ProgID 就只能共用一张图）。
- 撤销时只删自己的键，并还原关联前的原默认程序。

各平台的**能力边界**不同，面板会如实说明：

| | Windows | macOS | Linux |
| --- | --- | --- | --- |
| 自行注册关联 | ✅ | ❌（构建期固定） | ✅ |
| 静默设为默认 | ❌（需用户在系统设置确认一次） | ❌ | ✅ |
| 按格式自定义图标 | ✅ | ❌ | ❌ |

> Windows 8 起默认程序受 `UserChoice` 哈希保护，**任何程序都无法静默抢默认**——面板因此只说「注册 + 引导确认」，不承诺一键设为默认。细节见 [`docs/INTERFACES.md` §10](docs/INTERFACES.md#10-文件关联与每格式文件图标)。

---

## 快速开始

### 环境要求

| 工具 | 版本（已验证） |
| --- | --- |
| Rust / Cargo | 1.99.0 |
| Node.js | 24.14.0 |
| pnpm | 11.22.0 |

目标平台为 Windows / macOS / Linux（当前以 Windows 11 完成实机验证；WebView2 随系统自带）。包管理器统一使用 **pnpm**（不要用 npm / yarn）。

```bash
pnpm install          # 安装前端依赖
pnpm tauri dev        # 启动开发模式（Vite + Rust 热重载）
pnpm tauri build      # 打包发布版本
```

### 本机安装包构建与静默安装

在 Windows 上运行会构建 NSIS 安装包并以 `/S` 静默安装；在 macOS 上构建 DMG，并静默安装到当前用户的 `~/Applications/Lumina.app`（不需要管理员权限）：

```bash
pnpm package:install                    # 构建并静默安装
pnpm package:install -- --build-only    # 只构建安装包
pnpm package:install -- --install-only  # 安装最近构建的安装包
```

脚本只支持在对应系统本机打包：Windows 生成 NSIS，macOS 生成 DMG。首次运行前需安装项目要求的 Rust、Node.js、pnpm，以及 Tauri 对应平台的构建依赖。

单独校验：

```bash
pnpm build                                  # 前端类型检查 + 构建
cd src-tauri && cargo check                 # Rust 编译检查
cd src-tauri && cargo test                  # 图像管线单元 / 集成测试
```

---

## 架构

```
src/                         前端（原生 TS）
  main.ts                    UI：查看 / 缩放 / 拖拽 / 缩略图 / 编辑 / 文件操作 / 设置
  api.ts                     类型化 IPC 封装（唯一的前端调用入口）
  styles.css
assets/
  icon.svg                   应用图标设计源（1024×1024）
  file-icons/*.svg           每格式文件图标设计源（由 scripts/make-file-icons.py 生成）
src-tauri/src/
  lib.rs                     Tauri Builder：插件注册、命令注册、异步协议注册
  commands.rs                IPC 命令层（重活命令均为 async）
  imaging.rs                 核心管线：解码 / 元信息 / EXIF / 缩略图 / 转换（不依赖 Tauri，可单测）
  protocol.rs                lumina:// 自定义协议（异步、流式传输图片字节）
  cache.rs                   缩略图进程内 LRU 缓存
  file_assoc/                系统文件关联：mod（门面）/ plan（注册表布局，纯函数）/ windows / macos / linux
  bin/gen-file-icons.rs      SVG → 多尺寸 .ico（开发工具，产物已入库）
  error.rs                   统一错误类型
docs/INTERFACES.md           ★ IPC 接口规范（前端 / 格式模块的契约）
```

分层原则：`imaging.rs` 与 `file_assoc/plan.rs` 都不依赖任何 Tauri 类型，可独立单测；`commands.rs` 只做参数适配；前端所有 IPC 调用必须经过 `src/api.ts`。

### 图片传输方案

原图与缩略图都通过 **自定义协议 `lumina://`** 流式传输，不走 base64、不走 JSON IPC：

- WebView 原生支持的格式（png/jpg/gif/webp/bmp/ico/svg）直接返回原始字节，零重编码；
- TIFF 等不支持的格式在 Rust 侧解码后转码为 PNG；
- 缩略图在 Rust 侧降采样（`mode=thumb&max=192`）后再传输，避免大图进城；
- `get_image_bytes` 命令作为兜底，在协议不可用时回退（返回原始字节，仍优于 base64）。

完整取舍分析见 [`docs/INTERFACES.md`](docs/INTERFACES.md#2-图片数据传输方案取舍重点大图性能)。

---

## 图标

除应用图标外，Lumina 还为**每种图片格式**配了一张文件图标（供系统关联使用）。两者流程一致：SVG 设计源 → 生成产物 → 入库。

### 应用图标

源文件 [`assets/icon.svg`](assets/icon.svg)（1024×1024，另有栅格化产物 `assets/app-icon.png`）。

- **设计意象**：光圈（镜头叶片）+ 从中透出的光，把应用名 **Lumina（光）** 与「看图」这件事合成一个符号。
- **造型**：圆角方形底板（squircle，圆角半径 = 0.219 × 边长），深蓝对角渐变底 + 蓝色六叶光圈 + 中心白色光晕；光圈几何取自 Lucide `aperture` 图标（24×24 网格），缩放 32 倍并按小尺寸加大线宽。
- **配色**：底板 `#0E1420 → #1C2637`，符号 `#6EA8FE`，点睛为白色径向光。扁平、双色为主、单层渐变。
- 已用**像素级缩放自检** 16 / 32 px：32 px 清晰可辨，16 px 仍能看出「光圈 + 中心亮点」。

```bash
magick -background none -density 384 assets/icon.svg -resize 1024x1024 -depth 8 assets/app-icon.png
pnpm tauri icon assets/app-icon.png
```

`pnpm tauri icon` 派生全平台图标并替换 `src-tauri/icons/` 下的默认图标：

| 平台 | 文件 |
| --- | --- |
| Windows | `icon.ico`（16/24/32/48/64/256，六种尺寸） |
| macOS | `icon.icns` |
| Linux / 通用 | `32x32.png` `64x64.png` `128x128.png` `128x128@2x.png` `icon.png` |
| Windows Store | `Square*Logo.png` `StoreLogo.png` |
| 移动端 | `android/`、`ios/` |

`src-tauri/build.rs` 已注册 `cargo:rerun-if-changed=icons`，改图标后会重新嵌入 exe。

### 每格式文件图标

设计源 `assets/file-icons/*.svg`（8 个，对应 PNG / JPEG / GIF / WebP / BMP / TIFF / ICO / SVG）：

```bash
python3 scripts/make-file-icons.py                # 生成 SVG
cd src-tauri && cargo run --bin gen-file-icons    # 栅格化为 16/24/32/48/64/256 六尺寸 .ico
```

- **造型**：与应用图标同构的 squircle 底板，但底板是**浅色**——`#F8FAFC → #E2E8F0` 对角渐变，外加 `#0F172A`（10% 不透明度）细描边；中央是各格式**强调色的字形缩写**，再套一圈同色柔光晕——光晕是品牌「光」的延续，也是 16 px 下唯一还分辨得出的信息（字形此时已糊成色块）。注意应用图标本身目前仍是深蓝底板（见上），两者尚未统一。
- **产物**：`src-tauri/icons/file-icons/<key>.ico`，随 `bundle.resources` 打包，已提交入库。缺尺寸会让资源管理器在对应视图下放大邻近尺寸，图标就糊了。
- **字形是描边路径，不是 `<text>`**：用 `<text>` 会让栅格化结果依赖本机字体——换台机器生成的 `.ico` 字节不同，且与应用内预览（webview 用系统字体）不一致。字形按**墨迹宽度**排版，因此相邻字母的光学间距一致（否则 `GIF` 里的 `I` 会被两侧吃掉，看着像 `GF`）。

---

## 快捷键

| 按键 | 功能 |
| --- | --- |
| `Ctrl+O` | 打开图片 |
| `Ctrl+S` | 另存为 / 转换格式 |
| `←` `↑` `PageUp` `Backspace` | 上一张 |
| `→` `↓` `PageDown` `Enter` | 下一张 |
| `Home` / `End` | 第一张 / 最后一张 |
| `+` / `-` | 放大 / 缩小（以中心） |
| 滚轮 | 缩放（以光标为中心） |
| 拖拽 | 平移 |
| `F` / `0` | 适应窗口 |
| `1` | 原始大小 |
| `R` / `L` | 向右 / 向左旋转 90°（预览） |
| `H` / `V` | 水平 / 垂直翻转（预览） |
| `Shift+0` | 重置旋转 / 翻转 |
| `C` | 进入 / 退出裁剪模式 |
| `O` | 识别当前图片文字 |
| `T` / `B` | 显示 / 隐藏缩略图侧栏 |
| `I` | 显示 / 隐藏信息面板 |
| `空格` | 播放 / 暂停幻灯片 |
| `F11` / 双击 | 全屏 |
| `Esc` | 退出区域框选 / OCR 面板 / 裁剪 / 全屏 / 关闭菜单 / 停止幻灯片 |

---

## 应用图标

图标源：[`assets/icon.svg`](assets/icon.svg)（1024×1024，另有栅格化产物 `assets/app-icon.png`）。

- **设计意象**：光圈（镜头叶片）+ 从中透出的光，把应用名 **Lumina（光）** 与「看图」这件事合成一个符号。
- **造型**：圆角方形底板（squircle，圆角半径 = 0.219 × 边长），深蓝对角渐变底 + 蓝色六叶光圈 + 中心白色光晕；光圈几何取自 Lucide `aperture` 图标（24×24 网格），缩放 32 倍并按小尺寸加大线宽。
- **配色**：底板 `#0E1420 → #1C2637`，符号 `#6EA8FE`，点睛为白色径向光。扁平、双色为主、单层渐变。
- 已用**像素级缩放自检** 16 / 32 px：32 px 清晰可辨，16 px 仍能看出「光圈 + 中心亮点」。

已通过 `pnpm tauri icon assets/app-icon.png` 派生全平台图标并替换 `src-tauri/icons/` 下的默认图标：

| 平台 | 文件 |
| --- | --- |
| Windows | `icon.ico`（16/24/32/48/64/256，六种尺寸） |
| macOS | `icon.icns` |
| Linux / 通用 | `32x32.png` `64x64.png` `128x128.png` `128x128@2x.png` `icon.png` |
| Windows Store | `Square*Logo.png` `StoreLogo.png` |
| 移动端 | `android/`、`ios/` |

`src-tauri/build.rs` 已注册 `cargo:rerun-if-changed=icons`，改图标后会重新嵌入 exe。重新生成流程：

```bash
magick -background none -density 384 assets/icon.svg -resize 1024x1024 -depth 8 assets/app-icon.png
pnpm tauri icon assets/app-icon.png
```

---

## 验证状态

**在本机（macOS）已完成：**

- ✅ `cargo check --all-targets` 无警告
- ✅ `cargo test` — 39 项通过（端到端管线：解码 → 元信息 → 缩略图 → 转码 → 目录列举；缩略图 LRU 缓存；格式 × 结果矩阵；EXIF Orientation 角像素断言；**文件关联的注册表布局断言 + 图标存在性**）
- ✅ `pnpm build`（tsc + vite）通过
- ✅ 每格式图标已生成并校验 ICO 结构（`type=1`、6 个尺寸、`bWidth=0` 表示 256、PNG 帧签名）
- ✅ 设置面板在深色 / 浅色两套主题下截图自查；图标在 16 / 32 / 64 / 256 px 四档检查可辨性
- ✅ `pnpm tauri dev` 可编译并启动应用进程

**必须到 Windows 11 验证**（本机是 macOS，这部分只能静态保证，无法运行）：

- ⏳ 文件关联注册表的实际落盘，及在「打开方式」「默认应用」中的可见性
- ⏳ `SHChangeNotify` 是否即时刷新资源管理器图标
- ⏳ 各视图尺寸下 16 / 24 px 图标的实际观感（`image` 0.25 的 ICO 编码器只产出 PNG 帧，微软惯例是小尺寸用 BMP DIB）
- ⏳ 撤销关联后是否还原用户原有的默认程序

## 已知限制

见 [`docs/INTERFACES.md` §8](docs/INTERFACES.md#8-已知限制--待办)。摘要：`mode=full` 原始字节暂无缓存、目录列举非递归且不分页（超大目录需虚拟化）、AVIF / HEIC 解码未支持、有损 WebP 编码未支持、`transform_image` 为整图处理（大图一次性占用内存较多）。

文件关联的平台边界见 [`docs/INTERFACES.md` §10](docs/INTERFACES.md#10-文件关联与每格式文件图标)：Windows 无法静默设为默认（`UserChoice` 哈希保护），macOS 无法运行时建立关联，每格式图标仅 Windows 支持。
