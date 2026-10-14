import { invoke } from "@tauri-apps/api/core";
import { open as openDialog, save as saveDialog } from "@tauri-apps/plugin-dialog";

/* -------------------------------------------------------------------------- */
/* Types — mirrored 1:1 in docs/INTERFACES.md and in Rust (src-tauri/src).     */
/* -------------------------------------------------------------------------- */

export interface ExifData {
  orientation: number | null;
  make: string | null;
  model: string | null;
  datetime: string | null;
  software: string | null;
  exposure_time: string | null;
  f_number: string | null;
  iso: string | null;
  focal_length: string | null;
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
 * follows the output file extension.
 */
export function saveImageAs(
  inputPath: string,
  outputPath: string,
  format?: string,
): Promise<string> {
  return invoke<string>("save_image_as", {
    inputPath,
    outputPath,
    format: format ?? null,
  });
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
