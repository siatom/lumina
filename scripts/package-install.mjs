#!/usr/bin/env node

import { spawnSync } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, readdirSync, renameSync, rmSync, statSync } from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const platform = process.platform;
const mode = process.argv[2] ?? "";
const usage = "Usage: node scripts/package-install.mjs [--build-only|--install-only]";

if (mode === "--help" || mode === "-h") {
  console.log(usage);
  process.exit(0);
}

if (platform !== "win32" && platform !== "darwin") {
  console.error("This script supports Windows (NSIS) and macOS (DMG) only.");
  process.exit(1);
}
if (mode && mode !== "--build-only" && mode !== "--install-only") {
  console.error(usage);
  process.exit(2);
}

const bundle = platform === "win32" ? "nsis" : "dmg";
const extension = platform === "win32" ? ".exe" : ".dmg";
const artifactDir = path.join(root, "src-tauri", "target", "release", "bundle", bundle);

function run(command, args, options = {}) {
  console.log(`> ${command} ${args.join(" ")}`);
  const result = spawnSync(command, args, { cwd: root, stdio: "inherit", ...options });
  if (result.error) throw result.error;
  if (result.status !== 0) throw new Error(`${command} exited with status ${result.status}`);
}

function findArtifact() {
  if (!existsSync(artifactDir)) throw new Error(`Artifact directory not found: ${artifactDir}`);
  const files = readdirSync(artifactDir)
    .filter((name) => name.toLowerCase().endsWith(extension) && name.toLowerCase().startsWith("lumina"))
    .map((name) => path.join(artifactDir, name))
    .filter((file) => statSync(file).isFile())
    .sort((a, b) => statSync(b).mtimeMs - statSync(a).mtimeMs);
  if (!files.length) throw new Error(`No Lumina ${bundle.toUpperCase()} package found in ${artifactDir}`);
  return files[0];
}

function build() {
  const args = ["tauri", "build", "--bundles", bundle];
  if (platform === "win32") {
    // pnpm is normally exposed as pnpm.cmd on Windows, which CreateProcess
    // cannot launch directly, so route through cmd.exe. `shell: true` would do
    // the same thing but triggers DEP0190 (args + shell) on Node 22+.
    run(process.env.ComSpec ?? "cmd.exe", ["/d", "/s", "/c", "pnpm", ...args]);
  } else {
    run("pnpm", args);
  }
}

function installWindows(installer) {
  run(installer, ["/S"]);
}

function installMac(dmg) {
  const tempRoot = mkdtempSync(path.join(os.tmpdir(), "lumina-install-"));
  const mount = path.join(tempRoot, "mount");
  mkdirSync(mount);
  let mounted = false;
  let stage;
  let backup;
  const applications = path.join(os.homedir(), "Applications");
  const destination = path.join(applications, "Lumina.app");
  try {
    run("hdiutil", ["attach", "-nobrowse", "-quiet", "-mountpoint", mount, dmg]);
    mounted = true;
    const app = readdirSync(mount)
      .map((name) => path.join(mount, name))
      .find((candidate) => candidate.toLowerCase().endsWith(".app") && statSync(candidate).isDirectory());
    if (!app) throw new Error(`No .app bundle found inside ${dmg}`);

    mkdirSync(applications, { recursive: true });
    stage = path.join(applications, `.Lumina.app-stage-${process.pid}`);
    backup = path.join(applications, `.Lumina.app-backup-${process.pid}`);
    rmSync(stage, { recursive: true, force: true });
    run("/usr/bin/ditto", ["--rsrc", "--extattr", "--acl", app, stage]);

    if (existsSync(destination)) renameSync(destination, backup);
    try {
      renameSync(stage, destination);
    } catch (error) {
      if (existsSync(backup) && !existsSync(destination)) renameSync(backup, destination);
      throw error;
    }
    if (existsSync(backup)) rmSync(backup, { recursive: true, force: true });
    console.log(`Installed silently to ${destination}`);
  } finally {
    if (mounted) {
      try { run("hdiutil", ["detach", "-quiet", mount]); }
      catch (error) { console.error(`Could not detach the DMG: ${error.message}`); }
    }
    if (stage && existsSync(stage)) rmSync(stage, { recursive: true, force: true });
    rmSync(tempRoot, { recursive: true, force: true });
  }
}

try {
  if (mode !== "--install-only") build();
  const artifact = findArtifact();
  console.log(`Package: ${artifact}`);
  if (mode !== "--build-only") {
    if (platform === "win32") installWindows(artifact);
    else installMac(artifact);
  }
} catch (error) {
  console.error(`Packaging/install failed: ${error.message}`);
  process.exitCode = 1;
}
