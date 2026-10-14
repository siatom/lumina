/**
 * Lumina — minimal vertical slice.
 *
 * Flow: open dialog → `get_image_metadata` → stream the image through the
 * `lumina://` protocol → render with basic zoom/pan → prev/next via
 * `list_images` → lazy thumbnails via `get_thumbnail`.
 *
 * All IPC goes through `./api`, which is framework-agnostic: this file could be
 * swapped for a Svelte/Vue layer without touching the Rust contract.
 */

import {
  getImageBlob,
  getImageMetadata,
  getThumbnail,
  listImages,
  luminaImageUrl,
  pickImage,
  type DirectoryListing,
  type ImageMetadata,
} from "./api";

/* -------------------------------------------------------------------------- */
/* DOM                                                                         */
/* -------------------------------------------------------------------------- */

const stage = document.getElementById("stage") as HTMLElement;
const viewer = document.getElementById("viewer") as HTMLImageElement;
const emptyHint = document.getElementById("empty-hint") as HTMLElement;
const filmstrip = document.getElementById("filmstrip") as HTMLElement;
const infoPanel = document.getElementById("info-panel") as HTMLElement;
const infoList = document.getElementById("info-list") as HTMLElement;
const zoomLabel = document.getElementById("zoom-label") as HTMLElement;
const fileLabel = document.getElementById("file-label") as HTMLElement;

const btnOpen = document.getElementById("btn-open") as HTMLButtonElement;
const btnPrev = document.getElementById("btn-prev") as HTMLButtonElement;
const btnNext = document.getElementById("btn-next") as HTMLButtonElement;
const btnZoomIn = document.getElementById("btn-zoom-in") as HTMLButtonElement;
const btnZoomOut = document.getElementById("btn-zoom-out") as HTMLButtonElement;
const btnFit = document.getElementById("btn-fit") as HTMLButtonElement;
const btnActual = document.getElementById("btn-actual") as HTMLButtonElement;
const btnInfo = document.getElementById("btn-info") as HTMLButtonElement;

/* -------------------------------------------------------------------------- */
/* State                                                                       */
/* -------------------------------------------------------------------------- */

const MIN_SCALE = 0.05;
const MAX_SCALE = 32;

let meta: ImageMetadata | null = null;
let listing: DirectoryListing | null = null;
let currentPath: string | null = null;

/** View transform: image point (px,py) maps to stage point (tx + s*px, ty + s*py). */
let scale = 1;
let tx = 0;
let ty = 0;

/** Guards the protocol → blob fallback from looping on repeated errors. */
let viewerFellBack = false;

/* -------------------------------------------------------------------------- */
/* View transform                                                              */
/* -------------------------------------------------------------------------- */

function clampScale(value: number): number {
  return Math.min(MAX_SCALE, Math.max(MIN_SCALE, value));
}

function applyTransform(): void {
  viewer.style.transform = `translate(${tx}px, ${ty}px) scale(${scale})`;
  zoomLabel.textContent = `${Math.round(scale * 100)}%`;
}

/** Scale that fits the whole image inside the stage, never upscaling past 1. */
function fitScale(): number {
  if (!meta) return 1;
  const pad = 32;
  const { width, height } = stage.getBoundingClientRect();
  const s = Math.min((width - pad) / meta.width, (height - pad) / meta.height);
  return clampScale(Math.min(s, 1));
}

function zoomToFit(): void {
  if (!meta) return;
  scale = fitScale();
  centerImage();
}

function zoomToActual(): void {
  if (!meta) return;
  scale = 1;
  centerImage();
}

function centerImage(): void {
  if (!meta) return;
  const { width, height } = stage.getBoundingClientRect();
  tx = (width - scale * meta.width) / 2;
  ty = (height - scale * meta.height) / 2;
  applyTransform();
}

/** Zoom by `factor`, keeping the stage-relative point (sx, sy) fixed. */
function zoomAt(factor: number, sx: number, sy: number): void {
  const next = clampScale(scale * factor);
  const k = next / scale;
  tx = sx - k * (sx - tx);
  ty = sy - k * (sy - ty);
  scale = next;
  applyTransform();
}

function zoomAtCenter(factor: number): void {
  const { width, height } = stage.getBoundingClientRect();
  zoomAt(factor, width / 2, height / 2);
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

function renderInfo(m: ImageMetadata): void {
  infoList.replaceChildren();
  addInfoRow("文件名", m.file_name);
  addInfoRow("格式", m.format.toUpperCase());
  addInfoRow("尺寸", `${m.width} × ${m.height}`);
  addInfoRow("文件大小", formatBytes(m.file_size));

  const e = m.exif;
  if (e) {
    addInfoRow("相机", [e.make, e.model].filter(Boolean).join(" ") || null);
    addInfoRow("拍摄时间", e.datetime);
    addInfoRow("曝光时间", e.exposure_time);
    addInfoRow("光圈", e.f_number);
    addInfoRow("ISO", e.iso);
    addInfoRow("焦距", e.focal_length);
    addInfoRow("方向", e.orientation != null ? String(e.orientation) : null);
  }
  addInfoRow("路径", m.path);
}

/* -------------------------------------------------------------------------- */
/* Filmstrip (lazy thumbnails)                                                 */
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
  { root: filmstrip, rootMargin: "300px" },
);

async function loadThumbnail(img: HTMLImageElement): Promise<void> {
  const path = img.dataset.path;
  if (!path) return;
  try {
    const blob = await getThumbnail(path, 128);
    const url = URL.createObjectURL(blob);
    img.addEventListener("load", () => URL.revokeObjectURL(url), { once: true });
    img.src = url;
  } catch {
    img.classList.add("thumb-failed");
  }
}

function renderFilmstrip(list: DirectoryListing, active: string): void {
  filmstrip.replaceChildren();
  const frag = document.createDocumentFragment();

  list.entries.forEach((entry, index) => {
    const button = document.createElement("button");
    button.type = "button";
    button.className = "thumb";
    button.dataset.index = String(index);
    button.title = entry.file_name;
    button.classList.toggle("active", entry.path === active);

    const img = document.createElement("img");
    img.dataset.path = entry.path;
    img.alt = entry.file_name;
    img.draggable = false;

    button.appendChild(img);
    frag.appendChild(button);
  });

  filmstrip.appendChild(frag);
  for (const img of filmstrip.querySelectorAll<HTMLImageElement>("img")) {
    thumbObserver.observe(img);
  }
  filmstrip.querySelector(".thumb.active")?.scrollIntoView({ block: "nearest", inline: "center" });
}

/* -------------------------------------------------------------------------- */
/* Loading                                                                     */
/* -------------------------------------------------------------------------- */

async function openPath(path: string): Promise<void> {
  let m: ImageMetadata;
  try {
    m = await getImageMetadata(path);
  } catch (err) {
    setStatus(`无法打开：${String(err)}`);
    return;
  }

  meta = m;
  currentPath = m.path;
  viewerFellBack = false;

  fileLabel.textContent = m.file_name;
  emptyHint.hidden = true;
  viewer.classList.add("visible");
  renderInfo(m);

  // Point the viewer at the image. Fast path: lumina:// protocol streaming.
  viewer.src = await luminaImageUrl(m.path, { mode: "full" });

  // The image dimensions drive the fit, so no need to wait for `load`.
  zoomToFit();

  // Refresh the sibling listing for the directory we are browsing.
  const dir = parentDir(m.path);
  try {
    listing = await listImages(dir, m.path);
  } catch {
    listing = null;
  }
  updateNavigation();

  if (listing && listing.entries.length > 0) {
    renderFilmstrip(listing, m.path);
  } else {
    filmstrip.replaceChildren();
  }
}

function setStatus(text: string): void {
  emptyHint.hidden = false;
  emptyHint.textContent = text;
  viewer.classList.remove("visible");
}

function updateNavigation(): void {
  const enabled = listing !== null && listing.entries.length > 1;
  btnPrev.disabled = !enabled;
  btnNext.disabled = !enabled;
}

function step(delta: number): void {
  if (!listing || !currentPath) return;
  const index = listing.entries.findIndex((e) => e.path === currentPath);
  if (index < 0) return;
  const next = index + delta;
  if (next < 0 || next >= listing.entries.length) return;
  void openPath(listing.entries[next].path);
}

/* -------------------------------------------------------------------------- */
/* Events                                                                      */
/* -------------------------------------------------------------------------- */

viewer.addEventListener("error", async () => {
  // Fallback transport: pull the bytes over IPC and use a blob URL.
  if (viewerFellBack || !currentPath) return;
  viewerFellBack = true;
  try {
    const blob = await getImageBlob(currentPath);
    viewer.src = URL.createObjectURL(blob);
  } catch (err) {
    setStatus(`无法显示图片：${String(err)}`);
  }
});

btnOpen.addEventListener("click", async () => {
  const path = await pickImage();
  if (path) await openPath(path);
});

btnPrev.addEventListener("click", () => step(-1));
btnNext.addEventListener("click", () => step(1));
btnZoomIn.addEventListener("click", () => zoomAtCenter(1.25));
btnZoomOut.addEventListener("click", () => zoomAtCenter(1 / 1.25));
btnFit.addEventListener("click", zoomToFit);
btnActual.addEventListener("click", zoomToActual);
btnInfo.addEventListener("click", () => {
  infoPanel.hidden = !infoPanel.hidden;
});

filmstrip.addEventListener("click", (event) => {
  const button = (event.target as HTMLElement).closest<HTMLElement>(".thumb");
  if (!button || !listing) return;
  const index = Number(button.dataset.index);
  const entry = listing.entries[index];
  if (entry) void openPath(entry.path);
});

// Wheel zoom, anchored at the cursor.
stage.addEventListener(
  "wheel",
  (event) => {
    if (!meta) return;
    event.preventDefault();
    const rect = stage.getBoundingClientRect();
    const factor = event.deltaY < 0 ? 1.1 : 1 / 1.1;
    zoomAt(factor, event.clientX - rect.left, event.clientY - rect.top);
  },
  { passive: false },
);

// Drag to pan.
let dragging = false;
let lastX = 0;
let lastY = 0;

stage.addEventListener("pointerdown", (event) => {
  if (!meta) return;
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

// Keep the image centered when the window is resized in fit mode.
window.addEventListener("resize", () => {
  if (!meta) return;
  if (Math.abs(scale - fitScale()) < 0.001) zoomToFit();
});

// Keyboard shortcuts.
window.addEventListener("keydown", (event) => {
  if (event.ctrlKey || event.metaKey) {
    if (event.key.toLowerCase() === "o") {
      event.preventDefault();
      btnOpen.click();
    }
    return;
  }
  switch (event.key) {
    case "ArrowLeft":
      step(-1);
      break;
    case "ArrowRight":
      step(1);
      break;
    case "+":
    case "=":
      zoomAtCenter(1.25);
      break;
    case "-":
      zoomAtCenter(1 / 1.25);
      break;
    case "0":
      zoomToFit();
      break;
    case "1":
      zoomToActual();
      break;
    case "i":
      infoPanel.hidden = !infoPanel.hidden;
      break;
    default:
      break;
  }
});
