# Lumina — IPC 接口规范

> 版本：阶段 1（脚手架 + 核心图像管线）
> 状态：已实现并通过 `cargo test` / `pnpm build`
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
2. **缩略图** → `lumina://...&mode=thumb&max=128`
   - 在 Rust 侧 `thumbnail()` 降采样后再编码为 PNG，避免把原始大图传给前端。
   - 前端用 `IntersectionObserver` 懒加载，只对进入视口的缩略图发起请求。
3. **兜底通道** → `get_image_bytes` 命令（`tauri::ipc::Response`，原始字节）
   - 前端在 `<img>` 触发 `error` 时（协议不可用等）自动回退，保证在任何环境下都能显示。
   - 注意：这里返回的是**原始字节**而非 base64，仍然比 base64 方案省 33% 带宽与内存。

> 后续优化方向（阶段 2+）：给 `lumina://` 增加 LRU 解码缓存、按需 `max` 的渐进式降采样（`image` 的 `resize` + 解码器 hint）、以及 EXIF 方向自动应用。

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

- 成功：`200`，`Content-Type` 为 `image/png` / `image/jpeg` / `image/webp` / `image/gif` / `image/bmp` / `image/x-icon`，body 为图片字节。
- 失败：`400`，`Content-Type: text/plain`，body 为错误说明。

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
  format: string;        // 规范化格式名："png" | "jpeg" | "gif" | "webp" | "bmp" | "tiff" | "ico"
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
  datetime: string | null;      // DateTimeOriginal，缺失时回退 DateTime
  software: string | null;      // 处理软件
  exposure_time: string | null; // 曝光时间，如 "1/250"
  f_number: string | null;      // 光圈，如 "f/2.8"
  iso: string | null;           // ISO 感光度
  focal_length: string | null;  // 焦距，如 "35 mm"
}
```

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

### 4.4 `get_image_bytes`（兜底）

返回原图字节（与 `lumina://` 的 `mode=full` 语义一致），供协议不可用时回退。

```ts
function getImageBytes(path: string): Promise<ArrayBuffer>;
```

### 4.5 `save_image_as`（转换/另存）

```ts
function saveImageAs(
  inputPath: string,
  outputPath: string,
  format?: string, // "png" | "jpeg" | "gif" | "webp" | "bmp" | "tiff" | "ico"；省略则按 outputPath 扩展名推断
): Promise<string>; // 返回写入的 outputPath
```

> 阶段 1 实现：解码 → 按目标格式重编码。**尚未支持质量参数（JPEG quality）、色彩空间转换、批量转换**——这些属于 Pixel 的阶段 2 范围，接口预留 `format` 字段，后续可扩展为 `options` 对象（保持向后兼容）。

### 4.6 `get_protocol_base`

```ts
function getProtocolBase(): Promise<string>; // "http://lumina.localhost" 或 "lumina://localhost"
```

---

## 5. 文件对话框

由 `@tauri-apps/plugin-dialog` 提供，已在 `src/api.ts` 封装：

```ts
function pickImage(): Promise<string | null>;                              // 单张图片选择
function pickSavePath(defaultPath: string, extension: string): Promise<string | null>;
```

支持的扩展名常量（与 Rust `SUPPORTED_EXTENSIONS` 保持一致）：

```ts
export const SUPPORTED_EXTENSIONS = [
  "png", "jpg", "jpeg", "gif", "webp", "bmp", "tif", "tiff", "ico",
] as const;
```

对应的 Tauri 权限已在 `src-tauri/capabilities/default.json` 授予：`dialog:default`。

---

## 6. 给 Pixel 的对接摘要（格式 / 操作模块）

- 解码/编码入口集中在 `src-tauri/src/imaging.rs`，**不依赖 Tauri**，可直接调用或扩展：
  - `decode(path) -> DynamicImage`
  - `encode_png(&DynamicImage) -> Vec<u8>`
  - `make_thumbnail(path, max_size) -> Vec<u8>`
  - `read_for_display(path) -> (Vec<u8>, mime)`
  - `convert_image(input, output, Option<ImageFormat>)`
  - `read_metadata(path) -> ImageMetadata`
  - `list_directory(dir, current) -> DirectoryListing`
- 已启用 `image` crate 特性：`png, jpeg, gif, webp, bmp, tiff, ico`。新增格式（avif、heic 等）时：在 `Cargo.toml` 加特性 → 更新 `SUPPORTED_EXTENSIONS` / `WEBVIEW_NATIVE_EXTENSIONS` / `mime_for_ext` → 更新 `parse_format` → 同步本文档与 `src/api.ts`。
- 错误统一使用 `src-tauri/src/error.rs::AppError`（`Io` / `Image` / `Unsupported` / `Other`），在命令边界转成 `String`。
- 想暴露新操作给前端时，在 `commands.rs` 加 `#[tauri::command]`，并在 `lib.rs` 的 `generate_handler![]` 中注册。

---

## 7. 给 Prism 的对接摘要（前端）

- **只用 `src/api.ts`**，不要直接 `invoke`。类型、错误处理、`lumina://` URL 构造都已封装。
- **显示原图**：`img.src = await luminaImageUrl(path, { mode: "full" })`；务必监听 `error` 事件并回退到 `getImageBlob(path)`（`src/main.ts` 已示范）。
- **缩略图墙**：用 `IntersectionObserver` 懒加载 `getThumbnail(path, 128)`，并对返回的 `Blob` 调用 `URL.createObjectURL`；**记得在 `img.onload` 后 `URL.revokeObjectURL`** 释放内存。
- **上一张/下一张**：一次 `listImages` 拿到全目录 + 当前下标，本地计算索引即可，不要每张都重新列目录。
- 当前参考实现（`src/main.ts`）用**原生 TS + `<img>` + CSS transform** 实现缩放/拖拽，是最轻量的基线；若阶段 2 引入 Svelte，`api.ts` 可整体复用，Rust 契约无需改动。

---

## 8. 已知限制 / 待办

1. 自定义协议**未做解码结果缓存**，同一张图重复请求会重复解码（缩略图有懒加载缓解）。
2. **EXIF 方向未自动应用**：`orientation` 已暴露，但缩略图/原图尚未按它旋转。WebView 对 JPEG 默认会按 EXIF 旋转，可能与缩略图不一致（`<img>` 与 canvas 行为差异）。
3. `list_images` 非递归、不分页，超大目录（数万文件）需要虚拟化。
4. `save_image_as` 无质量/元数据保留参数。
5. 尚未验证的运行时项：`lumina://` 在 Windows WebView2 下的实际加载、`dialog` 权限在生产构建下的行为（阶段 1 已通过编译/单测/前端构建，GUI 冒烟见 README）。
