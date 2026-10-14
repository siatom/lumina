# Lumina — IPC 接口规范

> 版本：阶段 2C-B（EXIF orientation 自动应用修复）
> 状态：已实现并通过 `cargo test`（含真实固件格式矩阵 + orientation 断言）
> 前端类型定义见 `src/api.ts`，Rust 实现见 `src-tauri/src/commands.rs`

本文档是前端（Prism）与格式/操作模块（Pixel）共同依赖的稳定契约。修改任何命令签名、类型字段或协议格式前，请先更新本文件与 `src/api.ts`。

---

## 1. 架构总览

```
┌─────────────────────────── WebView (前端) ───────────────────────────┐
│  UI (src/main.ts)                                                     │
│    │  调用类型化封装 (src/api.ts)                                      │
│    ├── invoke("...") ─────────────►  Tauri IPC (JSON)                 │
│    └── <img src="lumina://..."> ──►  自定义协议 (二进制流)              │
└───────────────────────────────────────────────────────────────────────┘
                                   │
┌─────────────────────────── Rust 后端 ────────────────────────────────┐
│  commands.rs  ← IPC 命令层（参数校验 / 错误 -> String）                 │
│        │                                                              │
│        ├── imaging.rs   核心管线：解码 / 元信息 / EXIF / 缩略图 / 转换   │
│        └── protocol.rs  lumina:// 协议：流式返回图片字节                 │
└───────────────────────────────────────────────────────────────────────┘
```

分层原则：

- **`imaging.rs` 不依赖任何 Tauri 类型**，可独立单测、可被 Pixel 的格式模块直接复用或扩展。
- **`commands.rs` 只做参数适配**，业务逻辑一律下沉到 `imaging.rs`。
- 前端所有 IPC 调用必须经过 `src/api.ts`，不要在组件里直接 `invoke`。

除「请求 / 响应」外还有一条**反向通道**：后端 `watcher.rs` 监听当前浏览目录，文件增删改时 emit `directory-changed` 事件，前端 `listen` 后重新列举目录（见 §4.12）。

---

## 2. 图片数据传输方案取舍（重点：大图性能）

把一个 40 MB 的 JPEG 送到 `<img>`，有三种做法：

| 方案 | 大图性能 | 内存开销 | 服务端可处理（缩放/转码/缓存） | 结论 |
| --- | --- | --- | --- | --- |
| **base64 over IPC** | 差 | 极差 | 可以 | ❌ 二进制膨胀约 **+33%**，40 MB → 53 MB 字符串，经历 JSON 序列化/反序列化，主线程卡顿 |
| **asset protocol** (`convertFileSrc`) | 好 | 好 | ❌ 只能原样返回文件 | ⚠️ 零拷贝流式，但无法在传输时降采样，TIFF 等 WebView 不支持的格式无法显示，且需要 `assetProtocol` 作用域配置与额外权限 |
| **自定义 protocol**（本项目采用） | 好 | 好 | ✅ 可解码、降采样、转码、缓存 | ✅ WebView 直接把响应体喂给图片解码器，不经 JSON、不做 base64；缩略图在 Rust 侧降采样后再传 |

### 本项目的具体决策

1. **原图浏览** → `lumina://...&mode=full`
   - WebView 原生支持的格式（png/jpg/jpeg/gif/webp/bmp/ico）**直接返回原始文件字节**：不解码、不重编码，零质量损失，最快。
   - WebView 不支持的格式（当前是 **TIFF**）→ Rust 解码后**转码成 PNG** 再返回。
2. **缩略图** → `lumina://...&mode=thumb&max=128&v=<rev>`
   - 在 Rust 侧 `thumbnail()` 降采样后再编码为 PNG，避免把原始大图传给前端。
   - 结果按 `(path, v, max)` 进**进程内 LRU 缓存**（`src-tauri/src/cache.rs`，512 条），重复滚动/回看同一目录不再重复解码。
   - 前端用 `IntersectionObserver` 懒加载，只对进入视口的缩略图发起请求，并以**最多 6 个并发**的池加载。
   - `v` 是 `<modified_ms>-<file_size>` 版本令牌：既是缓存键，也让 URL 随文件修订变化（协议对带 `v` 的请求回 `Cache-Control: immutable`）。
3. **兜底通道** → `get_image_bytes` 命令（`tauri::ipc::Response`，原始字节）
   - 前端在 `<img>` 触发 `error` 时（协议不可用等）自动回退，保证在任何环境下都能显示。
   - 注意：这里返回的是**原始字节**而非 base64，仍然比 base64 方案省 33% 带宽与内存。

> 后续优化方向（阶段 2+）：给 `mode=full` 的原始字节也加 LRU 缓存、按需 `max` 的渐进式降采样（`image` 的 `resize` + 解码器 hint）。（缩略图 LRU 缓存已在性能优化中落地；EXIF 方向自动应用已在阶段 2C-B 完成，见第 9 节。）

---

## 3. `lumina://` 自定义协议

平台相关的基础地址由 `get_protocol_base` 返回，前端**不要硬编码**：

| 平台 | Base |
| --- | --- |
| Windows | `http://lumina.localhost` |
| macOS / Linux | `lumina://localhost` |

### 请求格式

```
<base>/?path=<percent-encoded>&mode=full|thumb&max=<number>
```

| 参数 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `path` | string | ✅ | 图片绝对路径（`encodeURIComponent` / `URLSearchParams` 编码） |
| `mode` | `"full" \| "thumb"` | ❌ | 默认 `full` |
| `max` | number | ❌ | `thumb` 模式下最长边的像素上限，默认 `256`，取值范围 `[16, 4096]` |

### 响应

- 成功：`200`，`Content-Type` 为 `image/png` / `image/jpeg` / `image/webp` / `image/gif` / `image/bmp` / `image/x-icon` / `image/svg+xml`，body 为图片字节。
- 失败：`400`，`Content-Type: text/plain`，body 为错误说明。

> 阶段 2B：**SVG 加入 webview 原生可渲染集合**（Chromium/WebView2 直接渲染矢量），`mode=full` 时返回原始 SVG 字节 + `image/svg+xml`，缩略图与元信息则由 resvg 在 Rust 侧栅格化。

### 前端用法

```ts
import { luminaImageUrl } from "./api";

const url = await luminaImageUrl("C:\\pics\\a.jpg", { mode: "full" });
img.src = url;

const thumbUrl = await luminaImageUrl("C:\\pics\\a.jpg", { mode: "thumb", max: 128 });
```

---

## 4. IPC 命令

所有命令失败时返回 `Err(String)`，在 JS 中表现为 **rejected promise**，`catch` 到的即错误信息（人类可读）。参数名在 JS 侧使用 **camelCase**，Tauri 会自动映射到 Rust 的 snake_case。

### 4.1 `get_image_metadata`

读取单张图片的元信息（**仅解析文件头，不做完整解码**，大图也很快）。

```ts
function getImageMetadata(path: string): Promise<ImageMetadata>;
```

```ts
interface ImageMetadata {
  path: string;          // 绝对路径（原样回传）
  file_name: string;     // 文件名（含扩展名）
  format: string;        // 规范化格式名："png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "tiff" | "ico" | "svg"
  width: number;         // 像素宽
  height: number;        // 像素高
  file_size: number;     // 文件字节数
  has_exif: boolean;     // 是否存在可解析的 EXIF 块
  exif: ExifData | null; // 无 EXIF 时为 null
}

interface ExifData {
  orientation: number | null;   // EXIF 方向 1..8
  make: string | null;          // 厂商
  model: string | null;         // 机型
  lens_model: string | null;    // 镜头型号（阶段 2B 新增）
  datetime: string | null;      // DateTimeOriginal，缺失时回退 DateTime
  software: string | null;      // 处理软件
  artist: string | null;        // 作者（阶段 2B 新增）
  copyright: string | null;     // 版权（阶段 2B 新增）
  description: string | null;   // ImageDescription（阶段 2B 新增）
  exposure_time: string | null; // 曝光时间，如 "1/250"
  f_number: string | null;      // 光圈，如 "f/2.8"
  iso: string | null;           // ISO 感光度
  focal_length: string | null;  // 焦距，如 "35 mm"
  gps_latitude: number | null;  // 十进制度，南纬为负（阶段 2B 新增）
  gps_longitude: number | null; // 十进制度，西经为负（阶段 2B 新增）
  gps_altitude: number | null;  // 海拔米（阶段 2B 新增）
}
```

> 阶段 2B：**字符串型 EXIF 值不再带引号**（修复了阶段 1 中 `Make` 等被 `display_value()` 包上 `"..."` 的问题）；新增镜头、作者、版权、描述与 **GPS 十进制度**（由 DMS + 半球引用换算，S/W 自动取负）。
> **SVG 元信息**：`width`/`height` 由 `resvg` 按 96 DPI 解析 `width`/`height`/`viewBox` 得到，`exif` 恒为 `null`。

### 4.2 `list_images`

列出某目录下的同级图片（按文件名**自然排序**，`img2` 排在 `img10` 前），并给出当前文件在列表中的下标，用于上一张/下一张与相册。

```ts
function listImages(dir: string, current?: string): Promise<DirectoryListing>;
```

```ts
interface DirectoryListing {
  dir: string;
  entries: ImageEntry[];
  index: number | null; // current 在 entries 中的下标；未传或不在目录内时为 null
}

interface ImageEntry {
  path: string;
  file_name: string;
  format: string;    // 扩展名（小写），如 "jpg"
  file_size: number;
}
```

> 当前为**非递归**列出单层目录。相册/递归扫描、虚拟滚动在阶段 2 由 Prism 与 Pixel 共同细化。

### 4.3 `get_thumbnail`

生成等比缩放的 PNG 缩略图，最长边不超过 `maxSize`。

```ts
function getThumbnail(path: string, maxSize?: number): Promise<Blob>; // 默认 256，范围 [16, 4096]
```

Rust 侧返回 `tauri::ipc::Response::new(bytes)`，前端收到 `ArrayBuffer`，`api.ts` 已封装为 `Blob`。

### 4.4 `getImageBlob`（兜底，Rust 命令 `get_image_bytes`）

前端封装的导出名是 **`getImageBlob`**（返回 `Blob`，不是裸 `ArrayBuffer`）——文档早期误写为 `getImageBytes`。

返回原图字节（与 `lumina://` 的 `mode=full` 语义一致），供协议不可用时回退。

```ts
function getImageBlob(path: string): Promise<Blob>;
```

Blob 的 `type` 由**路径扩展名**推导（`api.ts` 的 `imageMimeType`），与 `imaging::mime_for_ext` 及 `WEBVIEW_NATIVE_EXTENSIONS` 保持一致：

- webview 原生格式（`png` / `jpg` / `jpeg` / `gif` / `webp` / `bmp` / `ico` / `svg`）→ 原始字节直传，Blob 标记为对应的 MIME（如 `image/jpeg`、`image/svg+xml`）。
- 其余格式（当前为 `tif` / `tiff`，以及未知扩展名）→ Rust 已转码为 PNG，Blob 标记为 `image/png`。

> SVG 必须标记为 `image/svg+xml` 才能在 `<img>` 中渲染；早期版本固定写死 `image/png` 属缺陷，已在阶段 4 修复。

### 4.5 `save_image_as`（转换/另存）

```ts
function saveImageAs(
  inputPath: string,
  outputPath: string,
  format?: string, // "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "tiff" | "ico"；省略则按 outputPath 扩展名推断
  quality?: number, // 1..=100，仅 JPEG 生效（阶段 2B 新增）；默认 85
): Promise<string>; // 返回写入的 outputPath
```

> 阶段 2B：新增可选 `quality`（**向后兼容**，旧的三参调用不受影响）。解码 → 按目标格式重编码，不做几何变换；几何操作请用 [`transform_image`](#46-transform_image裁剪--翻转--旋转--缩放)。
> 注意：`image` crate 的 **WebP 编码器是无损**的，`quality` 对 WebP/PNG 无效（已在文档标注，必要时后续接入 libwebp 做有损 WebP）。

### 4.6 `transform_image`（裁剪 / 翻转 / 旋转 / 缩放）

阶段 2B 新增。声明式地描述一次编辑，Rust 端一步执行「解码 → 编辑 → 编码 → 写文件」。

```ts
function transformImage(
  inputPath: string,
  outputPath: string,
  options: TransformOptions,
): Promise<string>; // 返回写入的 outputPath
```

```ts
interface TransformOptions {
  rotate?: 0 | 90 | 180 | 270;    // 顺时针
  flipHorizontal?: boolean;
  flipVertical?: boolean;
  crop?: { x: number; y: number; width: number; height: number }; // 源图坐标
  resize?: {
    width?: number;
    height?: number;
    exact?: boolean;              // true=精确 w×h（可能变形）；默认等比适配
    filter?: "nearest" | "triangle" | "catmullrom" | "gaussian" | "lanczos3"; // 默认 lanczos3
  };
  format?: string;                // 目标格式；省略按 outputPath 扩展名推断
  quality?: number;               // 1..=100，仅 JPEG
}
```

**操作顺序固定为 `crop → flip → rotate → resize → encode`**（前端预览时请按同一顺序叠加，否则结果会不一致）。

参数校验（失败即 `Err`，不会写出半成品文件）：

- `rotate` 只接受 0/90/180/270；其它值报错。
- `crop.width` / `crop.height` 必须 > 0；`crop.x`/`crop.y` 越界报错，宽高超出部分**自动裁剪到图像边界**。
- `resize` 至少给出 `width` 或 `height` 之一。
- 目标格式为 avif/heic 时报「已评估并推迟」的明确错误（见第 8 节）。

### 4.7 `get_image_pages`

阶段 2B 新增。返回文件的页数 / 帧数：GIF 与动图 WebP 返回帧数，多页 TIFF 返回页数，单帧格式返回 `1`。

```ts
function getImagePages(path: string): Promise<number>;
```

> 实现会**完整解码所有帧**来计数，超大动图较耗时；建议按需调用并对结果做缓存。

### 4.8 `get_image_frame`

阶段 2B 新增。取出第 `index` 帧（从 0 开始）并转成 PNG 返回，供分页浏览。

```ts
function getImageFrame(path: string, index: number): Promise<Blob>;
```

- GIF / 动图 WebP：按帧序号取帧。
- 多页 TIFF：按目录序号取页（支持 8/16 位 RGB / RGBA / Gray / GrayA）。
- 单帧格式：仅 `index = 0` 合法。
- 越界返回 `Err("frame index N out of range ...")`。

> **动画格式播放说明**：GIF 与动图 WebP 属于 webview 原生可渲染集合，`<img src="lumina://...&mode=full">` **会直接播放动画**（无需本命令）。本命令用于「暂停/逐帧/显示第 N 帧」等场景。

### 4.9 `rename_file`

阶段 2B 新增。原地重命名，返回新的绝对路径。

```ts
function renameFile(path: string, newName: string): Promise<string>;
```

安全约束（违反即 `Err`）：`newName` 必须是**纯文件名**（不含 `/` `\`，不能是 `.` / `..`，不能为空）；源必须是文件；目标已存在时拒绝覆盖。

### 4.10 `delete_file`

阶段 2B 新增。删除单个文件。

```ts
function deleteFile(path: string): Promise<void>;
```

安全约束：后端用 `symlink_metadata` 判断，**只允许删除普通文件**，目录与符号链接一律拒绝（避免误删整目录）。**删除不可恢复——UI 必须先弹确认框**。

### 4.11 `get_protocol_base`

```ts
function getProtocolBase(): Promise<string>; // "http://lumina.localhost" 或 "lumina://localhost"
```

### 4.12 `watch_directory` / `unwatch_directory`（目录监听）

让缩略图侧栏跟着磁盘走：目录里的图片被外部增删改时，前端无需用户操作即可刷新。

```ts
function watchDirectory(dir: string): Promise<void>;   // 监听（替换上一个）
function unwatchDirectory(): Promise<void>;            // 停止监听

/** 订阅目录变化；返回取消订阅函数。 */
function onDirectoryChanged(handler: (dir: string) => void): Promise<UnlistenFn>;
```

**契约要点**

- **只监听一个目录**，非递归。前端在每次打开文件后调用 `watch_directory`；**重复监听当前目录是 no-op**，因此可以放心地每次都调。
- **事件只报「哪个目录变了」，不报「什么变了」**：payload 为 `{ dir: string }`。前端收到后**重新列举目录并整体替换**，不做增量合并——这样漏事件、乱序事件都不会在界面上留下幽灵条目。
- **事件已防抖**：280ms 静默窗口内的连续变化合并为一次通知（批量导出几十张图只刷新一次）。无关流量（非图片文件、`.DS_Store`）不会延长静默窗口，因此同目录里持续追加的日志文件不会把刷新无限推迟。
- **过滤规则**：只认「创建 / 删除 / 写入 / 重命名」，且路径带受支持图片扩展名（大小写不敏感）或**就是被监听的目录本身**（目录被改名 / 删除时也要通知前端）。`Access`（读取）事件一律忽略，否则会自激。
- 监听通过各平台原生 API 实现（macOS FSEvents / Linux inotify / Windows `ReadDirectoryChangesW`），无 C 工具链依赖。

**前端行为约定**（`src/main.ts`）

| 磁盘上的变化 | 界面反应 |
| --- | --- |
| 新增图片 | 缩略图侧栏出现新条目，计数更新 |
| 删除图片（非当前） | 对应条目消失，导航索引重算 |
| 删除**正在浏览**的图片 | 自动接替原位置的下一张（末尾则退到上一张）并 toast 提示；全部删空则回到空状态 |
| 修改**正在浏览**的图片 | 按新的 `modified_ms` / `file_size` 重新解码显示 |
| 浏览中的目录被改名 / 删除 | 回到空状态并提示，同时停止监听 |

> 修改检测依赖 `modified_ms` 与 `file_size` 两个字段（见 §4.1），后端 `ImageEntry` 必须持续提供它们。

---

## 5. 文件对话框

由 `@tauri-apps/plugin-dialog` 提供，已在 `src/api.ts` 封装：

```ts
function pickImage(): Promise<string | null>;                              // 单张图片选择
function pickSavePath(defaultPath: string, extension: string): Promise<string | null>;

// 阶段 2B 新增：把路径写入系统剪贴板（"复制路径"）
function copyPathToClipboard(path: string): Promise<void>;
```

支持的扩展名常量（与 Rust `SUPPORTED_EXTENSIONS` 保持一致）：

```ts
export const SUPPORTED_EXTENSIONS = [
  "png", "jpg", "jpeg", "gif", "webp", "bmp", "tif", "tiff", "ico", "svg",
] as const;

// 可作为 transform/save-as 输出格式的集合（与 Rust 写出能力一致）
export const ENCODABLE_EXTENSIONS = [
  "png", "jpg", "jpeg", "gif", "webp", "bmp", "tif", "tiff", "ico",
] as const;
```

对应的 Tauri 权限已在 `src-tauri/capabilities/default.json` 授予：`dialog:default`。

---

## 6. 格式支持矩阵（阶段 2B 实测）

固件位于 `src-tauri/tests/fixtures/`，由 `scripts/make-fixtures.cmd`（ImageMagick）与 `scripts/make-exif.py`（Pillow）生成。一键复现：

```bash
cd src-tauri && cargo test --lib -- --nocapture    # 打印「格式 × 结果矩阵」
```

### A. 基础格式（真实固件解码通过）

| 格式 | 固件 | 元信息 | 原图显示 | 缩略图 | 结论 |
| --- | --- | --- | --- | --- | --- |
| PNG | grad.png | ✅ 96×64 | ✅ 原字节 | ✅ | 支持 |
| JPEG | grad.jpg | ✅ 96×64 | ✅ 原字节 | ✅ | 支持 |
| GIF | anim.gif | ✅ | ✅ 原字节（**动图原生播放**） | ✅ | 支持 |
| BMP | grad.bmp | ✅ 96×64 | ✅ 原字节 | ✅ | 支持 |
| WebP | grad.webp | ✅ 96×64 | ✅ 原字节 | ✅ | 支持 |
| TIFF | grad.tiff | ✅ | ✅ 转码 PNG | ✅ | 支持（webview 不支持，转 PNG） |
| ICO | grad.ico | ✅ 96×96（多分辨率取最大） | ✅ 原字节 | ✅ | 支持 |

### B. 阶段 2B 新增

| 格式 | 实现方式 | 结果 |
| --- | --- | --- |
| **SVG** | `resvg 0.45`（`usvg` 解析 + `tiny-skia` 栅格化，纯 Rust） | ✅ 元信息按 96 DPI 解析；缩略图直接按目标尺寸栅格化；`mode=full` 由 webview 原生渲染原始 SVG |
| **GIF / WebP 动图** | webview 原生播放 + `AnimationDecoder` 取帧 | ✅ 帧数 3（anim.gif / anim.webp），逐帧 API 可用 |
| **多页 TIFF** | `tiff 0.11`（`image` 只暴露首页） | ✅ 页数 3（multipage.tiff），按页取图 |

### C. 评估后推迟（已实测）

| 格式 | 评估结论 |
| --- | --- |
| **AVIF** | ❌ 推迟。`image 0.25` 的 AVIF **解码**需要 `avif-native`（原生 **dav1d**），Windows 上需 meson/nasm 工具链；纯 Rust 只有 ravif **编码**。为守住「轻量」原则不引入原生依赖。固件 `grad.avif` 实测返回清晰错误：`The image format Avif is not supported`。 |
| **HEIC / HEIF** | ❌ 推迟（同因）。无纯 Rust 解码器；`libheif-rs` 需原生 libheif。本机 ImageMagick 对 HEIC 是**只读**（`r--`），无法生成测试固件。 |
| 调色板 / 双色 TIFF | ⚠️ 已知限制：`tiff 0.11` 在**像素解码**层拒绝 `RGBPalette` / `TransparencyMask`。页数统计与导航仍可用（`palette.tiff` 实测 `pages=3`，取帧报清晰错误）。 |

> 后续若必须支持 AVIF/HEIC：建议以**可选 Cargo feature** 引入（`avif-native` / `libheif`），默认关闭，避免污染主构建。

### D. 维护者备注：管线入口与「新增格式」清单

`src-tauri/src/imaging.rs`（**不依赖 Tauri**，可单测）现有关键入口：

- `decode(path) -> DynamicImage`（SVG 会先栅格化）
- `encode_image(&DynamicImage, ImageFormat, Option<u8>) -> Vec<u8>`（JPEG 走 quality）
- `encode_png` / `make_thumbnail` / `read_for_display` / `convert_image`
- `apply_transform(DynamicImage, &TransformSpec) -> DynamicImage` / `transform_image(input, output, &TransformSpec)`
- `read_metadata` / `list_directory` / `page_count` / `read_frame_png`
- `rename_file` / `delete_file` / `parse_format` / `mime_for_ext`

新增一个格式的最小改动集：

1. `Cargo.toml`（如需）→ `image` 特性或独立解码 crate；
2. `imaging.rs` 的 `SUPPORTED_EXTENSIONS` / `WEBVIEW_NATIVE_EXTENSIONS` / `mime_for_ext` / `parse_format`；
3. 若需要特殊解码路径（如 SVG 的栅格化、TIFF 的多页），加分支；
4. `src-tauri/tests/fixtures/` 加真实固件 + `format_matrix.rs` 矩阵用例；
5. 同步本文档与 `src/api.ts`（`SUPPORTED_EXTENSIONS` / `ENCODABLE_EXTENSIONS`）。

错误统一用 `src-tauri/src/error.rs::AppError`（`Io` / `Image` / `Unsupported` / `Other`），命令边界转 `String`。

---

## 7. 给 Prism 的对接摘要（前端）

- **只用 `src/api.ts`**，不要直接 `invoke`。类型、错误处理、`lumina://` URL 构造都已封装。
- **显示原图**：`img.src = await luminaImageUrl(path, { mode: "full" })`；务必监听 `error` 事件并回退到 `getImageBlob(path)`（`src/main.ts` 已示范）。**SVG 与动图 GIF/WebP 都可直接这样显示**。
- **缩略图墙**：用 `IntersectionObserver` 懒加载，直接 `img.src = await luminaImageUrl(path, { mode: "thumb", max: 128, version })`（`version` = `<modified_ms>-<file_size>`，来自 `listImages` 的条目）。协议不可用时再回退到 `getThumbnail(path, 128)` + `URL.createObjectURL`（记得在 `load`/`error` 后 `URL.revokeObjectURL`）。建议用并发池限制同时在飞的请求数（`src/main.ts` 用 6）。
- **上一张/下一张**：一次 `listImages` 拿到全目录 + 当前下标，本地计算索引即可，不要每张都重新列目录。
- **编辑（阶段 2B）**：旋转/翻转可先用 CSS `transform` 做**即时预览**，点「保存」时再用 `transformImage(path, out, { rotate, flipHorizontal, ... })` 落到文件；裁剪/缩放请在 UI 里换算成**源图坐标**后传 `crop` / `resize`（顺序见 4.6）。
- **列表信息面板**：`getImageMetadata` 现在带镜头、作者、版权、GPS 十进制度，可直接展示。
- **文件操作**：重命名用 `renameFile`；删除用 `deleteFile`，**务必先确认**；复制路径用 `copyPathToClipboard`。

---

## 8. 已知限制 / 待办

1. 自定义协议对 **`mode=thumb` 有进程内 LRU 缓存**（`cache.rs`）；`mode=full` 的原始字节仍**未缓存**，同一张图重复请求会重新读盘（`fs::read`，受 OS page cache 加速）。
2. ~~EXIF 方向未自动应用~~ → **阶段 2C-B 已修复**，见第 9 节（缩略图/转码路径/转换编辑统一按方向旋转，webview 原生格式走原字节直通避免二次旋转）。
3. `list_images` 非递归、不分页，超大目录（数万文件）需要虚拟化。
4. **AVIF / HEIC 解码未支持**，见第 6 节 C；SVG 已支持。
5. **有损 WebP 编码未支持**（`image` crate 仅无损）。
6. `get_image_pages` 通过完整解码计数，大动图较慢；无缓存。前端已按扩展名（gif/webp/tif/tiff）门控，且**不阻塞主图显示**（fire-and-forget）。
7. 多页/调色板 TIFF 见第 6 节 C（像素解码不支持 palette/bilevel）。
8. `transform_image` 每次都是「整图解码 → 编辑 → 编码」，大图会一次性占用较多内存；尚未做流式/分块处理。
9. 尚未验证的运行时项：`lumina://` 在 Windows WebView2 下的实际加载、`dialog` 权限在生产构建下的行为（阶段 1 已通过编译/单测/前端构建，GUI 冒烟见 README）。
10. 目录监听（§4.12）**非递归**：只看当前浏览目录，子目录里的变化不会触发刷新。事件已防抖，但极端高频写入（如边下载边写入大图）仍可能在写入完成前就触发一次刷新，此时读到的是中间态。
11. 目录监听依赖平台原生 API，**网络盘 / 某些虚拟文件系统**可能不上报事件；此时界面退回「手动重新打开目录」的行为。

---

## 9. EXIF orientation 自动应用（阶段 2C-B）

### 9.1 问题

照片（尤其手机竖拍）常把**传感器原始方向**的像素存盘，另在 EXIF `Orientation`（tag 274）里记录「显示时应如何旋转」。若不处理，会出现：缩略图是横的、点开大图却是竖的（或反之）。

### 9.2 关键约束：WebView2 **会**自动应用 orientation

Chromium/Blink 的 `<img>` 默认按 CSS `image-orientation: from-image` 渲染，即**读取 JPEG 的 EXIF Orientation 并自动旋转**，同时 `naturalWidth/naturalHeight` 也随之对调。因此：

- 主视图（`<img src="lumina://...&mode=full">`）对 webview 原生格式**已经是正确方向**。
- 若 Rust 侧再旋转一次（或在流式直通前重编码旋转），就会**二次旋转**。

**实测取证**（headless Edge / WebView2，Blink `155.0.4283.45`，脚本 `scripts/probe-webview2-orientation.cmd` + `scripts/webview2-probe.html`，固件 `orient{1,6,8}.jpg` 存储 96×64、红色标记在存储左上角）：

```
FIXTURE orient1.jpg natural=96x64 stored=96x64 axes-swapped=no  red=tl
FIXTURE orient6.jpg natural=64x96 stored=96x64 axes-swapped=yes red=tr   # 引擎顺时针转 90°
FIXTURE orient8.jpg natural=64x96 stored=96x64 axes-swapped=yes red=bl   # 引擎顺时针转 270°
```

结论：**主视图方向由引擎负责；Rust 侧只需保证「非直通」输出与之一致。**

### 9.3 统一策略（`src-tauri/src/imaging.rs`）

| 路径 | 处理 | 原因 |
| --- | --- | --- |
| **webview 原生格式**（`WEBVIEW_NATIVE_EXTENSIONS`，含 JPEG/WebP/PNG/GIF/BMP/ICO/SVG）`read_for_display` | **原字节直通，不改** | 引擎会自行旋转；改了就二次旋转 |
| **缩略图** `make_thumbnail` | `decode` 原始像素 → `thumbnail()` 降采样 → **再** `apply_orientation` | 缩略图是 Rust 重编码的 PNG，无 EXIF，必须自己烘进方向。降采样后再转 90° 的整数倍**无损**（无重采样），且旋转小图更快 |
| **转码路径** `read_for_display`（非原生，当前 TIFF→PNG） | `decode` → `apply_orientation` → `encode_png` | 输出的 PNG 无 EXIF，引擎无从旋转，必须烘进像素 |
| **转换 / 编辑** `convert_image` / `transform_image` | 经 `decode_oriented()` 解码（先烘方向）再操作 | 后续 crop/flip/rotate/resize 都在**已定向**坐标系里进行，前后端预览一致；输出不带陈旧 orientation 标签 |
| **多页/取帧** `read_frame_png` / `tiff_frame` | 取帧后 `apply_orientation`（TIFF 按每页 `Tag::Orientation` 单独读） | 每帧/每页可能各有方向 |

新增内部函数：`exif_orientation(path) -> Option<Orientation>`、`apply_exif_orientation(&mut DynamicImage, path)`、`decode_oriented(path) -> Result<DynamicImage>`。方向应用走 `image::metadata::Orientation` + `DynamicImage::apply_orientation()`（`image` 的解码器**不会**自动应用，需显式调用）。

### 9.4 固件与断言

- `scripts/make-exif.py` 生成 `orient{1,6,8}.jpg`（存储 96×64，存储左上角为红色块）+ `orient6.tiff` / `orient8.tiff`（TIFF baseline tag 274）。
- `src-tauri/src/format_matrix.rs` 新增 5 个断言用例（`cargo test` 全绿）：
  - `exif_orientation_thumbnail_matches_webview`：缩略图 48×32/32×48，红角 TL/TR/BL 与 9.2 的探针一致。
  - `exif_orientation_transcode_applies_before_encode`：`orient6.tiff`/`orient8.tiff` → PNG 64×96，红角 TR/BL。
  - `webview_native_passthrough_keeps_raw_bytes`：JPEG `mode=full` 返回字节与文件**逐字节相等**（证明未重编码）。
  - `convert_and_transform_use_oriented_pixels`：`convert_image(orient6.jpg→png)` = 64×96 红角 TR；`transform_image` 裁剪已定向坐标命中标记。
  - `exif_orientation_is_reported_for_every_container`：JPEG **与** TIFF 都能读到 `orientation`（前端 9.6 的显示尺寸推导依赖它）。

复现：

```bash
python scripts/make-exif.py                          # 造固件
src-tauri> cargo test orientation -- --nocapture      # 打印方向断言表
src-tauri> cargo test --all-targets                   # 全量（23 项）
```

### 9.5 给 Prism 的注意点

- 主视图继续用 `luminaImageUrl(path, { mode: "full" })` 即可，**方向由引擎处理**，前端无需额外 CSS 旋转。
- **不要**在前端对原生格式再叠加 `transform: rotate(...)`——会与引擎的自动旋转叠加。
- 缩略图（`getThumbnail` / `mode=thumb`）与主视图现在方向**一致**，可放心并排显示。
- 若前端要把图绘到 `<canvas>`（`drawImage`）再处理：canvas 按**原始像素**绘制，**不含** EXIF 自动旋转；需要方向一致时请改用 `getImageFrame`/`transformImage` 的输出（那些是已定向像素）。

### 9.6 前端显示坐标系（坐标偏移修复）

`read_metadata` 返回的 `width/height` 是**存储尺寸**（`into_dimensions()` 只读文件头），而引擎渲染 orientation 5–8 的图时会**对调宽高**。前端若直接用存储尺寸做 fit / 缩放 / 裁剪，竖拍照片就会出现「方向正确但坐标错位」——fit 比例不对、裁剪框偏移、裁剪结果与所见不符。

修法（`src/main.ts`）：

- 新增 `orientedDims(m)`：orientation ∈ {5,6,7,8} 时对调宽高，得到**显示尺寸**。
- `openPath` 中 `meta.width/height` 与 `fileDims` 一律改用显示尺寸；信息面板「尺寸」也显示显示尺寸（EXIF「方向」仍单列一行）。
- 于是 `viewDims` / `fitScale` / `centerImage` / `origin` / `imageToStage` / `stageToImage` / `validateCrop` / `cropRectFromStage` 全部落在**显示像素坐标系**，与后端 `decode_oriented`（`transform_image` 的裁剪坐标系）一致 —— 裁剪坐标因此前后端对齐。
- **安全网**：`viewer` 的 `load` 事件里用 `naturalWidth/naturalHeight` 校正 `meta` 并 `reflow()`，覆盖「EXIF 缺失但引擎仍旋转」或「后端已把方向烘进像素」（TIFF 转码、取帧）的情况；两者一致时是 no-op，无跳变。

> 注意：`getImageMetadata` 的 `width/height` 语义仍是**存储尺寸**（后端契约不变），显示尺寸的推导由前端按 `exif.orientation` 完成。
