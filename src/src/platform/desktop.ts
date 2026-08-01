import { Channel, invoke } from "@tauri-apps/api/core";
import { getBundleType } from "@tauri-apps/api/app";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { relaunch } from "@tauri-apps/plugin-process";
import { check } from "@tauri-apps/plugin-updater";
import packageJson from "../../package.json";

export const APP_VERSION = packageJson.version;

export type AppUpdate = {
  currentVersion: string;
  version: string;
  date?: string;
  body?: string;
  portable: boolean;
};

type PortableDownloadEvent =
  | { event: "Started"; data: { contentLength?: number } }
  | { event: "Progress"; data: { chunkLength: number } }
  | { event: "Finished" };

export type Platform = "windows" | "macos" | "linux";

export function getPlatform() {
  return invoke<Platform>("get_platform");
}

export function getSystemLocale() {
  return invoke<string | null>("get_system_locale");
}

export function openApiKeyPage(provider: "zenith" | "openai" | "openrouter") {
  return invoke<void>("open_api_key_page", { provider });
}

export function minimizeWindow() {
  return getCurrentWindow().minimize();
}

export function toggleMaximizeWindow() {
  return getCurrentWindow().toggleMaximize();
}

export function closeWindow() {
  return getCurrentWindow().close();
}

export function setWindowBackgroundColor(color: string) {
  try {
    return getCurrentWebviewWindow().setBackgroundColor(color).catch(() => undefined);
  } catch {
    return Promise.resolve();
  }
}

export function recordPerformance(name: string, durationMs: number, context?: string) {
  if (!Number.isFinite(durationMs) || durationMs < 0) return Promise.resolve();
  return invoke<void>("record_local_performance_sample", {
    name,
    durationMs,
    context,
  }).catch(() => undefined);
}

async function getPortableUpdateTarget(): Promise<string | null> {
  const bundleType = await getBundleType().catch(() => undefined);
  if (bundleType !== null) return null;
  return invoke<string | null>("get_portable_update_target").catch(() => null);
}

async function checkPortableUpdate(target: string) {
  try {
    return await check({ target });
  } catch {
    // A stale but otherwise valid manifest can predate the portable target.
    // A generic check proves that the manifest is reachable and verified, but
    // its installer update must never be offered to a portable executable.
    const genericUpdate = await check();
    await genericUpdate?.close();
    return null;
  }
}

export async function checkForUpdate(): Promise<AppUpdate | null> {
  const portableTarget = await getPortableUpdateTarget();
  const update = portableTarget ? await checkPortableUpdate(portableTarget) : await check();
  if (!update) return null;
  const metadata = {
    currentVersion: update.currentVersion,
    version: update.version,
    date: update.date,
    body: update.body,
    portable: Boolean(portableTarget),
  };
  await update.close();
  return metadata;
}

export async function installUpdate(
  updateToInstall: AppUpdate,
  onProgress?: (downloaded: number, total?: number) => void,
) {
  if (updateToInstall.portable) {
    let downloaded = 0;
    let total: number | undefined;
    const channel = new Channel<PortableDownloadEvent>();
    channel.onmessage = (event) => {
      if (event.event === "Started") {
        total = event.data.contentLength;
        downloaded = 0;
        onProgress?.(downloaded, total);
      } else if (event.event === "Progress") {
        downloaded += event.data.chunkLength;
        onProgress?.(downloaded, total);
      }
    };
    await invoke<void>("install_portable_update", {
      expectedVersion: updateToInstall.version,
      onEvent: channel,
    });
    return "installed" as const;
  }

  const update = await check();
  if (!update || update.version !== updateToInstall.version) {
    await update?.close();
    return "unavailable" as const;
  }

  let downloaded = 0;
  let total: number | undefined;
  try {
    await update.downloadAndInstall((event) => {
      if (event.event === "Started") {
        total = event.data.contentLength ?? undefined;
        downloaded = 0;
        onProgress?.(downloaded, total);
      }
      if (event.event === "Progress") {
        downloaded += event.data.chunkLength;
        onProgress?.(downloaded, total);
      }
    });
  } catch (error) {
    await update.close().catch(() => undefined);
    throw error;
  }
  await relaunch();
  return "installed" as const;
}

export function restartApplication() {
  return relaunch();
}
