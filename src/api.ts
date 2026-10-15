import { invoke } from "@tauri-apps/api/core";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { confirm, open as openDialog, save as saveDialog } from "@tauri-apps/plugin-dialog";

/* -------------------------------------------------------------------------- */
/* Types — mirrored 1:1 in docs/INTERFACES.md and in Rust (src-tauri/src).     */
/* -------------------------------------------------------------------------- */

export interface ExifData {
  orientation: number | null;
  make: string | null;
  model: string | null;
  /** Lens model, e.g. "EF 50mm f/1.8". */
  lens_model: string | null;
  datetime: string | null;
  software: string | null;
  artist: string | null;
  copyright: string | null;
  description: string | null;
  exposure_time: string | null;
  f_number: string | null;
  iso: string | null;
  focal_length: string | null;
  /** Signed decimal degrees (negative = south). */
  gps_latitude: number | null;
  /** Signed decimal degrees (negative = west). */
  gps_longitude: number | null;
  /** Metres above sea level. */
  gps_altitude: number | null;
}

export interface ImageMetadata {
  path: string;
  file_name: string;
  /** "png" | "jpeg" | "gif" | "webp" | "bmp" | "tiff" | "ico" | ... */
  format: string;
  width: number;
  height: number;
  /** Bytes. */
  file_size: number;
  /** Last-modified time in milliseconds since the Unix epoch (0 if unknown). */
  modified_ms: number;
  has_exif: boolean;
  exif: ExifData | null;
}

export interface ImageEntry {
  path: string;
  file_name: string;
  format: string;
  file_size: number;
  /** Last-modified time in milliseconds since the Unix epoch (0 if unknown). */
  modified_ms: number;
}

export interface DirectoryListing {
  dir: string;
  entries: ImageEntry[];
  index: number | null;
}

export type ImageMode = "full" | "thumb";

/** Extensions accepted by the open dialog (keep in sync with Rust). */
export const SUPPORTED_EXTENSIONS = [
  "png",
  "jpg",
  "jpeg",
  "gif",
  "webp",
  "bmp",
  "tif",
  "tiff",
  "ico",
  "svg",
] as const;

/** Formats that can be written by transform / save-as (keep in sync with Rust). */
export const ENCODABLE_EXTENSIONS = [
  "png",
  "jpg",
  "jpeg",
  "gif",
  "webp",
  "bmp",
  "tif",
  "tiff",
  "ico",
] as const;

/* -------------------------------------------------------------------------- */
/* MIME types                                                                  */
/* -------------------------------------------------------------------------- */

/**
 * Extensions the backend streams through **verbatim** (mirrors
 * `imaging::WEBVIEW_NATIVE_EXTENSIONS`). Everything else — currently TIFF — is
 * decoded and re-encoded to PNG before it reaches us, so its payload must not
 * be labelled with the source extension.
 */
const PASSTHROUGH_EXTENSIONS = new Set([
  "png",
  "jpg",
  "jpeg",
  "gif",
  "webp",
  "bmp",
  "ico",
  "svg",
]);

/** Mirrors `imaging::mime_for_ext` in `src-tauri/src/imaging.rs`. */
const MIME_BY_EXTENSION: Record<string, string> = {
  png: "image/png",
  jpg: "image/jpeg",
  jpeg: "image/jpeg",
  gif: "image/gif",
  webp: "image/webp",
  bmp: "image/bmp",
  ico: "image/x-icon",
  svg: "image/svg+xml",
};

/** Lower-cased extension of a path, or "" when it has none. */
function extensionOf(path: string): string {
  const dot = path.lastIndexOf(".");
  if (dot < 0) return "";
  const ext = path.slice(dot + 1);
  // A dot in a directory name (`C:\my.pics\photo`) must not count.
  if (ext.includes("/") || ext.includes("\\")) return "";
  return ext.toLowerCase();
}

/**
 * MIME type of the bytes {@link getImageBlob} receives for `path`.
 *
 * Passthrough formats keep their own type; transcoded ones (TIFF) become PNG.
 * Getting this right is not cosmetic — an SVG served as `image/png` will not
 * render in an `<img>`.
 */
export function imageMimeType(path: string): string {
  const ext = extensionOf(path);
  if (!PASSTHROUGH_EXTENSIONS.has(ext)) return "image/png";
  return MIME_BY_EXTENSION[ext] ?? "image/png";
}

/* -------------------------------------------------------------------------- */
/* Commands                                                                    */
/* -------------------------------------------------------------------------- */

/** Dimensions, format, size and basic EXIF for one image. */
export function getImageMetadata(path: string): Promise<ImageMetadata> {
  return invoke<ImageMetadata>("get_image_metadata", { path });
}

/**
 * Sibling images in `dir`, naturally sorted, plus the index of `current`.
 * `current` is optional so a directory can be listed without an open file.
 */
export function listImages(dir: string, current?: string): Promise<DirectoryListing> {
  return invoke<DirectoryListing>("list_images", { dir, current: current ?? null });
}

/**
 * Watch `dir` for image changes, replacing any directory watched before.
 *
 * Nothing is reported back inline — changes arrive through
 * {@link onDirectoryChanged}. Re-watching the directory already watched is a
 * no-op, so this is safe to call on every open.
 */
export function watchDirectory(dir: string): Promise<void> {
  return invoke<void>("watch_directory", { dir });
}

/** Stop watching the browsed directory. Safe when nothing is watched. */
export function unwatchDirectory(): Promise<void> {
  return invoke<void>("unwatch_directory");
}

/**
 * Downscaled PNG thumbnail as a ready-to-use Blob.
 *
 * `image/png` is not an assumption: `imaging::make_thumbnail` always encodes
 * PNG regardless of the source format.
 */
export async function getThumbnail(path: string, maxSize = 256): Promise<Blob> {
  const bytes = await invoke<ArrayBuffer>("get_thumbnail", { path, maxSize });
  return new Blob([bytes], { type: "image/png" });
}

/**
 * Original image bytes as a Blob. This is the fallback transport used when the
 * `lumina://` protocol is not available; prefer {@link luminaImageUrl}.
 *
 * The payload keeps its real format for webview-native extensions (JPEG stays
 * JPEG, SVG stays SVG), so the Blob type is derived from the path rather than
 * hard-coded — see {@link imageMimeType}.
 */
export async function getImageBlob(path: string): Promise<Blob> {
  const bytes = await invoke<ArrayBuffer>("get_image_bytes", { path });
  return new Blob([bytes], { type: imageMimeType(path) });
}

/**
 * Save/convert an image. `format` is optional — when omitted the output format
 * follows the output file extension. `quality` (1..=100) only affects JPEG.
 */
export function saveImageAs(
  inputPath: string,
  outputPath: string,
  format?: string,
  quality?: number,
): Promise<string> {
  return invoke<string>("save_image_as", {
    inputPath,
    outputPath,
    format: format ?? null,
    quality: quality ?? null,
  });
}

/** Rectangle in *source* pixel coordinates. */
export interface CropRect {
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface ResizeSpec {
  width?: number;
  height?: number;
  /** true => exactly width×height (may distort); default fits inside. */
  exact?: boolean;
  filter?: "nearest" | "triangle" | "catmullrom" | "gaussian" | "lanczos3";
}

/**
 * Declarative edit applied by {@link transformImage}. Operations run in a
 * fixed order: **crop → flip → rotate → resize → encode**.
 */
export interface TransformOptions {
  /** 0 | 90 | 180 | 270, clockwise. */
  rotate?: number;
  flipHorizontal?: boolean;
  flipVertical?: boolean;
  crop?: CropRect;
  resize?: ResizeSpec;
  /** Target format name; defaults to the output path extension. */
  format?: string;
  /** 1..=100, JPEG only (WebP/PNG are lossless). */
  quality?: number;
}

/** Apply crop/flip/rotate/resize and write the result to `outputPath`. */
export function transformImage(
  inputPath: string,
  outputPath: string,
  options: TransformOptions,
): Promise<string> {
  return invoke<string>("transform_image", { inputPath, outputPath, options });
}

/** Number of pages / frames (GIF, animated WebP, multi-page TIFF); else 1. */
export function getImagePages(path: string): Promise<number> {
  return invoke<number>("get_image_pages", { path });
}

/**
 * One page / frame of an animated or multi-page file, as a PNG Blob.
 *
 * Always PNG: `imaging::read_frame_png` decodes the frame and re-encodes it,
 * whatever the container format was.
 */
export async function getImageFrame(path: string, index: number): Promise<Blob> {
  const bytes = await invoke<ArrayBuffer>("get_image_frame", { path, index });
  return new Blob([bytes], { type: "image/png" });
}

/** Rename a file in place; returns the new absolute path. */
export function renameFile(path: string, newName: string): Promise<string> {
  return invoke<string>("rename_file", { path, newName });
}

/**
 * Delete a single file. The backend refuses directories; ask the user to
 * confirm in the UI before calling this — it is not reversible.
 */
export function deleteFile(path: string): Promise<void> {
  return invoke<void>("delete_file", { path });
}

/** Copy a path to the system clipboard (for "copy path" / "copy image path"). */
export async function copyPathToClipboard(path: string): Promise<void> {
  await navigator.clipboard.writeText(path);
}

/* -------------------------------------------------------------------------- */
/* lumina:// transport                                                         */
/* -------------------------------------------------------------------------- */

let cachedBase: string | null = null;

/** Platform-correct base URL of the `lumina://` transport. */
export async function protocolBase(): Promise<string> {
  if (cachedBase === null) {
    cachedBase = await invoke<string>("get_protocol_base");
  }
  return cachedBase;
}

/** Image path supplied by the OS when Lumina was opened through file association. */
export function getLaunchPath(): Promise<string | null> {
  return invoke<string | null>("get_launch_path");
}

/**
 * Build a `lumina://` URL for an image.
 *
 * This is the preferred transport: the bytes are streamed by the webview
 * directly from Rust, with no base64 inflation and no JSON IPC hop.
 *
 * `version` (typically `<modified_ms>-<file_size>`) is echoed back as the `v`
 * query parameter. It serves two purposes: it lets the backend key its
 * thumbnail cache without a `stat`, and it gives the webview a distinct URL per
 * file revision so a cached entry is never served after the file changes.
 */
export async function luminaImageUrl(
  path: string,
  opts: { mode?: ImageMode; max?: number; version?: string | number } = {},
): Promise<string> {
  const base = await protocolBase();
  const q = new URLSearchParams({ path, mode: opts.mode ?? "full" });
  if (opts.max !== undefined) q.set("max", String(opts.max));
  if (opts.version !== undefined) q.set("v", String(opts.version));
  return `${base}/?${q.toString()}`;
}

/* -------------------------------------------------------------------------- */
/* Dialogs                                                                     */
/* -------------------------------------------------------------------------- */

/** Native "open image" dialog; returns the chosen absolute path or null. */
export async function pickImage(): Promise<string | null> {
  const selected = await openDialog({
    multiple: false,
    directory: false,
    filters: [{ name: "Images", extensions: [...SUPPORTED_EXTENSIONS] }],
  });
  return typeof selected === "string" ? selected : null;
}

/** Native "open folder" dialog; returns the chosen directory or null. */
export async function pickFolder(): Promise<string | null> {
  const selected = await openDialog({ directory: true, multiple: false });
  return typeof selected === "string" ? selected : null;
}

/** Native "save as" dialog restricted to the given extension. */
export async function pickSavePath(
  defaultPath: string,
  extension: string,
): Promise<string | null> {
  const selected = await saveDialog({
    defaultPath,
    filters: [{ name: extension.toUpperCase(), extensions: [extension] }],
  });
  return typeof selected === "string" ? selected : null;
}

/**
 * "Save as" dialog offering several output formats at once.
 *
 * Unlike {@link pickSavePath} the user can retarget the output format by
 * changing the extension; the backend infers `format` from the written path
 * when `transformImage` / `saveImageAs` omit it.
 */
export async function pickSavePathFor(
  defaultPath: string,
  extensions: readonly string[],
): Promise<string | null> {
  const exts = extensions.length > 0 ? [...extensions] : ["png"];
  const selected = await saveDialog({
    defaultPath,
    filters: [{ name: "图片", extensions: exts }],
  });
  return typeof selected === "string" ? selected : null;
}

/**
 * Native confirmation dialog (`Yes`/`No`).
 *
 * Used before irreversible actions such as deleting a file. Backed by the
 * `message` command, which `dialog:default` already permits.
 */
export async function confirmAction(message: string, title = "Lumina"): Promise<boolean> {
  return confirm(message, { title, kind: "warning" });
}

/* -------------------------------------------------------------------------- */
/* Filesystem watch                                                            */
/* -------------------------------------------------------------------------- */

/**
 * Subscribe to image changes in the watched directory.
 *
 * The payload only names the directory — re-list it and apply the result
 * wholesale rather than patching entries in place, so a missed event can never
 * leave a phantom entry on screen. Resolves to an unsubscribe function.
 */
export async function onDirectoryChanged(
  handler: (dir: string) => void,
): Promise<UnlistenFn> {
  return listen<{ dir: string }>("directory-changed", (event) => {
    handler(event.payload.dir);
  });
}

/* -------------------------------------------------------------------------- */
/* File association                                                            */
/* -------------------------------------------------------------------------- */

/** One image format family: its extensions, its wordmark and its current state. */
export interface AssocFormat {
  /** Icon key — also the `<key>.ico` file name among the bundle's resources. */
  key: string;
  label: string;
  /** Every extension in this family, e.g. `["jpg", "jpeg"]`. */
  ext: string[];
  /** True when the OS currently opens these with Lumina. */
  associated: boolean;
}

/**
 * Association state plus the platform's capability flags.
 *
 * The panel must word itself from these rather than assume: Windows will not
 * let an application become the default silently (the user confirms once in
 * Settings), and macOS cannot be associated from a running app at all.
 */
export interface AssocStatus {
  /** `std::env::consts::OS`: "windows" | "macos" | "linux". */
  platform: string;
  /** Whether each format can carry its own file icon (Windows only). */
  supports_per_format_icons: boolean;
  /** Whether Lumina can register itself at all. */
  can_associate: boolean;
  /** Whether Lumina can become the default without the user confirming elsewhere. */
  can_set_default: boolean;
  formats: AssocFormat[];
  /** Platform-specific explanation, shown verbatim in the panel. */
  note: string;
}

/** Current association state for every format Lumina can open. */
export function getAssociationStatus(): Promise<AssocStatus> {
  return invoke<AssocStatus>("get_association_status");
}

/**
 * Register or withdraw Lumina for exactly `exts`.
 *
 * This is the set to *change*, not the desired end state — send only the rows
 * the user actually toggled. Returns the refreshed status.
 */
export function setFileAssociation(
  exts: string[],
  enabled: boolean,
): Promise<AssocStatus> {
  return invoke<AssocStatus>("set_file_association", { exts, enabled });
}

/**
 * Open the operating system's "default apps" page.
 *
 * Windows only: it is the one place the user can confirm Lumina as the default
 * handler, which Windows 10/11 will not let an application do on its own.
 */
export function openDefaultAppsSettings(): Promise<void> {
  return invoke<void>("open_default_apps_settings");
}

/* -------------------------------------------------------------------------- */
/* Native drag & drop                                                          */
/* -------------------------------------------------------------------------- */

export type DropPhase = "enter" | "over" | "drop" | "leave";

/**
 * Subscribe to native file drag & drop over the current webview.
 *
 * Resolves to an unsubscribe function. `paths` is only populated for `enter`
 * and `drop`; the other phases just report cursor movement.
 */
export async function onFileDrop(
  handler: (phase: DropPhase, paths: string[]) => void,
): Promise<() => void> {
  return getCurrentWebview().onDragDropEvent((event) => {
    const payload = event.payload;
    if (payload.type === "enter" || payload.type === "drop") {
      handler(payload.type, payload.paths);
    } else {
      handler(payload.type, []);
    }
  });
}

