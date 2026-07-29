import { invoke } from "@tauri-apps/api/core";
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
};

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

export async function checkForUpdate(): Promise<AppUpdate | null> {
  const update = await check();
  if (!update) return null;
  const metadata = {
    currentVersion: update.currentVersion,
    version: update.version,
    date: update.date,
    body: update.body,
  };
  await update.close();
  return metadata;
}

export async function installUpdate(
  expectedVersion: string,
  onProgress?: (downloaded: number, total?: number) => void,
) {
  const update = await check();
  if (!update || update.version !== expectedVersion) {
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
