# Lumina

轻量、跨平台的图片查看器（参考 ImageGlass）。Rust + Tauri 2 + Vite/TypeScript。

> 当前进度：**阶段 1 — 脚手架 + 核心图像管线**。已打通「选择文件 → Rust 解码 → 前端显示 + 缩放」的纵向切片。

## 环境要求

| 工具 | 版本（已验证） |
| --- | --- |
| Rust / Cargo | 1.99.0 |
| Node.js | 24.14.0 |
| pnpm | 11.22.0 |
| 平台 | Windows 11（WebView2 随系统自带） |

包管理器统一使用 **pnpm**（不要用 npm / yarn）。

## 快速开始

```bash
pnpm install          # 安装前端依赖
pnpm tauri dev        # 启动开发模式（Vite + Rust 热重载）
pnpm tauri build      # 打包发布版本
```

单独校验：

```bash
pnpm build                                  # 前端类型检查 + 构建
cd src-tauri && cargo check                 # Rust 编译检查
cd src-tauri && cargo test                  # 图像管线单元 / 集成测试
```

## 架构

```
src/                         前端（原生 TS）
  main.ts                    UI：打开、显示、缩放/拖拽、上一张/下一张、缩略图条
  api.ts                     类型化 IPC 封装（唯一的前端调用入口）
  styles.css
src-tauri/src/
  lib.rs                     Tauri Builder：插件注册、命令注册、协议注册
  commands.rs                IPC 命令层
  imaging.rs                 核心管线：解码 / 元信息 / EXIF / 缩略图 / 转换（不依赖 Tauri，可单测）
  protocol.rs                lumina:// 自定义协议（流式传输图片字节）
  error.rs                   统一错误类型
docs/INTERFACES.md           ★ IPC 接口规范（前端 / 格式模块的契约）
```

### 图片传输方案

原图与缩略图都通过 **自定义协议 `lumina://`** 流式传输，不走 base64、不走 JSON IPC：

- WebView 原生支持的格式（png/jpg/gif/webp/bmp/ico）直接返回原始字节，零重编码；
- TIFF 等不支持的格式在 Rust 侧解码后转码为 PNG；
- 缩略图在 Rust 侧降采样（`mode=thumb&max=128`）后再传输，避免大图进城；
- `get_image_bytes` 命令作为兜底，在协议不可用时回退（返回原始字节，仍优于 base64）。

完整取舍分析见 [`docs/INTERFACES.md`](docs/INTERFACES.md#2-图片数据传输方案取舍重点大图性能)。

### 已支持格式

`png` `jpg/jpeg` `gif` `webp` `bmp` `tif/tiff` `ico`

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

`src-tauri/build.rs` 已注册 `cargo:rerun-if-changed=icons`，改图标后会重新嵌入 exe。已实测确认 exe 内嵌图标为新图标。

重新生成流程：

```bash
magick -background none -density 384 assets/icon.svg -resize 1024x1024 -depth 8 assets/app-icon.png
pnpm tauri icon assets/app-icon.png
```

## 快捷键

| 按键 | 功能 |
| --- | --- |
| `Ctrl+O` | 打开图片 |
| `←` / `→` | 上一张 / 下一张 |
| `+` / `-` | 放大 / 缩小（以光标为中心） |
| 滚轮 | 缩放 |
| 拖拽 | 平移 |
| `0` | 适应窗口 |
| `1` | 原始大小 |
| `i` | 显示/隐藏信息面板 |

## 验证状态

- ✅ `cargo check` 无警告
- ✅ `cargo test` — 6 项通过（含端到端管线：解码 → 元信息 → 缩略图 → 转码 → 目录列举）
- ✅ `pnpm build`（tsc + vite）通过
- ✅ `pnpm tauri dev` 可编译并启动应用进程（自动化环境中无法人工目视验证窗口渲染与 `lumina://` 加载）

## 已知限制

见 [`docs/INTERFACES.md` §8](docs/INTERFACES.md#8-已知限制--待办)：暂无解码缓存、EXIF 方向未自动应用、目录列举非递归、`save_image_as` 无质量参数。
