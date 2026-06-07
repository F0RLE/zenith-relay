import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { relaunch } from "@tauri-apps/plugin-process";
import { check } from "@tauri-apps/plugin-updater";

export type Platform = "windows" | "macos" | "linux";

export type UiState = {
  providerActive: boolean;
  codexRunning: boolean;
  savedApiKey: string;
};

export type KeyStats = {
  maskedKey: string;
  label: string | null;
  enabled: boolean;
  status: string;
  balanceCents: number;
  balanceMicrousd?: number | null;
  spentCents: number;
  spentMicrousd?: number | null;
  totalCreditsCents: number;
  totalCreditsMicrousd?: number | null;
  requests: number;
  inputTokens: number;
  cachedInputTokens: number;
  reasoningTokens: number;
  outputTokens: number;
  totalTokens: number;
  dailySpentCents: number;
  dailySpentMicrousd?: number | null;
  weeklySpentCents: number;
  weeklySpentMicrousd?: number | null;
  monthlySpentCents: number;
  monthlySpentMicrousd?: number | null;
  balance: string;
  spent: string;
  totalCredits: string;
  requestsDisplay: string;
  inputTokensDisplay: string;
  cachedInputTokensDisplay: string;
  reasoningTokensDisplay: string;
  outputTokensDisplay: string;
  totalTokensDisplay: string;
  dailySpent: string;
  weeklySpent: string;
  monthlySpent: string;
};

export type UsageLogEntry = {
  id: number;
  model: string | null;
  requestId: string | null;
  inputTokens: number;
  cachedInputTokens: number;
  reasoningTokens: number;
  outputTokens: number;
  totalTokens: number;
  costCents: number;
  costMicrousd?: number | null;
  status: string;
  createdAt: string;
  modelDisplay: string;
  createdAtDisplay: string;
  cost: string;
  inputTokensDisplay: string;
  cachedInputTokensDisplay: string;
  reasoningTokensDisplay: string;
  outputTokensDisplay: string;
  totalTokensDisplay: string;
};

export type UsageVersion = {
  version: number;
};

export type PreparedTopUpAmount = {
  amountCents: number;
  amountUsd: number;
  valid: boolean;
};

export function getState() {
  return invoke<UiState>("get_state");
}

export function getPlatform() {
  return invoke<Platform>("get_platform");
}

export function getSystemLocale() {
  return invoke<string | null>("get_system_locale");
}

export function saveKey(apiKey: string) {
  return invoke<string>("save_key", { apiKey });
}

export function resetKey() {
  return invoke<string>("reset_key");
}

export function launchCodex() {
  return invoke<string>("launch_saved_codex");
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

export function createTopUpIntentAndOpen(apiKey: string, amountCents: number) {
  return invoke<void>("create_top_up_intent_and_open", { apiKey, amountCents });
}

export function prepareTopUpAmount(value: string) {
  return invoke<PreparedTopUpAmount>("prepare_top_up_amount", { value });
}

export function getKeyStats(apiKey: string) {
  return invoke<KeyStats>("get_key_stats", { apiKey });
}

export function getKeyUsageHistory(apiKey: string, sinceId?: number, afterId?: number) {
  return invoke<{ usage: UsageLogEntry[]; limit: number; sinceId: number | null }>("get_key_usage_history", {
    apiKey,
    sinceId,
    afterId,
  });
}

export function getKeyUsageVersion(apiKey: string) {
  return invoke<UsageVersion>("get_key_usage_version", { apiKey });
}

export async function updateAndRelaunch(onProgress?: (downloaded: number, total?: number) => void) {
  const update = await check();
  if (!update) {
    return "none" as const;
  }

  let downloaded = 0;
  let total: number | undefined;
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
  await relaunch();
  return "installed" as const;
}

export function onStateChanged(callback: () => void) {
  return listen("zenith-state-changed", callback);
}

export function onKeyStatsChanged(callback: (stats: KeyStats) => void) {
  return listen<KeyStats>("zenith-key-stats-changed", (event) => callback(event.payload));
}

export function onUsageHistoryChanged(
  callback: (page: { usage: UsageLogEntry[]; limit: number; sinceId: number | null }) => void,
) {
  return listen<{ usage: UsageLogEntry[]; limit: number; sinceId: number | null }>(
    "zenith-usage-history-changed",
    (event) => callback(event.payload),
  );
}
