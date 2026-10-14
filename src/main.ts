/**
 * Lumina — viewer UI.
 *
 * The Rust contract lives behind `./api` (see docs/INTERFACES.md); this file is
 * the whole frontend. It layers a full ImageGlass-style shell on top of the
 * stage-1 vertical slice — zoom/pan, fit/1:1, prev/next, thumbnails — and adds
 * a toolbar, a thumbnail sidebar, a status bar, slideshow, fullscreen, a
 * dark/light theme and keyboard shortcuts.
 *
 * Transport / performance: the original image is streamed through the
 * `lumina://` protocol (`luminaImageUrl`) and falls back to `getImageBlob` on
 * `error`. Thumbnails are fetched lazily, one downscaled PNG per visible tile,
 * and every object URL is revoked on load/error so the webview's memory does
 * not grow with the directory size.
 */

import {
  getImageBlob,
  getImageMetadata,
  getThumbnail,
  listImages,
  luminaImageUrl,
  pickFolder,
  pickImage,
  type DirectoryListing,
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

const stage = el<HTMLElement>("stage");
const viewer = el<HTMLImageElement>("viewer");
const emptyHint = el<HTMLElement>("empty-hint");
const toast = el<HTMLElement>("toast");

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
const btnThumbs = el<HTMLButtonElement>("btn-thumbs");
const btnInfo = el<HTMLButtonElement>("btn-info");
const btnTheme = el<HTMLButtonElement>("btn-theme");
const btnFullscreen = el<HTMLButtonElement>("btn-fullscreen");

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
let thumbsVisible = true;
let infoVisible = false;

/** Monotonic token so a slow open cannot overwrite a newer one. */
let openToken = 0;

/* -------------------------------------------------------------------------- */
/* View transform                                                              */
/* -------------------------------------------------------------------------- */

function clampScale(value: number): number {
  return Math.min(MAX_SCALE, Math.max(MIN_SCALE, value));
}

/** On-screen (post-rotation) dimensions of the image, in image pixels. */
function viewDims(m: ImageMetadata): { w: number; h: number } {
  const swapped = rotation % 180 !== 0;
  return swapped ? { w: m.height, h: m.width } : { w: m.width, h: m.height };
}

/**
 * Compose the CSS transform for the current view.
 *
 * Model: an image point `p` maps to `t + s·(c + R(θ)·F·(p − c))`, where `c` is
 * the image center, `F` the flip signs and `R` the rotation — i.e. zoom and
 * flips scale about the center and rotation spins about the center. CSS applies
 * right-to-left, so `translate(O) rotate(θ) scale(s·fh, s·fv)` realises it with
 * `O = t + s·c − R·F·(s·c)`.
 */
function applyTransform(): void {
  if (meta) {
    const s = scale;
    const cx = meta.width / 2;
    const cy = meta.height / 2;
    const fh = flipH ? -1 : 1;
    const fv = flipV ? -1 : 1;
    const rad = (rotation * Math.PI) / 180;
    const cos = Math.cos(rad);
    const sin = Math.sin(rad);
    // R(θ) · (s·F·c)
    const a = s * fh * cx;
    const b = s * fv * cy;
    const rotatedX = a * cos - b * sin;
    const rotatedY = a * sin + b * cos;
    const ox = tx + s * cx - rotatedX;
    const oy = ty + s * cy - rotatedY;
    viewer.style.transform =
      `translate(${ox}px, ${oy}px) rotate(${rotation}deg) scale(${s * fh}, ${s * fv})`;
  }
  const label = `${Math.round(scale * 100)}%`;
  zoomLabel.textContent = label;
  statusZoom.textContent = label;
}

/** Scale that fits the whole (rotated) image inside the stage, never upscaling. */
function fitScale(): number {
  if (!meta) return 1;
  const pad = 32;
  const rect = stage.getBoundingClientRect();
  const { w, h } = viewDims(meta);
  const s = Math.min((rect.width - pad) / w, (rect.height - pad) / h);
  return clampScale(Math.min(s, 1));
}

function centerImage(): void {
  if (!meta) return;
  const rect = stage.getBoundingClientRect();
  tx = (rect.width - scale * meta.width) / 2;
  ty = (rect.height - scale * meta.height) / 2;
  applyTransform();
}

function zoomToFit(): void {
  if (!meta) return;
  scale = fitScale();
  fitActive = true;
  centerImage();
}

function zoomToActual(): void {
  if (!meta) return;
  scale = 1;
  fitActive = false;
  centerImage();
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
}

function zoomAtCenter(factor: number): void {
  const rect = stage.getBoundingClientRect();
  zoomAt(factor, rect.width / 2, rect.height / 2);
}

/** Re-fit or re-center after a rotation/flip changes the framing. */
function reflow(): void {
  if (fitActive) zoomToFit();
  else centerImage();
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
  emptyHint.textContent = text;
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
    btnResetTransform,
    btnSlideshow,
    zoomLabel,
  ]) {
    button.disabled = !on;
  }
}

/* -------------------------------------------------------------------------- */
/* Info panel                                                                  */
/* -------------------------------------------------------------------------- */

function addInfoRow(label: string, value: string | null | undefined): void {
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

function renderInfo(m: ImageMetadata): void {
  infoList.replaceChildren();
  addInfoRow("文件名", m.file_name);
  addInfoRow("格式", m.format.toUpperCase());
  addInfoRow("尺寸", `${m.width} × ${m.height}`);
  addInfoRow("文件大小", formatBytes(m.file_size));
  addInfoRow("路径", m.path);

  const e = m.exif;
  if (e) {
    addInfoSub("EXIF");
    addInfoRow("相机", [e.make, e.model].filter(Boolean).join(" ") || null);
    addInfoRow("拍摄时间", e.datetime);
    addInfoRow("曝光时间", e.exposure_time);
    addInfoRow("光圈", e.f_number);
    addInfoRow("ISO", e.iso);
    addInfoRow("焦距", e.focal_length);
    addInfoRow("方向", e.orientation != null ? String(e.orientation) : null);
  }
}

/* -------------------------------------------------------------------------- */
/* Thumbnail sidebar (lazy, one blob per visible tile)                         */
/* -------------------------------------------------------------------------- */

const thumbObserver = new IntersectionObserver(
  (entries) => {
    for (const entry of entries) {
      if (!entry.isIntersecting) continue;
      const img = entry.target as HTMLImageElement;
      thumbObserver.unobserve(img);
      void loadThumbnail(img);
    }
  },
  { root: thumbsTrack, rootMargin: "240px" },
);

async function loadThumbnail(img: HTMLImageElement): Promise<void> {
  const path = img.dataset.path;
  if (!path) return;
  try {
    const blob = await getThumbnail(path, 192);
    const url = URL.createObjectURL(blob);
    img.addEventListener("load", () => URL.revokeObjectURL(url), { once: true });
    img.addEventListener("error", () => URL.revokeObjectURL(url), { once: true });
    img.src = url;
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

  meta = m;
  currentPath = m.path;
  viewerFellBack = false;
  releaseViewerBlob();
  rotation = 0;
  flipH = false;
  flipV = false;

  emptyHint.hidden = true;
  viewer.classList.add("visible");
  renderInfo(m);
  updateStatus();
  updateControlsEnabled();

  const url = await luminaImageUrl(m.path, { mode: "full" });
  if (token !== openToken) return;
  viewer.src = url;

  // Fit uses the metadata dimensions, so it does not wait for `load`.
  zoomToFit();

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
/* Slideshow                                                                   */
/* -------------------------------------------------------------------------- */

let slideshowTimer: number | null = null;

function updateSlideshowButton(): void {
  const playing = slideshowTimer !== null;
  btnSlideshow.textContent = playing ? "⏸ 暂停" : "▶ 幻灯片";
  btnSlideshow.setAttribute("aria-pressed", playing ? "true" : "false");
}

function stopSlideshow(): void {
  if (slideshowTimer !== null) {
    window.clearInterval(slideshowTimer);
    slideshowTimer = null;
  }
  updateSlideshowButton();
}

function startSlideshow(): void {
  if (!listing || listing.entries.length < 2) {
    showToast("幻灯片需要当前目录中至少两张图片");
    return;
  }
  const seconds = Number(slideshowInterval.value) || 3;
  slideshowTimer = window.setInterval(() => step(1, true), seconds * 1000);
  updateSlideshowButton();
}

function toggleSlideshow(): void {
  if (slideshowTimer !== null) stopSlideshow();
  else startSlideshow();
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
  btnTheme.textContent = theme === "dark" ? "浅色" : "深色";
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
  // Fallback transport: pull the bytes over IPC and use a blob URL.
  if (viewerFellBack || !currentPath) return;
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
  if (slideshowTimer !== null) {
    stopSlideshow();
    startSlideshow();
  }
});

btnThumbs.addEventListener("click", () => setThumbsVisible(!thumbsVisible));
btnInfo.addEventListener("click", () => setInfoVisible(!infoVisible));
btnTheme.addEventListener("click", () => {
  applyTheme(document.documentElement.dataset.theme === "light" ? "dark" : "light");
});
btnFullscreen.addEventListener("click", () => void toggleFullscreen());

document.addEventListener("fullscreenchange", () => {
  const on = document.fullscreenElement !== null;
  document.body.classList.toggle("is-fullscreen", on);
  btnFullscreen.textContent = on ? "退出全屏" : "全屏";
});

thumbsTrack.addEventListener("click", (event) => {
  const button = (event.target as HTMLElement).closest<HTMLElement>(".thumb");
  if (!button || !listing) return;
  const entry = listing.entries[Number(button.dataset.index)];
  if (entry) void openPath(entry.path);
});

// Wheel zoom, anchored at the cursor.
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

stage.addEventListener("dblclick", () => void toggleFullscreen());

// Drag to pan.
let dragging = false;
let lastX = 0;
let lastY = 0;

stage.addEventListener("pointerdown", (event) => {
  if (!meta || event.button !== 0) return;
  dragging = true;
  lastX = event.clientX;
  lastY = event.clientY;
  stage.setPointerCapture(event.pointerId);
  stage.classList.add("grabbing");
});

stage.addEventListener("pointermove", (event) => {
  if (!dragging) return;
  tx += event.clientX - lastX;
  ty += event.clientY - lastY;
  lastX = event.clientX;
  lastY = event.clientY;
  applyTransform();
});

function endDrag(event: PointerEvent): void {
  if (!dragging) return;
  dragging = false;
  stage.classList.remove("grabbing");
  if (stage.hasPointerCapture(event.pointerId)) {
    stage.releasePointerCapture(event.pointerId);
  }
}

stage.addEventListener("pointerup", endDrag);
stage.addEventListener("pointercancel", endDrag);

// Keep the image framed when the window resizes.
window.addEventListener("resize", () => {
  if (meta && fitActive) zoomToFit();
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
  if (event.ctrlKey || event.metaKey) {
    if (event.key.toLowerCase() === "o") {
      event.preventDefault();
      btnOpen.click();
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
      setThumbsVisible(!thumbsVisible);
      break;
    case "i":
    case "I":
      setInfoVisible(!infoVisible);
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
      if (document.fullscreenElement) void document.exitFullscreen();
      else if (slideshowTimer !== null) stopSlideshow();
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
setInfoVisible(false);
updateControlsEnabled();
updateNavigation();
updateStatus();
updateSlideshowButton();
