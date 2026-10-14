/**
 * Lumina — viewer UI.
 *
 * The Rust contract lives behind `./api` (see docs/INTERFACES.md); this file is
 * the whole frontend. It layers a full ImageGlass-style shell on top of the
 * stage-1 vertical slice — zoom/pan, fit/1:1, prev/next, thumbnails — and adds
 * a toolbar, a thumbnail sidebar, a status bar, slideshow, fullscreen, a
 * dark/light theme, keyboard shortcuts, and the stage-2B editing entry points
 * (save-as, save-rotation, crop, page navigation, rename/delete/copy-path).
 *
 * Transport / performance: the original image is streamed through the
 * `lumina://` protocol (`luminaImageUrl`) and falls back to `getImageBlob` on
 * `error`. Thumbnails are fetched lazily, one downscaled PNG per visible tile,
 * and every object URL is revoked on load/error so the webview's memory does
 * not grow with the directory size. Multi-page frames arrive as one PNG blob
 * at a time and reuse the same revoke path.
 *
 * Editing model: rotate/flip/crop are **previewed** with a CSS transform and
 * only written to disk through `transformImage`, which applies
 * `crop → flip → rotate → resize`. The preview below composes the same
 * operations in the same order, so what you see is what gets written.
 */

import {
  ENCODABLE_EXTENSIONS,
  SUPPORTED_EXTENSIONS,
  confirmAction,
  copyPathToClipboard,
  deleteFile,
  getImageBlob,
  getImageFrame,
  getImageMetadata,
  getImagePages,
  getThumbnail,
  listImages,
  luminaImageUrl,
  onFileDrop,
  onDirectoryChanged,
  pickFolder,
  pickImage,
  pickSavePathFor,
  protocolBase,
  renameFile,
  saveImageAs,
  transformImage,
  type CropRect,
  type DirectoryListing,
  type ImageEntry,
  unwatchDirectory,
  watchDirectory,
  type ImageMetadata,
} from "./api";

/* -------------------------------------------------------------------------- */
/* DOM                                                                         */
/* -------------------------------------------------------------------------- */

function el<T extends HTMLElement>(id: string): T {
  const node = document.getElementById(id);
  if (!node) throw new Error(`lumina: missing element #${id}`);
  return node as T;
}

/** Lower-cased extension of a path ("" when it has none). */
function extOf(path: string): string {
  const dot = path.lastIndexOf(".");
  if (dot < 0) return "";
  const ext = path.slice(dot + 1);
  // A dot in a directory name (`C:\my.pics\photo`) must not count.
  return ext.includes("/") || ext.includes("\\") ? "" : ext.toLowerCase();
}

/** Cache-busting / cache-key token for a file revision. */
function versionOf(entry: ImageEntry): string {
  return `${entry.modified_ms}-${entry.file_size}`;
}

const stage = el<HTMLElement>("stage");
const viewer = el<HTMLImageElement>("viewer");
const emptyHint = el<HTMLElement>("empty-hint");
const toast = el<HTMLElement>("toast");
const cropOverlay = el<HTMLElement>("crop-overlay");
const dropHint = el<HTMLElement>("drop-hint");

const thumbs = el<HTMLElement>("thumbs");
const thumbsTrack = el<HTMLElement>("thumbs-track");
const thumbsCount = el<HTMLElement>("thumbs-count");

const infoPanel = el<HTMLElement>("info-panel");
const infoList = el<HTMLElement>("info-list");

const statusFile = el<HTMLElement>("status-file");
const statusDims = el<HTMLElement>("status-dims");
const statusFormat = el<HTMLElement>("status-format");
const statusIndex = el<HTMLElement>("status-index");
const statusZoom = el<HTMLElement>("status-zoom");
const zoomLabel = el<HTMLButtonElement>("zoom-label");

const pageBar = el<HTMLElement>("page-bar");
const pageLabel = el<HTMLElement>("page-label");
const pageHint = el<HTMLElement>("page-hint");

const cropBar = el<HTMLElement>("crop-bar");
const cropX = el<HTMLInputElement>("crop-x");
const cropY = el<HTMLInputElement>("crop-y");
const cropW = el<HTMLInputElement>("crop-w");
const cropH = el<HTMLInputElement>("crop-h");
const ghostLayer = el<HTMLElement>("ghost-layer");
const ghostImg = el<HTMLImageElement>("viewer-ghost");

const emptyMessage = el<HTMLElement>("empty-message");
const renameModal = el<HTMLElement>("rename-modal");
const renameInput = el<HTMLInputElement>("rename-input");
const renameSub = el<HTMLElement>("rename-sub");
const renameError = el<HTMLElement>("rename-error");

const btnOpen = el<HTMLButtonElement>("btn-open");
const btnOpenFolder = el<HTMLButtonElement>("btn-open-folder");
const btnPrev = el<HTMLButtonElement>("btn-prev");
const btnNext = el<HTMLButtonElement>("btn-next");
const btnZoomIn = el<HTMLButtonElement>("btn-zoom-in");
const btnZoomOut = el<HTMLButtonElement>("btn-zoom-out");
const btnFit = el<HTMLButtonElement>("btn-fit");
const btnActual = el<HTMLButtonElement>("btn-actual");
const btnRotateLeft = el<HTMLButtonElement>("btn-rotate-left");
const btnRotateRight = el<HTMLButtonElement>("btn-rotate-right");
const btnFlipH = el<HTMLButtonElement>("btn-flip-h");
const btnFlipV = el<HTMLButtonElement>("btn-flip-v");
const btnResetTransform = el<HTMLButtonElement>("btn-reset-transform");
const btnSlideshow = el<HTMLButtonElement>("btn-slideshow");
const slideshowInterval = el<HTMLSelectElement>("slideshow-interval");
const btnSaveAs = el<HTMLButtonElement>("btn-save-as");
const qualitySelect = el<HTMLSelectElement>("quality-select");
const btnSaveTransform = el<HTMLButtonElement>("btn-save-transform");
const btnCrop = el<HTMLButtonElement>("btn-crop");
const btnCropApply = el<HTMLButtonElement>("btn-crop-apply");
const btnCropClear = el<HTMLButtonElement>("btn-crop-clear");
const btnCropExit = el<HTMLButtonElement>("btn-crop-exit");
const btnRename = el<HTMLButtonElement>("btn-rename");
const btnDelete = el<HTMLButtonElement>("btn-delete");
const btnCopyPath = el<HTMLButtonElement>("btn-copy-path");
const btnPagePrev = el<HTMLButtonElement>("btn-page-prev");
const btnPageNext = el<HTMLButtonElement>("btn-page-next");
const btnPageLive = el<HTMLButtonElement>("btn-page-live");
const btnThumbs = el<HTMLButtonElement>("btn-thumbs");
const btnInfo = el<HTMLButtonElement>("btn-info");
const btnTheme = el<HTMLButtonElement>("btn-theme");
const btnFullscreen = el<HTMLButtonElement>("btn-fullscreen");
const btnRenameCancel = el<HTMLButtonElement>("btn-rename-cancel");
const btnRenameConfirm = el<HTMLButtonElement>("btn-rename-confirm");

/* -------------------------------------------------------------------------- */
/* State                                                                       */
/* -------------------------------------------------------------------------- */

const MIN_SCALE = 0.02;
const MAX_SCALE = 32;
const ZOOM_STEP = 1.25;
const WHEEL_STEP = 1.1;

let meta: ImageMetadata | null = null;
let listing: DirectoryListing | null = null;
let currentPath: string | null = null;
let thumbsRendered = false;

/** Viewer transform. `tx/ty` place the image center on the stage (see below). */
let scale = 1;
let tx = 0;
let ty = 0;
/** Quarter-turn rotation in degrees: 0 | 90 | 180 | 270. */
let rotation = 0;
let flipH = false;
let flipV = false;
/** True while the view is "fit to window" — kept in sync across resize/rotate. */
let fitActive = true;

/** Guards the protocol → blob fallback from looping on repeated errors. */
let viewerFellBack = false;
let viewerBlobUrl: string | null = null;

/** Panel visibility, tracked explicitly (the DOM `hidden` prop is tri-state). */
const btnMore = el<HTMLButtonElement>("btn-more");
const moreMenu = el<HTMLElement>("more-menu");
const toolbar = el<HTMLElement>("toolbar");
let thumbsVisible = true;
let infoVisible = false;

/** Monotonic token so a slow open cannot overwrite a newer one. */
let openToken = 0;

/** Pages / frames of the current file, and the page being shown (null = native). */
let pageCount = 1;
let pageIndex: number | null = null;
/** Monotonic token for page switches, mirroring {@link openToken}. */
let pageToken = 0;
/**
 * Page-0 dimensions of the open file. `meta` is temporarily mutated to the
 * dimensions of whichever frame is on screen, so this is what restores the
 * file-level view when returning to the native pipeline.
 */
let fileDims: { w: number; h: number } | null = null;

/** Crop mode + the current selection in *source* pixel coordinates. */
let cropActive = false;

/** True while the rename modal is open; suppresses file-switch shortcuts. */
let renaming = false;

/** JPEG quality for save-as / transform, shared by both entry points. */
function quality(): number {
  return Number(qualitySelect.value) || 85;
}

function isMultiPage(): boolean {
  return pageCount > 1;
}

/* -------------------------------------------------------------------------- */
/* View transform                                                              */
/* -------------------------------------------------------------------------- */

function clampScale(value: number): number {
  return Math.min(MAX_SCALE, Math.max(MIN_SCALE, value));
}

/** True while playback is what tucked the filmstrip away, so stopping restores it. */
let thumbsHiddenBySlideshow = false;

/** Crossfade length; mirrors `--slide-ms` in styles.css. */
const SLIDE_MS = 520;
/** Set by the slideshow tick so the next open crossfades instead of cutting. */
let slideAdvance = false;
/** True when the slideshow itself went fullscreen, so stopping can hand it back. */
let fullscreenEnteredBySlideshow = false;
let ghostTimer: number | null = null;
const reduceMotion = window.matchMedia("(prefers-reduced-motion: reduce)");
/**
 * Dimensions as the viewer actually renders them.
 *
 * `meta.width/height` come from the file header (the *stored* pixel grid), but
 * the webview applies EXIF orientation itself — so orientations 5–8 (the
 * 90°/270° turns) swap width and height on screen. Every fit / zoom / crop
 * computation works in display space, so it must use these, not the raw header
 * dimensions. The same space is what the backend's `decode_oriented` produces,
 * which is why crop coordinates line up with `transform_image`.
 */
function orientedDims(m: ImageMetadata): { w: number; h: number } {
  const orientation = m.exif?.orientation ?? 1;
  const swapped = orientation >= 5 && orientation <= 8;
  return swapped ? { w: m.height, h: m.width } : { w: m.width, h: m.height };
}

/** On-screen (post-rotation) dimensions of the image, in image pixels. */
function viewDims(m: ImageMetadata): { w: number; h: number } {
  const swapped = rotation % 180 !== 0;
  return swapped ? { w: m.height, h: m.width } : { w: m.width, h: m.height };
}

/**
 * Translate offset of the CSS transform matrix for the current view.
 *
 * Model: an image point `p` maps to `t + s·(c + R(θ)·F·(p − c))`, where `c` is
 * the image center, `F` the flip signs and `R` the rotation — i.e. zoom and
 * flips scale about the center and rotation spins about the center. CSS applies
 * right-to-left, so `translate(O) rotate(θ) scale(s·fh, s·fv)` realises it with
 * `O = t + s·c − R·F·(s·c)`.
 */
function origin(): { ox: number; oy: number } {
  if (!meta) return { ox: tx, oy: ty };
  const cx = meta.width / 2;
  const cy = meta.height / 2;
  const rad = (rotation * Math.PI) / 180;
  const cos = Math.cos(rad);
  const sin = Math.sin(rad);
  // R(θ) · (s·F·c)
  const a = scale * (flipH ? -1 : 1) * cx;
  const b = scale * (flipV ? -1 : 1) * cy;
  return {
    ox: tx + scale * cx - (a * cos - b * sin),
    oy: ty + scale * cy - (a * sin + b * cos),
  };
}

function applyTransform(): void {
  if (meta) {
    const { ox, oy } = origin();
    const fh = flipH ? -1 : 1;
    const fv = flipV ? -1 : 1;
    viewer.style.transform =
      `translate(${ox}px, ${oy}px) rotate(${rotation}deg) scale(${scale * fh}, ${scale * fv})`;
  }
  const label = `${Math.round(scale * 100)}%`;
  // Skip identical writes: applyTransform() runs on every frame while the
  // window is being resized, and redundant text updates force needless work.
  if (label !== lastZoomLabel) {
    lastZoomLabel = label;
    zoomLabel.textContent = label;
    statusZoom.textContent = label;
  }
}

/**
 * Stage-relative → image-pixel mapping (the inverse of {@link applyTransform}).
 *
 * Both directions are exact for the transforms Lumina previews (quarter turns
 * and flips map axis-aligned rectangles onto axis-aligned rectangles), which is
 * what lets the crop selection convert cleanly into source coordinates.
 */
function imageToStage(px: number, py: number): { x: number; y: number } {
  const { ox, oy } = origin();
  const rad = (rotation * Math.PI) / 180;
  const cos = Math.cos(rad);
  const sin = Math.sin(rad);
  const ax = scale * (flipH ? -1 : 1) * px;
  const ay = scale * (flipV ? -1 : 1) * py;
  return { x: ox + ax * cos - ay * sin, y: oy + ax * sin + ay * cos };
}

function stageToImage(sx: number, sy: number): { x: number; y: number } {
  const { ox, oy } = origin();
  const rad = (-rotation * Math.PI) / 180;
  const cos = Math.cos(rad);
  const sin = Math.sin(rad);
  const dx = sx - ox;
  const dy = sy - oy;
  // R(−θ) · d, then undo the zoom + flip scale on each axis.
  return {
    x: (dx * cos - dy * sin) / (scale * (flipH ? -1 : 1)),
    y: (dx * sin + dy * cos) / (scale * (flipV ? -1 : 1)),
  };
}

let lastZoomLabel = "";

/** Scale that fits the whole (rotated) image inside the stage, never upscaling. */
function fitScale(): number {
  if (!meta) return 1;
  const pad = 32;
  const rect = stage.getBoundingClientRect();
  const { w, h } = viewDims(meta);
  const s = Math.min((rect.width - pad) / w, (rect.height - pad) / h);
  return clampScale(Math.min(s, 1));
}

/**
 * Center the *rotated* image in the stage.
 *
 * The image center maps to `t + s·c`, so putting it at the stage center only
 * needs `t = rect/2 − s·c` — which is also correct for 90°/270° turns, where
 * the on-screen bounding box is height×width rather than width×height.
 */
function centerImage(): void {
  if (!meta) return;
  const rect = stage.getBoundingClientRect();
  tx = rect.width / 2 - scale * (meta.width / 2);
  ty = rect.height / 2 - scale * (meta.height / 2);
  applyTransform();
}

function zoomToFit(): void {
  if (!meta) return;
  scale = fitScale();
  fitActive = true;
  centerImage();
  if (cropActive) syncCropOverlay();
}

function zoomToActual(): void {
  if (!meta) return;
  scale = 1;
  fitActive = false;
  centerImage();
  if (cropActive) syncCropOverlay();
}

/** Zoom by `factor`, keeping the stage-relative point (sx, sy) fixed. */
function zoomAt(factor: number, sx: number, sy: number): void {
  if (!meta) return;
  const next = clampScale(scale * factor);
  if (next === scale) return;
  const k = next / scale;
  tx = sx - k * (sx - tx);
  ty = sy - k * (sy - ty);
  scale = next;
  fitActive = false;
  applyTransform();
  if (cropActive) syncCropOverlay();
}

function zoomAtCenter(factor: number): void {
  const rect = stage.getBoundingClientRect();
  zoomAt(factor, rect.width / 2, rect.height / 2);
}

/** Re-fit or re-center after a rotation/flip changes the framing. */
function reflow(): void {
  if (fitActive) zoomToFit();
  else centerImage();
  if (cropActive) syncCropOverlay();
  updateTransformDependentControls();
}

function rotateBy(delta: number): void {
  if (!meta) return;
  rotation = (((rotation + delta) % 360) + 360) % 360;
  reflow();
}

function toggleFlipH(): void {
  if (!meta) return;
  flipH = !flipH;
  reflow();
}

function toggleFlipV(): void {
  if (!meta) return;
  flipV = !flipV;
  reflow();
}

function resetTransform(): void {
  if (!meta) return;
  rotation = 0;
  flipH = false;
  flipV = false;
  reflow();
}

/* -------------------------------------------------------------------------- */
/* Helpers                                                                     */
/* -------------------------------------------------------------------------- */

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  const units = ["KB", "MB", "GB"];
  let value = bytes / 1024;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return `${value.toFixed(value >= 10 ? 0 : 1)} ${units[unit]}`;
}

function parentDir(path: string): string {
  const sep = Math.max(path.lastIndexOf("\\"), path.lastIndexOf("/"));
  return sep > 0 ? path.slice(0, sep) : path;
}

let toastTimer: number | null = null;
function showToast(text: string): void {
  toast.textContent = text;
  toast.hidden = false;
  if (toastTimer !== null) window.clearTimeout(toastTimer);
  toastTimer = window.setTimeout(() => {
    toast.hidden = true;
    toastTimer = null;
  }, 2600);
}

function showPlaceholder(text: string): void {
  // Swap the branded empty state for the failure message.
  emptyHint.classList.add("is-message");
  emptyMessage.textContent = text;
  emptyMessage.hidden = false;
  emptyHint.hidden = false;
  viewer.classList.remove("visible");
}

/* -------------------------------------------------------------------------- */
/* Status bar                                                                  */
/* -------------------------------------------------------------------------- */

function updateStatus(): void {
  if (!meta) {
    statusFile.textContent = "未打开";
    statusFile.title = "";
    statusDims.textContent = "—";
    statusFormat.textContent = "—";
    statusIndex.textContent = "—";
    return;
  }
  statusFile.textContent = meta.file_name;
  statusFile.title = meta.path;
  statusDims.textContent = `${meta.width} × ${meta.height}`;
  statusFormat.textContent = meta.format.toUpperCase();

  const total = listing?.entries.length ?? 0;
  const index =
    listing && currentPath ? listing.entries.findIndex((e) => e.path === currentPath) : -1;
  statusIndex.textContent =
    total === 0 ? "—" : index >= 0 ? `${index + 1} / ${total}` : `– / ${total}`;
}

/** True when the transform preview has something worth writing to disk. */
function hasPendingTransform(): boolean {
  return rotation !== 0 || flipH || flipV;
}

function updateTransformDependentControls(): void {
  const pending = meta !== null && hasPendingTransform();
  btnResetTransform.disabled = !pending;
  // Multi-page files would only ever transform page 0 (decode() reads the first
  // page), so the entry point is disabled instead of silently dropping frames.
  btnSaveTransform.disabled = !pending || isMultiPage();
  btnSaveTransform.title = isMultiPage()
    ? "多页 / 动图暂不支持写入变换结果（后端只处理第一页）"
    : "把当前旋转/翻转写入新文件（不覆盖原图）";
}

function updateControlsEnabled(): void {
  const on = meta !== null;
  for (const button of [
    btnZoomIn,
    btnZoomOut,
    btnFit,
    btnActual,
    btnRotateLeft,
    btnRotateRight,
    btnFlipH,
    btnFlipV,
    btnSlideshow,
    zoomLabel,
    btnSaveAs,
    btnRename,
    btnDelete,
    btnCopyPath,
  ]) {
    button.disabled = !on;
  }

  btnCrop.disabled = !on || isMultiPage();
  btnCrop.title = isMultiPage()
    ? "多页 / 动图暂不支持裁剪（后端只处理第一页）"
    : "进入 / 退出裁剪模式 (C)";
  btnSaveAs.title = isMultiPage()
    ? "另存为 / 转换格式（多页 / 动图仅写入第一页）(Ctrl+S)"
    : "另存为 / 转换格式 (Ctrl+S)";

  updateTransformDependentControls();
  updateCropControls();
}

/* -------------------------------------------------------------------------- */
/* Info panel                                                                  */
/* -------------------------------------------------------------------------- */

function addInfoRow(label: string, value: string | null | undefined, wrap = false): void {
  if (value === null || value === undefined || value === "") return;
  const dt = document.createElement("dt");
  dt.textContent = label;
  const dd = document.createElement("dd");
  dd.textContent = value;
  dd.title = value;
  infoList.append(dt, dd);
}

function addInfoSub(title: string): void {
  const div = document.createElement("div");
  div.className = "info-sub";
  div.textContent = title;
  infoList.append(div);
}

/** Decimal degrees with a hemisphere suffix, e.g. `31.230416° N`. */
function formatLatitude(value: number): string {
  return `${Math.abs(value).toFixed(6)}° ${value >= 0 ? "N" : "S"}`;
}

function formatLongitude(value: number): string {
  return `${Math.abs(value).toFixed(6)}° ${value >= 0 ? "E" : "W"}`;
}

function renderInfo(m: ImageMetadata): void {
  infoList.replaceChildren();
  addInfoRow("文件名", m.file_name);
  addInfoRow("格式", m.format.toUpperCase());
  addInfoRow("尺寸", `${m.width} × ${m.height}`);
  addInfoRow("文件大小", formatBytes(m.file_size));
  addInfoRow("路径", m.path, true);

  const e = m.exif;
  if (!e) return;

  addInfoSub("EXIF");
  addInfoRow("相机", [e.make, e.model].filter(Boolean).join(" ") || null, true);
  addInfoRow("镜头", e.lens_model, true);
  addInfoRow("拍摄时间", e.datetime);
  addInfoRow("曝光时间", e.exposure_time);
  addInfoRow("光圈", e.f_number);
  addInfoRow("ISO", e.iso);
  addInfoRow("焦距", e.focal_length);
  addInfoRow("方向", e.orientation != null ? String(e.orientation) : null);
  addInfoRow("作者", e.artist);
  addInfoRow("版权", e.copyright, true);
  addInfoRow("描述", e.description, true);
  addInfoRow("软件", e.software);

  if (wrap) dd.classList.add("wrap");
  const hasGps =
    e.gps_latitude !== null || e.gps_longitude !== null || e.gps_altitude !== null;
  if (hasGps) {
    addInfoSub("位置");
    addInfoRow("纬度", e.gps_latitude !== null ? formatLatitude(e.gps_latitude) : null);
    addInfoRow("经度", e.gps_longitude !== null ? formatLongitude(e.gps_longitude) : null);
    addInfoRow(
      "海拔",
      e.gps_altitude !== null ? `${e.gps_altitude.toFixed(1)} m` : null,
    );
  }
}

/* -------------------------------------------------------------------------- */
/* Thumbnail sidebar (lazy, protocol transport, bounded concurrency)           */
/* -------------------------------------------------------------------------- */

/** Max thumbnails loading at once, so a fast scroll cannot saturate the CPU. */
const THUMB_CONCURRENCY = 6;

/** Tiles waiting for a free slot in the loading pool. */
  addInfoSub("文件");
const thumbQueue: HTMLImageElement[] = [];
let thumbsInFlight = 0;

const thumbObserver = new IntersectionObserver(
  (entries) => {
    for (const entry of entries) {
      if (!entry.isIntersecting) continue;
      const img = entry.target as HTMLImageElement;
      thumbObserver.unobserve(img);
      thumbQueue.push(img);
    }
    pumpThumbs();
  },
  { root: thumbsTrack, rootMargin: "240px" },
);

/** Start as many queued tiles as the concurrency budget allows. */
function pumpThumbs(): void {
  while (thumbsInFlight < THUMB_CONCURRENCY && thumbQueue.length > 0) {
    const img = thumbQueue.shift() as HTMLImageElement;
    thumbsInFlight++;
    void loadThumbnail(img).finally(() => {
      thumbsInFlight--;
      pumpThumbs();
    });
  }
}

/** Set `img.src` and resolve once the image has loaded or failed. */
function setImgSrc(img: HTMLImageElement, url: string): Promise<void> {
  return new Promise((resolve) => {
    img.addEventListener("load", () => resolve(), { once: true });
    img.addEventListener("error", () => resolve(), { once: true });
    img.src = url;
  });
}

/**
 * Load one tile through the `lumina://` protocol (streamed by Rust, no IPC
 * round-trip, no Blob). Falls back to the IPC command once if the protocol is
 * unavailable, mirroring the main viewer's protocol → blob path.
 */
async function loadThumbnail(img: HTMLImageElement): Promise<void> {
  const path = img.dataset.path;
  if (!path) return;

  try {
    const url = await luminaImageUrl(path, {
      mode: "thumb",
      max: 192,
      version: img.dataset.version,
    });
    await setImgSrc(img, url);
    if (img.naturalWidth > 0) return;
  } catch {
    /* fall through to the IPC fallback */
  }

  try {
    const blob = await getThumbnail(path, 192);
    const url = URL.createObjectURL(blob);
    try {
      await setImgSrc(img, url);
      if (img.naturalWidth === 0) throw new Error("thumbnail decode failed");
    } finally {
      URL.revokeObjectURL(url);
    }
  } catch {
    img.closest(".thumb")?.classList.add("thumb-failed");
  }
}

function renderThumbs(list: DirectoryListing, active: string): void {
  thumbsTrack.replaceChildren();
  thumbsCount.textContent = `${list.entries.length} 张`;

  const frag = document.createDocumentFragment();
  list.entries.forEach((entry, index) => {
    const button = document.createElement("button");
    button.type = "button";
    button.className = "thumb";
    button.dataset.index = String(index);
    button.title = entry.file_name;
    if (entry.path === active) button.classList.add("active");

    const img = document.createElement("img");
    img.dataset.path = entry.path;
    img.dataset.version = versionOf(entry);
    img.alt = entry.file_name;
    img.draggable = false;

    const name = document.createElement("span");
    name.className = "thumb-name";
    name.textContent = entry.file_name;

    button.append(img, name);
    frag.appendChild(button);
  });
  thumbsTrack.appendChild(frag);

  for (const img of thumbsTrack.querySelectorAll<HTMLImageElement>("img")) {
    thumbObserver.observe(img);
  }
  thumbsTrack.querySelector(".thumb.active")?.scrollIntoView({ block: "nearest" });
  thumbsRendered = true;
}

function clearThumbs(): void {
  thumbsTrack.replaceChildren();
  thumbsCount.textContent = "";
  thumbsRendered = false;
  thumbQueue.length = 0;
}

/** Move the highlight without rebuilding the strip (prev/next within a dir). */
function updateThumbActive(): void {
  thumbsTrack.querySelector(".thumb.active")?.classList.remove("active");
  if (!listing || !currentPath) return;
  const index = listing.entries.findIndex((e) => e.path === currentPath);
  if (index < 0) return;
  const button = thumbsTrack.querySelector<HTMLElement>(`.thumb[data-index="${index}"]`);
  button?.classList.add("active");
  button?.scrollIntoView({ block: "nearest" });
}

/* -------------------------------------------------------------------------- */
/* Loading                                                                     */
/* -------------------------------------------------------------------------- */

function releaseViewerBlob(): void {
  if (viewerBlobUrl !== null) {
    URL.revokeObjectURL(viewerBlobUrl);
    viewerBlobUrl = null;
  }
}

/**
 * Align `meta` with the bitmap the engine actually decoded.
 *
 * The EXIF-derived dimensions are right for every format Lumina renders, but
 * `naturalWidth`/`naturalHeight` are authoritative — they also cover the paths
 * where the backend bakes the orientation into the pixels (TIFF transcode,
 * multi-page frames). Called once per open, after the image loads; a no-op when
 * the two already agree, which is the normal case.
 */
function reconcileViewerDims(): void {
  if (!meta) return;
  const w = viewer.naturalWidth;
  const h = viewer.naturalHeight;
  if (w <= 0 || h <= 0) return;
  if (w === meta.width && h === meta.height) return;
  meta = { ...meta, width: w, height: h };
  fileDims = { w, h };
  updateStatus();
  reflow();
}

async function openPath(path: string): Promise<void> {
  const token = ++openToken;

  let m: ImageMetadata;
  try {
    m = await getImageMetadata(path);
  } catch (err) {
    showPlaceholder(`无法打开：${String(err)}`);
    return;
  }
  if (token !== openToken) return;

  // The webview rotates EXIF-oriented images itself, so `meta` must carry the
  // *display* dimensions (stored header dims swapped for orientation 5–8) — the
  // whole view/fit/crop maths, and the backend's `decode_oriented`, share that
  // space.
  const dims = orientedDims(m);
  const oriented: ImageMetadata = { ...m, width: dims.w, height: dims.h };
  meta = oriented;
  currentPath = m.path;
  fileDims = { w: dims.w, h: dims.h };
  viewerFellBack = false;
  releaseViewerBlob();
  rotation = 0;
  flipH = false;
  flipV = false;
  pageCount = 1;
  pageIndex = null;
  setCropMode(false);

  emptyHint.hidden = true;
  emptyHint.classList.remove("is-message");
  emptyMessage.hidden = true;
  // While animating, the ghost covers the stage: the viewer stays dark until
  // the incoming frame has decoded, then crossfades in.
  if (!animate || !parkViewerInGhost()) viewer.classList.add("visible");
  renderInfo(oriented);
  updateStatus();
  updateControlsEnabled();
  updatePageControls();

  // Put the image on screen before anything that is not required to show it.
  const url = await luminaImageUrl(m.path, {
    mode: "full",
    version: `${m.modified_ms}-${m.file_size}`,
  });
  if (token !== openToken) return;
  viewer.src = url;
/* -------------------------------------------------------------------------- */
/* Slideshow transition                                                        */
/* -------------------------------------------------------------------------- */

/**
 * Freeze the frame currently on screen into the ghost layer.
 *
 * The ghost is a *snapshot*, not a second viewer: it copies the outgoing image
 * URL and the live transform verbatim, so it lands pixel-for-pixel over
 * `#viewer` and the incoming frame can be swapped underneath it unseen.
 * Returns false when there is nothing worth snapshotting.
 */
function parkViewerInGhost(): boolean {
  if (!viewer.src || viewer.naturalWidth === 0) return false;
  ghostImg.src = viewer.src;
  ghostImg.style.width = `${viewer.naturalWidth}px`;
  ghostImg.style.height = `${viewer.naturalHeight}px`;
  ghostLayer.style.transform = viewer.style.transform;
  ghostLayer.classList.remove("leaving");
  ghostLayer.hidden = false;
  viewer.classList.remove("visible");
  // Safety net: a frame that never decodes must not strand the ghost.
  if (ghostTimer !== null) window.clearTimeout(ghostTimer);
  ghostTimer = window.setTimeout(releaseGhost, 2500);
  return true;
}

/** Fade the incoming frame in and let the parked frame drift out. */
function releaseGhost(): void {
  viewer.classList.add("visible");
  if (ghostLayer.hidden) return;
  if (ghostTimer !== null) window.clearTimeout(ghostTimer);
  ghostLayer.classList.add("leaving");
  ghostTimer = window.setTimeout(clearGhost, SLIDE_MS + 80);
}

/** Drop the ghost layer and reset it for the next transition. */
function clearGhost(): void {
  if (ghostTimer !== null) {
    window.clearTimeout(ghostTimer);
    ghostTimer = null;
  }
  ghostLayer.hidden = true;
  ghostLayer.classList.remove("leaving");
  ghostLayer.style.transform = "";
  ghostImg.removeAttribute("src");
  ghostImg.style.width = "";
  ghostImg.style.height = "";
}


  // Safety net: the decoded bitmap is authoritative. If the engine's orientation
  // differs from what the EXIF tag predicted (missing tag, unusual container),
  // adopt its dimensions and re-fit once the image has loaded.
  viewer.addEventListener(
    "load",
    () => {
      if (token !== openToken) return;
      reconcileViewerDims();
      if (animate) releaseGhost();
    },
    { once: true },
  );

  // Decide the transition style before `currentPath` is reassigned below: a
  // slideshow advance crossfades, every other open cuts.
  const animate =
    slideAdvance &&
    !reduceMotion.matches &&
    currentPath !== null &&
    currentPath !== m.path &&
    viewer.classList.contains("visible");
  slideAdvance = false;

  // Fit uses the metadata dimensions, so it does not wait for `load`.
  zoomToFit();

  // Frame count only matters for animated / multi-page containers, and counting
  // a GIF/WebP decodes every frame — so gate by extension (matching the
  // backend's `page_count` dispatch) and never await it: the image is already
  // on screen. Native animated formats keep playing either way.
  const ext = extOf(m.path);
  if (ext === "gif" || ext === "webp" || ext === "tif" || ext === "tiff") {
    void getImagePages(m.path)
      .then((count) => {
        if (token !== openToken) return;
        pageCount = count || 1;
        updateControlsEnabled();
        updatePageControls();
      })
      .catch(() => {
        /* leave pageCount at 1 */
      });
  }

  // Keep a single directory listing; only re-list when the folder changes.
  const dir = parentDir(m.path);
  const needList = listing === null || !listing.entries.some((e) => e.path === m.path);
  if (needList) {
    try {
      listing = await listImages(dir, m.path);
    } catch {
      listing = null;
    }
    if (token !== openToken) return;
    clearThumbs();
  }

  updateNavigation();
  updateStatus();

  if (listing && listing.entries.length > 0) {
    if (!thumbsRendered) renderThumbs(listing, m.path);
    else updateThumbActive();
  } else {
    clearThumbs();
  }

  preloadNeighbours(m.path);
}

/* -------------------------------------------------------------------------- */
/* Preloading                                                                  */
/* -------------------------------------------------------------------------- */

/** Keeps preloaded images alive so the webview can reuse their decoded bitmap. */
const preloadCache = new Map<string, HTMLImageElement>();
const PRELOAD_MAX = 4;

/**
 * Warm the neighbours of the open image so prev/next feel instant. The real
 * gain is the OS page cache plus the backend's own caches; keeping the `Image`
 * objects referenced additionally lets the webview reuse a decoded bitmap when
 * the same URL is later assigned to the viewer.
 */
function preloadNeighbours(path: string): void {
  if (!listing) return;
  const index = listing.entries.findIndex((e) => e.path === path);
  if (index < 0) return;
  for (const offset of [1, -1]) {
    const entry = listing.entries[index + offset];
    if (entry) void preloadImage(entry);
  }
}

async function preloadImage(entry: ImageEntry): Promise<void> {
  const version = versionOf(entry);
  const key = `${entry.path}|${version}`;
  if (preloadCache.has(key)) return;

  const img = new Image();
  preloadCache.set(key, img);
  if (preloadCache.size > PRELOAD_MAX) {
    const oldest = preloadCache.keys().next().value;
    if (oldest !== undefined) preloadCache.delete(oldest);
  }

  try {
    img.src = await luminaImageUrl(entry.path, { mode: "full", version });
  } catch {
    preloadCache.delete(key);
  }
  // Keep the filmstrip in step with the folder from here on.
  if (listing) ensureWatching(listing.dir);

}

async function openDirectory(dir: string): Promise<void> {
  const token = ++openToken;
  let list: DirectoryListing;
  try {
    list = await listImages(dir);
  } catch (err) {
    showToast(`无法读取文件夹：${String(err)}`);
    return;
  }
  if (token !== openToken) return;

  if (list.entries.length === 0) {
    showToast("该文件夹内没有支持的图片");
    return;
  }
  listing = list;
  clearThumbs();
  await openPath(list.entries[0].path);
}

function updateNavigation(): void {
  const enabled = listing !== null && listing.entries.length > 1;
  btnPrev.disabled = !enabled;
  btnNext.disabled = !enabled;
}

function step(delta: number, wrap = false): void {
  if (!listing || !currentPath) return;
  const count = listing.entries.length;
  const index = listing.entries.findIndex((e) => e.path === currentPath);
  if (index < 0) return;
  let next = index + delta;
  if (wrap) next = (next + count) % count;
  if (next < 0 || next >= count) return;
  void openPath(listing.entries[next].path);
}

function jumpTo(index: number): void {
  if (!listing || listing.entries.length === 0) return;
  const i = index < 0 ? listing.entries.length - 1 : index;
  const entry = listing.entries[i];
  if (entry) void openPath(entry.path);
}

/* -------------------------------------------------------------------------- */
/* Output helpers (save-as / transform)                                        */
/* -------------------------------------------------------------------------- */

const ENCODABLE: readonly string[] = ENCODABLE_EXTENSIONS;

function baseName(path: string): string {
  const sep = Math.max(path.lastIndexOf("\\"), path.lastIndexOf("/"));
  return sep >= 0 ? path.slice(sep + 1) : path;
}

function joinPath(dir: string, name: string): string {
  const sep = dir.includes("\\") ? "\\" : "/";
  return dir.endsWith(sep) ? `${dir}${name}` : `${dir}${sep}${name}`;
}

/** Output extension for a source image, falling back to PNG when unencodable. */
function encodableExt(m: ImageMetadata): string {
  const ext = m.format.toLowerCase();
  return ENCODABLE.includes(ext) ? ext : "png";
/* -------------------------------------------------------------------------- */
/* Filesystem watch                                                            */
/* -------------------------------------------------------------------------- */

/** Directory the backend is watching, so we only re-arm it when it changes. */
let watchedDir: string | null = null;

function ensureWatching(dir: string): void {
  if (watchedDir === dir) return;
  watchedDir = dir;
  watchDirectory(dir).catch(() => {
    // Not fatal — the filmstrip simply stops tracking the folder.
    watchedDir = null;
  });
}

/**
 * Re-list the browsed directory and bring the UI back in line with the disk.
 *
 * Nothing is applied incrementally: the listing is replaced wholesale, so a
 * coalesced — or even dropped — event can never leave a stale entry on screen.
 */
async function refreshListing(): Promise<void> {
  const dir = listing?.dir;
  if (dir === undefined) return;

  let fresh: DirectoryListing;
  try {
    fresh = await listImages(dir, currentPath ?? undefined);
  } catch {
    // The folder itself is gone — nothing left to browse or watch.
    watchedDir = null;
    void unwatchDirectory();
    listing = null;
    resetViewerState(`无法读取文件夹：${dir}`);
    return;
  }
  // A newer open won the race — drop this result.
  if (listing === null || listing.dir !== dir) return;

  const previous = listing;
  const open = currentPath;
  const entry = open !== null ? fresh.entries.find((e) => e.path === open) : undefined;
  listing = fresh;
  thumbsRendered = false;

  // Rewritten in place: re-decode under a fresh version token.
  if (
    open !== null &&
    entry !== undefined &&
    meta !== null &&
    (entry.modified_ms !== meta.modified_ms || entry.file_size !== meta.file_size)
  ) {
    await openPath(entry.path);
    return;
  }

  // The open file vanished, or nothing is open yet: adopt the nearest entry.
  if (entry === undefined) {
    const wasAt = open !== null ? previous.entries.findIndex((e) => e.path === open) : 0;
    const next = fresh.entries[Math.min(Math.max(wasAt, 0), fresh.entries.length - 1)];
    if (next !== undefined) {
      if (open !== null) showToast(`“${baseName(open)}” 已被删除`);
      await openPath(next.path);
      return;
    }
    if (open !== null) resetViewerState("目录内的图片已全部移除");
    else {
      clearThumbs();
      updateNavigation();
      updateStatus();
    }
    return;
  }

  renderThumbs(fresh, entry.path);
  updateNavigation();
  updateStatus();
}

/** Answer a watch event, ignoring folders we have since navigated away from. */
function handleDirectoryChanged(dir: string): void {
  if (listing === null || listing.dir !== dir) return;
  void refreshListing();
}

}

/** Default "save as" target: sibling of the source, same format, suffixed. */
function defaultOutputPath(m: ImageMetadata, suffix: string): string {
  const dir = parentDir(m.path);
  const stem = baseName(m.path).replace(/\.[^.]+$/, "");
  return joinPath(dir, `${stem}${suffix}.${encodableExt(m)}`);
}

/** Another image (not the one currently displayed) may be the output path. */
function warnIfOutputIsSource(outputPath: string): boolean {
  if (meta === null || outputPath.toLowerCase() !== meta.path.toLowerCase()) return false;
  showToast("输出路径与原文件相同，已取消以免覆盖原图");
  return true;
}

/** 另存为 / 格式转换（不改几何）。Output goes through the file dialog. */
async function saveAs(): Promise<void> {
  if (!meta) return;
  const out = await pickSavePathFor(defaultOutputPath(meta, "_另存"), ENCODABLE);
  if (!out || warnIfOutputIsSource(out)) return;
  try {
    const written = await saveImageAs(meta.path, out, undefined, quality());
    showToast(`已另存为 ${baseName(written)}`);
  } catch (err) {
    showToast(`另存失败：${String(err)}`);
  }
}

/**
 * Write the current rotate/flip preview to a new file.
 *
 * The preview and the backend both apply `crop → flip → rotate`, so the written
 * image matches what is on screen. The source file is never overwritten.
 */
async function saveTransformResult(): Promise<void> {
  if (!meta || !hasPendingTransform()) return;
  const out = await pickSavePathFor(defaultOutputPath(meta, "_变换"), ENCODABLE);
  if (!out || warnIfOutputIsSource(out)) return;
  try {
    const written = await transformImage(meta.path, out, {
      rotate: rotation,
      flipHorizontal: flipH,
      flipVertical: flipV,
      quality: quality(),
    });
    showToast(`已保存变换结果：${baseName(written)}`);
  } catch (err) {
    showToast(`保存失败：${String(err)}`);
  }
}

/* -------------------------------------------------------------------------- */
/* Crop                                                                        */
/* -------------------------------------------------------------------------- */

function readCropInputs(): CropRect | null {
  const x = Number(cropX.value);
  const y = Number(cropY.value);
  const width = Number(cropW.value);
  const height = Number(cropH.value);
  if (![x, y, width, height].every((n) => Number.isFinite(n))) return null;
  return {
    x: Math.round(x),
    y: Math.round(y),
    width: Math.round(width),
    height: Math.round(height),
  };
}

/** Selection clamped to the image; null when it is empty or out of bounds. */
function validateCrop(): CropRect | null {
  if (!meta) return null;
  const r = readCropInputs();
  if (!r) return null;
  if (r.width < 1 || r.height < 1) return null;
  if (r.x < 0 || r.y < 0 || r.x >= meta.width || r.y >= meta.height) return null;
  return {
    x: r.x,
    y: r.y,
    width: Math.min(r.width, meta.width - r.x),
    height: Math.min(r.height, meta.height - r.y),
  };
}

/**
 * Redraw the selection box from the numeric inputs.
 *
 * Quarter turns and flips map axis-aligned source rectangles onto axis-aligned
 * screen rectangles, so transforming the four corners and taking the bounding
 * box is exact rather than an approximation.
 */
function syncCropOverlay(): void {
  if (!cropActive || !meta) {
    cropOverlay.hidden = true;
    return;
  }
  const r = readCropInputs();
  if (!r || r.width < 1 || r.height < 1) {
    cropOverlay.hidden = true;
    return;
  }
  const corners = [
    imageToStage(r.x, r.y),
    imageToStage(r.x + r.width, r.y),
    imageToStage(r.x, r.y + r.height),
    imageToStage(r.x + r.width, r.y + r.height),
  ];
  const xs = corners.map((c) => c.x);
  const ys = corners.map((c) => c.y);
  const left = Math.min(...xs);
  const top = Math.min(...ys);
  const right = Math.max(...xs);
  const bottom = Math.max(...ys);

  cropOverlay.style.left = `${left}px`;
  cropOverlay.style.top = `${top}px`;
  cropOverlay.style.width = `${right - left}px`;
  cropOverlay.style.height = `${bottom - top}px`;
  cropOverlay.hidden = false;
}

function setCropRect(rect: CropRect | null): void {
  if (rect) {
    cropX.value = String(rect.x);
    cropY.value = String(rect.y);
    cropW.value = String(rect.width);
    cropH.value = String(rect.height);
  } else {
    cropX.value = "0";
    cropY.value = "0";
    cropW.value = "0";
    cropH.value = "0";
  }
  syncCropOverlay();
  updateCropControls();
}

/** Screen-space (stage-relative) rect → clamped source-image rect. */
function cropRectFromStage(a: { x: number; y: number }, b: { x: number; y: number }): CropRect | null {
  if (!meta) return null;
  const p = stageToImage(a.x, a.y);
  const q = stageToImage(b.x, b.y);
  const x = Math.round(Math.min(p.x, q.x));
  const y = Math.round(Math.min(p.y, q.y));
  const right = Math.round(Math.max(p.x, q.x));
  const bottom = Math.round(Math.max(p.y, q.y));

  const cx = Math.min(Math.max(x, 0), meta.width);
  const cy = Math.min(Math.max(y, 0), meta.height);
  const width = Math.min(right, meta.width) - cx;
  const height = Math.min(bottom, meta.height) - cy;
  if (width < 1 || height < 1) return null;
  return { x: cx, y: cy, width, height };
}

function updateCropControls(): void {
  btnCrop.setAttribute("aria-pressed", cropActive ? "true" : "false");
  btnCropApply.disabled = !cropActive || validateCrop() === null;
}

function setCropMode(on: boolean): void {
  cropActive = on && meta !== null;
  cropBar.hidden = !cropActive;
  stage.classList.toggle("cropping", cropActive);
  if (cropActive) setCropRect(null);
  else cropOverlay.hidden = true;
  updateCropControls();
}

/** Crop the selection (plus any pending rotate/flip) into a new file. */
async function applyCrop(): Promise<void> {
  if (!meta || !cropActive) return;
  const rect = validateCrop();
  if (!rect) {
    showToast("裁剪区域无效：请先在图像上框选，或填写有效的源图坐标");
    return;
  }
  const out = await pickSavePathFor(defaultOutputPath(meta, "_裁剪"), ENCODABLE);
  if (!out || warnIfOutputIsSource(out)) return;
  try {
    const written = await transformImage(meta.path, out, {
      crop: rect,
      rotate: rotation,
      flipHorizontal: flipH,
      flipVertical: flipV,
      quality: quality(),
    });
    showToast(`裁剪结果已保存：${baseName(written)}`);
  } catch (err) {
    showToast(`裁剪失败：${String(err)}`);
  }
}

/* -------------------------------------------------------------------------- */
/* Multi-page / animation navigation                                           */
/* -------------------------------------------------------------------------- */

function isAnimatedFormat(m: ImageMetadata): boolean {
  const f = m.format.toLowerCase();
  return f === "gif" || f === "webp";
}

function updatePageControls(): void {
  const multi = isMultiPage() && meta !== null;
  pageBar.hidden = !multi;
  if (!multi || !meta) return;

  const shown = pageIndex ?? 0;
  pageLabel.textContent = `${shown + 1} / ${pageCount}`;
  btnPagePrev.disabled = shown <= 0;
  btnPageNext.disabled = shown >= pageCount - 1;

  const animated = isAnimatedFormat(meta);
  btnPageLive.hidden = !animated;
  pageHint.textContent = animated
    ? pageIndex === null
      ? "正在播放动画（原生）"
      : "已切换到静态帧"
    : "多页 TIFF：逐页查看";
}

/**
 * Show one frame as a PNG blob.
 *
 * Native formats keep animating through `lumina://`; this is the "pause / step"
 * path. The blob replaces whatever the viewer was showing, so it reuses the
 * same revoke slot and the same error guard.
 */
async function gotoPage(index: number): Promise<void> {
  if (!meta || !currentPath || !isMultiPage()) return;
  const i = Math.max(0, Math.min(pageCount - 1, index));
  const token = ++pageToken;
  pageIndex = i;
  updatePageControls();

  try {
    const blob = await getImageFrame(currentPath, i);
    if (token !== pageToken || !currentPath) return;

    // Frames can differ in size from page 0, so sync the view maths to the
    // frame that is actually on screen before fitting it.
    let w = 0;
    let h = 0;
    try {
      const bitmap = await createImageBitmap(blob);
      w = bitmap.width;
      h = bitmap.height;
      bitmap.close();
    } catch {
      /* keep the metadata dimensions if the bitmap cannot be decoded */
    }
    if (w > 0 && h > 0) meta = { ...meta, width: w, height: h };
    if (token !== pageToken) return;

    viewerFellBack = false;
    releaseViewerBlob();
    viewerBlobUrl = URL.createObjectURL(blob);
    viewer.src = viewerBlobUrl;
    updateStatus();
    reflow();
  } catch (err) {
    if (token !== pageToken) return;
    showToast(`无法读取第 ${i + 1} 页：${String(err)}`);
    pageIndex = null;
    updatePageControls();
  }
}

/** Return to the native pipeline (animated GIF/WebP plays again). */
async function showLiveFrame(): Promise<void> {
  if (!meta || !currentPath) return;
  pageToken += 1;
  pageIndex = null;
  updatePageControls();
  if (fileDims) meta = { ...meta, width: fileDims.w, height: fileDims.h };
  viewerFellBack = false;
  releaseViewerBlob();
  viewer.src = await luminaImageUrl(currentPath, { mode: "full" });
  updateStatus();
  reflow();
}

/* -------------------------------------------------------------------------- */
/* File operations                                                             */
/* -------------------------------------------------------------------------- */

let renameTarget: string | null = null;

function showRenameError(text: string): void {
  renameError.textContent = text;
  renameError.hidden = false;
}

function openRenameModal(path: string): void {
  renaming = true;
  renameTarget = path;
  renameInput.value = baseName(path);
  renameSub.textContent = path;
  renameError.hidden = true;
  renameError.textContent = "";
  renameModal.hidden = false;
  renameInput.focus();
  renameInput.select();
}

function closeRenameModal(): void {
  renaming = false;
  renameTarget = null;
  renameModal.hidden = true;
}

async function submitRename(): Promise<void> {
  if (!renameTarget) return;
  const from = renameTarget;
  const next = renameInput.value.trim();
  if (!next) {
    showRenameError("文件名不能为空");
    return;
  }
  if (next === baseName(from)) {
    closeRenameModal();
    return;
  }
  btnRenameConfirm.disabled = true;
  try {
    const renamed = await renameFile(from, next);
    closeRenameModal();
    showToast(`已重命名为 ${baseName(renamed)}`);
    // The renamed path is not in the cached listing, so openPath re-lists.
    await openPath(renamed);
  } catch (err) {
    showRenameError(String(err));
  } finally {
    btnRenameConfirm.disabled = false;
  }
}

/** Clear every per-file bit of state and show the placeholder again. */
function resetViewerState(message: string): void {
  void stopSlideshow();
  releaseViewerBlob();
  meta = null;
  currentPath = null;
  rotation = 0;
  flipH = false;
  flipV = false;
  pageCount = 1;
  pageIndex = null;
  fileDims = null;
  setCropMode(false);
  viewer.removeAttribute("src");
  showPlaceholder(message);
  clearThumbs();
  updateStatus();
  updateNavigation();
  updateControlsEnabled();
  updatePageControls();
}

/** Delete the open file after an explicit confirmation. */
async function removeCurrent(): Promise<void> {
  if (!meta || !currentPath) return;
  const path = currentPath;
  const name = meta.file_name;
  const ok = await confirmAction(
    `确定要删除“${name}”吗？\n\n此操作不可撤销，文件不会被移入回收站。`,
    "删除文件",
  );
  if (!ok) return;

  void stopSlideshow();
  try {
    await deleteFile(path);
  } catch (err) {
    showToast(`删除失败：${String(err)}`);
    return;
  }
  showToast(`已删除 ${name}`);

  const entries = listing?.entries;
  if (entries) {
    const index = entries.findIndex((e) => e.path === path);
    if (index >= 0) entries.splice(index, 1);
    thumbsRendered = false;
    if (entries.length > 0) {
      const next = entries[Math.min(Math.max(index, 0), entries.length - 1)];
      await openPath(next.path);
      return;
    }
  }
  resetViewerState(`已删除“${name}”，目录内没有更多图片`);
}

async function copyCurrentPath(): Promise<void> {
  if (!currentPath) return;
  try {
    await copyPathToClipboard(currentPath);
    showToast("已复制文件路径");
  } catch (err) {
    showToast(`复制失败：${String(err)}`);
  }
}

/* -------------------------------------------------------------------------- */
/* Native drag & drop                                                          */
/* -------------------------------------------------------------------------- */

function isSupportedImage(path: string): boolean {
  const dot = path.lastIndexOf(".");
  if (dot < 0) return false;
  const ext = path.slice(dot + 1).toLowerCase();
  return (SUPPORTED_EXTENSIONS as readonly string[]).includes(ext);
}

function setDropHint(visible: boolean): void {
  stage.classList.toggle("drop-target", visible);
  dropHint.hidden = !visible;
}

async function handleDroppedPath(path: string): Promise<void> {
  if (isSupportedImage(path)) {
    await openPath(path);
    return;
  }
  // Not a known image extension — assume a folder and let the backend decide
  // (openDirectory reports a readable error when it is neither).
  await openDirectory(path);
}

function wireDragAndDrop(): void {
  void onFileDrop((phase, paths) => {
    if (phase === "enter") {
      setDropHint(true);
    } else if (phase === "leave") {
      setDropHint(false);
    } else if (phase === "drop") {
      setDropHint(false);
      const first = paths[0];
      if (first) void handleDroppedPath(first);
    }
  }).catch(() => {
    // Drag & drop is a progressive enhancement; ignore when unavailable.
  });
}

/* -------------------------------------------------------------------------- */
/* Slideshow                                                                   */
/* -------------------------------------------------------------------------- */

let slideshowTimer: number | null = null;

function updateSlideshowButton(): void {
  const playing = slideshowTimer !== null;
  // Icon-only button: the play/pause swap is driven by aria-pressed in CSS.
  btnSlideshow.setAttribute("aria-pressed", playing ? "true" : "false");
}

async function stopSlideshow(): Promise<void> {
  if (slideshowTimer !== null) {
    window.clearInterval(slideshowTimer);
    slideshowTimer = null;
  }
  updateSlideshowButton();
}

async function startSlideshow(): Promise<void> {
  if (!listing || listing.entries.length < 2) {
    showToast("幻灯片需要当前目录中至少两张图片");
    return;
  }
  const seconds = Number(slideshowInterval.value) || 3;
  slideshowTimer = window.setInterval(slideshowTick, seconds * 1000);
  stage.classList.add("slideshow");
  // Playback is about the image alone — tuck the filmstrip away, and remember
  // it so stopping restores whatever the user had before.
  if (thumbsVisible) {
    thumbsHiddenBySlideshow = true;
    setThumbsVisible(false);
  }
  // ...and step into fullscreen so the chrome gets out of the way.
  if (!document.fullscreenElement) {
    try {
      await document.documentElement.requestFullscreen();
      fullscreenEnteredBySlideshow = true;
    } catch {
      /* fullscreen refused — keep playing windowed */
    }
  }
  updateSlideshowButton();
}

function toggleSlideshow(): void {
  if (slideshowTimer !== null) void stopSlideshow();
  else void startSlideshow();
}

/* -------------------------------------------------------------------------- */
/* Fullscreen / theme / panels                                                 */
/* -------------------------------------------------------------------------- */

async function toggleFullscreen(): Promise<void> {
  try {
    if (document.fullscreenElement) await document.exitFullscreen();
    else await document.documentElement.requestFullscreen();
  } catch (err) {
    showToast(`无法切换全屏：${String(err)}`);
  }
}

function setThumbsVisible(visible: boolean): void {
  thumbsVisible = visible;
  thumbs.hidden = !visible;
  btnThumbs.setAttribute("aria-pressed", visible ? "true" : "false");
}

function setInfoVisible(visible: boolean): void {
  infoVisible = visible;
  infoPanel.hidden = !visible;
  btnInfo.setAttribute("aria-pressed", visible ? "true" : "false");
}

type Theme = "dark" | "light";

function applyTheme(theme: Theme): void {
  document.documentElement.dataset.theme = theme;
  // Icon-only button: sun while light, moon while dark; the title mirrors the
  // action the click will perform.
  btnTheme.dataset.state = theme === "light" ? "on" : "off";
  btnTheme.title = theme === "light" ? "切换到深色主题" : "切换到浅色主题";
  try {
    localStorage.setItem("lumina-theme", theme);
  } catch {
    /* storage unavailable — theme stays for this session only */
  }
}

function initTheme(): void {
  let saved: string | null = null;
  try {
    saved = localStorage.getItem("lumina-theme");
  } catch {
    saved = null;
  }
  applyTheme(saved === "light" ? "light" : "dark");
}

/* -------------------------------------------------------------------------- */
/* Events                                                                      */
/* -------------------------------------------------------------------------- */

viewer.addEventListener("error", async () => {
  // Fallback transport: pull the bytes over IPC and use a blob URL. A frame
  // blob that fails would fall back to page 0, which is more confusing than
  // just reporting the failure, so skip the fallback while stepping frames.
  if (viewerFellBack || !currentPath || pageIndex !== null) return;
  viewerFellBack = true;
  const path = currentPath;
  try {
    const blob = await getImageBlob(path);
    if (path !== currentPath) return;
    releaseViewerBlob();
    viewerBlobUrl = URL.createObjectURL(blob);
    viewer.src = viewerBlobUrl;
  } catch (err) {
    showPlaceholder(`无法显示图片：${String(err)}`);
  }
});

btnOpen.addEventListener("click", async () => {
  const path = await pickImage();
  if (path) await openPath(path);
});

btnOpenFolder.addEventListener("click", async () => {
  const dir = await pickFolder();
  if (dir) await openDirectory(dir);
});

btnPrev.addEventListener("click", () => step(-1));
btnNext.addEventListener("click", () => step(1));
btnZoomIn.addEventListener("click", () => zoomAtCenter(ZOOM_STEP));
btnZoomOut.addEventListener("click", () => zoomAtCenter(1 / ZOOM_STEP));
btnFit.addEventListener("click", zoomToFit);
zoomLabel.addEventListener("click", zoomToFit);
btnActual.addEventListener("click", zoomToActual);
btnRotateLeft.addEventListener("click", () => rotateBy(-90));
btnRotateRight.addEventListener("click", () => rotateBy(90));
btnFlipH.addEventListener("click", toggleFlipH);
btnFlipV.addEventListener("click", toggleFlipV);
btnResetTransform.addEventListener("click", resetTransform);
btnSlideshow.addEventListener("click", toggleSlideshow);
slideshowInterval.addEventListener("change", () => {
  if (slideshowTimer === null) return;
  // Re-arm in place: a stop/start cycle would drop and re-acquire fullscreen.
  window.clearInterval(slideshowTimer);
  const seconds = Number(slideshowInterval.value) || 3;
  slideshowTimer = window.setInterval(slideshowTick, seconds * 1000);
});

/* ---------------------------- editing / files ---------------------------- */

btnSaveAs.addEventListener("click", () => void saveAs());
btnSaveTransform.addEventListener("click", () => void saveTransformResult());
btnCrop.addEventListener("click", () => setCropMode(!cropActive));
btnCropApply.addEventListener("click", () => void applyCrop());
btnCropClear.addEventListener("click", () => setCropRect(null));
btnCropExit.addEventListener("click", () => setCropMode(false));

for (const input of [cropX, cropY, cropW, cropH]) {
  input.addEventListener("input", () => {
    syncCropOverlay();
    updateCropControls();
  });
}

btnRename.addEventListener("click", () => {
  if (currentPath) openRenameModal(currentPath);
});
btnRenameCancel.addEventListener("click", closeRenameModal);
btnRenameConfirm.addEventListener("click", () => void submitRename());
renameInput.addEventListener("keydown", (event) => {
  if (event.key === "Enter") {
    event.preventDefault();
    void submitRename();
  } else if (event.key === "Escape") {
    event.preventDefault();
    closeRenameModal();
  }
});
renameModal.addEventListener("pointerdown", (event) => {
  if (event.target === renameModal) closeRenameModal();
});

btnDelete.addEventListener("click", () => void removeCurrent());
btnCopyPath.addEventListener("click", () => void copyCurrentPath());

/* ------------------------------- pages ---------------------------------- */

btnPagePrev.addEventListener("click", () => void gotoPage((pageIndex ?? 0) - 1));
btnPageNext.addEventListener("click", () => void gotoPage((pageIndex ?? 0) + 1));
btnPageLive.addEventListener("click", () => void showLiveFrame());

btnThumbs.addEventListener("click", toggleThumbsVisible);
btnInfo.addEventListener("click", () => setInfoVisible(!infoVisible));
btnTheme.addEventListener("click", () => {
  applyTheme(document.documentElement.dataset.theme === "light" ? "dark" : "light");
});
btnFullscreen.addEventListener("click", () => void toggleFullscreen());

document.addEventListener("fullscreenchange", () => {
  const on = document.fullscreenElement !== null;
  document.body.classList.toggle("is-fullscreen", on);
  // Icon-only button: expand when windowed, compress while fullscreen.
  btnFullscreen.dataset.state = on ? "on" : "off";
  btnFullscreen.title = on ? "退出全屏 (F11)" : "全屏 (F11)";
});

thumbsTrack.addEventListener("click", (event) => {
  btnSlideshow.title = playing ? "暂停幻灯片 (空格)" : "播放幻灯片 (空格)";
}

/** One slideshow beat: the flag asks `openPath` for a crossfade. */
function slideshowTick(): void {
  slideAdvance = true;
  step(1, true);
  const button = (event.target as HTMLElement).closest<HTMLElement>(".thumb");
  if (!button || !listing) return;
  const entry = listing.entries[Number(button.dataset.index)];
  if (entry) void openPath(entry.path);
});

// Wheel zoom, anchored at the cursor.
  stage.classList.remove("slideshow");
  clearGhost();
  // Hand the filmstrip back if playback is what tucked it away.
  if (thumbsHiddenBySlideshow) {
    thumbsHiddenBySlideshow = false;
    setThumbsVisible(true);
  }
  // Only give up fullscreen if playback is what took it.
  if (fullscreenEnteredBySlideshow) {
    fullscreenEnteredBySlideshow = false;
    try {
      if (document.fullscreenElement) await document.exitFullscreen();
    } catch {
      /* already out — nothing to restore */
    }
  }
stage.addEventListener(
  "wheel",
  (event) => {
    if (!meta) return;
    event.preventDefault();
    const rect = stage.getBoundingClientRect();
    const factor = event.deltaY < 0 ? WHEEL_STEP : 1 / WHEEL_STEP;
    zoomAt(factor, event.clientX - rect.left, event.clientY - rect.top);
  },
  { passive: false },
);

stage.addEventListener("dblclick", () => {
  if (cropActive) return;
  void toggleFullscreen();
});

// Drag to pan — or, in crop mode, drag out a selection rectangle.
let dragging = false;
let lastX = 0;
let lastY = 0;
let selectStart: { x: number; y: number } | null = null;

function stagePoint(event: PointerEvent): { x: number; y: number } {
  const rect = stage.getBoundingClientRect();
  return { x: event.clientX - rect.left, y: event.clientY - rect.top };
}

stage.addEventListener("pointerdown", (event) => {
  if (!meta || event.button !== 0) return;
  stage.setPointerCapture(event.pointerId);
  if (cropActive) {
    selectStart = stagePoint(event);
    setCropRect(null);
    return;
  }
  dragging = true;
/** User-initiated toggle — reclaims the panel from the slideshow's control. */
function toggleThumbsVisible(): void {
  thumbsHiddenBySlideshow = false;
  setThumbsVisible(!thumbsVisible);
}

  lastX = event.clientX;
  lastY = event.clientY;
  stage.classList.add("grabbing");
});

stage.addEventListener("pointermove", (event) => {
  if (selectStart) {
    // Ignore degenerate rects so the box does not flicker mid-drag.
    const rect = cropRectFromStage(selectStart, stagePoint(event));
    if (rect) setCropRect(rect);
    return;
  }
  if (!dragging) return;
  tx += event.clientX - lastX;
  ty += event.clientY - lastY;
  lastX = event.clientX;
  lastY = event.clientY;
  applyTransform();
});

function endDrag(event: PointerEvent): void {
  if (stage.hasPointerCapture(event.pointerId)) {
    stage.releasePointerCapture(event.pointerId);
  }
  if (selectStart) {
    selectStart = null;
    return;
  }
  if (!dragging) return;
  dragging = false;
  stage.classList.remove("grabbing");
}

stage.addEventListener("pointerup", endDrag);
stage.addEventListener("pointercancel", endDrag);

// Keep the image framed when the window resizes. A drag or a maximize/restore
// animation fires `resize` far faster than the compositor can consume, and each
// re-fit reads layout, so coalesce to at most one update per animation frame —
// running it per event is what made resizing stutter.
let resizeFrame = 0;
window.addEventListener("resize", () => {
  if (resizeFrame !== 0) return;
  resizeFrame = requestAnimationFrame(() => {
    resizeFrame = 0;
    // The toolbar reflows on resize, so a pinned popover needs re-anchoring.
    if (menuOpen) positionMenu();
    if (!meta) return;
    if (fitActive) zoomToFit();
    else if (cropActive) syncCropOverlay();
  });
});

/* -------------------------------------------------------------------------- */
/* Keyboard shortcuts (ImageGlass-style)                                       */
/* -------------------------------------------------------------------------- */

function isEditable(target: EventTarget | null): boolean {
  const node = target as HTMLElement | null;
  if (!node) return false;
  return (
    node.tagName === "INPUT" ||
    node.tagName === "SELECT" ||
    node.tagName === "TEXTAREA" ||
    node.isContentEditable
  );
}

window.addEventListener("keydown", (event) => {
  // The rename modal owns the keyboard while it is open.
  if (renaming) {
    if (event.key === "Escape") {
      event.preventDefault();
      closeRenameModal();
    }
    return;
  }
  if (event.ctrlKey || event.metaKey) {
    const key = event.key.toLowerCase();
    if (key === "o") {
      event.preventDefault();
      btnOpen.click();
    } else if (key === "s") {
      event.preventDefault();
      void saveAs();
    }
    return;
  }
  if (event.altKey) return;
  if (isEditable(event.target) && event.key !== "Escape") return;
  // Let a focused button handle Enter/Space itself (default button activation).
  const onButton = (event.target as HTMLElement | null)?.tagName === "BUTTON";

  switch (event.key) {
    case "ArrowUp":
    case "ArrowLeft":
    case "PageUp":
    case "Backspace":
      event.preventDefault();
      step(-1);
      break;

    case "ArrowDown":
    case "ArrowRight":
    case "PageDown":
      event.preventDefault();
      step(1);
      break;
    case "Enter":
      if (onButton) break;
      event.preventDefault();
      step(1);
      break;

    case "Home":
      event.preventDefault();
      jumpTo(0);
      break;
    case "End":
      event.preventDefault();
      jumpTo(-1);
      break;

    case "+":
    case "=":
      event.preventDefault();
      zoomAtCenter(ZOOM_STEP);
      break;
    case "-":
    case "_":
      event.preventDefault();
      zoomAtCenter(1 / ZOOM_STEP);
      break;

    case "f":
    case "F":
    case "0":
      event.preventDefault();
      zoomToFit();
      break;
    case "1":
      event.preventDefault();
      zoomToActual();
      break;
    case ")":
      event.preventDefault();
      resetTransform();
      break;

    case "r":
      rotateBy(90);
      break;
    case "R":
      rotateBy(-90);
/* ------------------------------- more menu ------------------------------- */

let menuOpen = false;

/**
 * Drop the popover under its trigger. The toolbar is centered, so the trigger
 * is nowhere near a fixed edge — measure instead of relying on a CSS anchor.
 * Clamped so the panel never spills past the bar.
 */
function positionMenu(): void {
  const bar = toolbar.getBoundingClientRect();
  const btn = btnMore.getBoundingClientRect();
  const maxLeft = bar.width - moreMenu.offsetWidth - 8;
  moreMenu.style.left = `${Math.max(8, Math.min(btn.left - bar.left, maxLeft))}px`;
}

function setMenuOpen(open: boolean): void {
  menuOpen = open;
  moreMenu.hidden = !open;
  btnMore.setAttribute("aria-expanded", open ? "true" : "false");
  // Measured after the panel is visible, or its width reads as zero.
  if (open) positionMenu();
}

btnMore.addEventListener("click", () => setMenuOpen(!menuOpen));

// Dismiss on a click outside. `pointerdown` runs before the button's own click,
// so clicking the trigger never double-toggles.
document.addEventListener("pointerdown", (event) => {
  if (!menuOpen) return;
  const node = event.target as HTMLElement;
  if (node.closest("#more-menu") || node.closest("#btn-more")) return;
  setMenuOpen(false);
});

// Acting on an item closes the popover; the two <select> rows stay open so the
// user can change a value without the menu vanishing under the pointer.
moreMenu.addEventListener("click", (event) => {
  const node = event.target as HTMLElement;
  if (node.closest("select")) return;
  if (node.closest("button")) setMenuOpen(false);
});

      break;
    case "l":
    case "L":
      rotateBy(-90);
      break;
    case "h":
    case "H":
      toggleFlipH();
      break;
    case "v":
    case "V":
      toggleFlipV();
      break;

    case "t":
    case "T":
    case "b":
    case "B":
      toggleThumbsVisible();
      break;
    case "i":
    case "I":
      setInfoVisible(!infoVisible);
      break;

    case "c":
    case "C":
      if (meta && !isMultiPage()) setCropMode(!cropActive);
      break;

    case " ":
      if (onButton) break;
      event.preventDefault();
      toggleSlideshow();
      break;

    case "F11":
      event.preventDefault();
      void toggleFullscreen();
      break;

    case "Escape":
      if (menuOpen) setMenuOpen(false);
      else if (document.fullscreenElement) void document.exitFullscreen();
      else if (cropActive) setCropMode(false);
      else if (slideshowTimer !== null) void stopSlideshow();
      break;

    default:
      break;
  }
});

/* -------------------------------------------------------------------------- */
/* Init                                                                        */
/* -------------------------------------------------------------------------- */

initTheme();
setThumbsVisible(true);

// Warm the `lumina://` base URL so the first open does not pay for it.
void protocolBase();
setInfoVisible(false);
updateControlsEnabled();
updateNavigation();
updateStatus();
updateSlideshowButton();
updatePageControls();
wireDragAndDrop();
    } else if (key === "c" && event.shiftKey) {
      // Matches "copy as path" in Explorer / Finder muscle memory.
      event.preventDefault();
      void copyCurrentPath();
    case "Delete":
      event.preventDefault();
      void removeCurrent();
      break;

    case "F2":
      event.preventDefault();
      if (currentPath) openRenameModal(currentPath);
      break;

void onDirectoryChanged(handleDirectoryChanged);
