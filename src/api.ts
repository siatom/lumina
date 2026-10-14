import { invoke } from "@tauri-apps/api/core";
import { open as openDialog, save as saveDialog } from "@tauri-apps/plugin-dialog";

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
  has_exif: boolean;
  exif: ExifData | null;
}

export interface ImageEntry {
  path: string;
  file_name: string;
  format: string;
  file_size: number;
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

/** Downscaled PNG thumbnail as a ready-to-use Blob. */
export async function getThumbnail(path: string, maxSize = 256): Promise<Blob> {
  const bytes = await invoke<ArrayBuffer>("get_thumbnail", { path, maxSize });
  return new Blob([bytes], { type: "image/png" });
}

/**
 * Original image bytes as a Blob. This is the fallback transport used when the
 * `lumina://` protocol is not available; prefer {@link luminaImageUrl}.
 */
export async function getImageBlob(path: string): Promise<Blob> {
  const bytes = await invoke<ArrayBuffer>("get_image_bytes", { path });
  return new Blob([bytes], { type: "image/png" });
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

/** One page / frame of an animated or multi-page file, as a PNG Blob. */
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

/**
 * Build a `lumina://` URL for an image.
 *
 * This is the preferred transport: the bytes are streamed by the webview
 * directly from Rust, with no base64 inflation and no JSON IPC hop.
 */
export async function luminaImageUrl(
  path: string,
  opts: { mode?: ImageMode; max?: number } = {},
): Promise<string> {
  const base = await protocolBase();
  const q = new URLSearchParams({ path, mode: opts.mode ?? "full" });
  if (opts.max !== undefined) q.set("max", String(opts.max));
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
