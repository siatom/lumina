# Lumina

[English](#english) | [简体中文](#简体中文)

<a id="english"></a>

Lumina is a lightweight, cross-platform image viewer for browsing, organizing, and making simple edits to images.

## Features

- **Browse images**: Open an image or folder, navigate between images, zoom, use full-screen and slideshow modes, drag and drop files, and browse thumbnails.
- **Support common formats**: PNG, JPEG, GIF, WebP, BMP, TIFF, ICO, and SVG. Play animated GIF and WebP files, and browse multi-page TIFF images.
- **View image details**: See dimensions, format, file size, and available EXIF metadata, including camera, lens, exposure, date, and GPS information. EXIF orientation is applied automatically.
- **Edit and convert**: Rotate, flip, crop, resize, save a copy, or convert to common image formats. JPEG quality is adjustable.
- **Recognize text**: Run on-device OCR for Simplified Chinese and English on the whole image or a selected region, then copy the results. OCR language data is downloaded on first use.
- **Manage files**: Rename or delete images and copy file paths.
- **Integrate with your system**: Configure image file associations. Windows can display a distinct file icon for each supported format. Check for and install signed updates from Settings.
- **Light and dark themes**: Follow the system appearance or switch themes in the app.

> AVIF and HEIC are not currently supported. File association capabilities vary by platform. On Windows, you must confirm Lumina as the default app in System Settings.

## Keyboard shortcuts

| Shortcut | Action |
| --- | --- |
| `Ctrl+O` | Open an image |
| `←` / `→` | Previous / next image |
| `+` / `-` | Zoom in / out |
| `F` / `0` | Fit to window |
| `1` | Actual size |
| `R` / `L` | Rotate preview right / left |
| `C` | Enter / exit crop mode |
| `O` | Recognize text in the image |
| `T` | Show / hide thumbnails |
| `I` | Show / hide image information |
| `F11` / double-click image | Toggle full screen |
| `Esc` | Exit the current operation or full screen |

## Development

Requires Rust, Node.js, and pnpm. Install dependencies and start the development app:

```bash
pnpm install
pnpm tauri dev
```

Build the app:

```bash
pnpm tauri build
```

See [`docs/INTERFACES.md`](docs/INTERFACES.md) for development interfaces and implementation details.

---

<a id="简体中文"></a>

## 简体中文

Lumina 是一款轻量的跨平台图片查看器，支持浏览、整理和简单编辑图片。

### 功能

- **浏览图片**：打开图片或文件夹，在图片间快速切换；支持缩放、全屏、幻灯片、拖放打开和缩略图浏览。
- **支持常见格式**：PNG、JPEG、GIF、WebP、BMP、TIFF、ICO 和 SVG。支持 GIF / WebP 动图播放，以及多页 TIFF 翻页。
- **查看图片信息**：显示尺寸、格式、文件大小及可用的 EXIF 信息，包括相机、镜头、拍摄参数、时间和 GPS 信息；自动校正 EXIF 方向。
- **编辑与转换**：旋转、翻转、裁剪、调整分辨率，并另存或转换为常见图片格式；JPEG 支持调整质量。
- **识别图片文字**：在本机识别简体中文和英文，可识别整图或选定区域，并复制识别结果。首次使用时会按需下载 OCR 语言数据。
- **管理文件**：重命名、删除图片或复制文件路径。
- **系统集成**：设置图片文件关联；Windows 支持为不同格式显示专属图标。可在设置中检查并安装签名校验后的更新。
- **深浅色主题**：跟随系统外观，也可在应用设置中切换。

> 当前不支持 AVIF 和 HEIC。各平台的文件关联能力有所不同；Windows 设为默认应用时仍需在系统设置中确认。

### 快捷键

| 按键 | 功能 |
| --- | --- |
| `Ctrl+O` | 打开图片 |
| `←` / `→` | 上一张 / 下一张 |
| `+` / `-` | 放大 / 缩小 |
| `F` / `0` | 适应窗口 |
| `1` | 原始大小 |
| `R` / `L` | 向右 / 向左旋转预览 |
| `C` | 开始 / 结束裁剪 |
| `O` | 识别图片文字 |
| `T` | 显示 / 隐藏缩略图 |
| `I` | 显示 / 隐藏图片信息 |
| `F11` / 双击图片 | 全屏 |
| `Esc` | 退出当前操作或全屏 |

### 开发

需要 Rust、Node.js 和 pnpm。安装依赖并启动开发版：

```bash
pnpm install
pnpm tauri dev
```

构建应用：

```bash
pnpm tauri build
```

详细的开发接口与实现说明见 [`docs/INTERFACES.md`](docs/INTERFACES.md)。
