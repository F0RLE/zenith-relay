import { useCallback, useEffect, useRef, useState } from "react";
import { checkForUpdate, installUpdate, type AppUpdate } from "../../../platform/desktop";
import { readRelayPreference, writeRelayPreference } from "../state/relayPreferences";

export const SKIPPED_UPDATE_KEY = "relay.skippedUpdate";
export const UPDATE_CHECK_INTERVAL_MS = 8 * 60 * 60 * 1_000;

export type UpdateCheckState = "idle" | "checking" | "current" | "available" | "error" | "skipped";
export type UpdateInstallError = "write" | "install" | null;
export type UpdateProgress = { downloaded: number; total?: number };
export type UpdateCheckOptions = {
  openWhenAvailable?: boolean;
  includeSkipped?: boolean;
};

type ResolvedUpdateCheckOptions = Required<UpdateCheckOptions>;

function resolveCheckOptions(options: UpdateCheckOptions = {}): ResolvedUpdateCheckOptions {
  return {
    openWhenAvailable: Boolean(options.openWhenAvailable),
    includeSkipped: Boolean(options.includeSkipped),
  };
}

export type AppUpdates = {
  availableUpdate: AppUpdate | null;
  updateDialogOpen: boolean;
  updateCheckState: UpdateCheckState;
  installingUpdate: boolean;
  updateProgress: UpdateProgress | null;
  updateInstallError: UpdateInstallError;
  checkUpdates: (options?: UpdateCheckOptions) => Promise<UpdateCheckState>;
  applyUpdate: () => Promise<void>;
  skipUpdate: () => void;
  openUpdateDialog: () => void;
  closeUpdateDialog: () => void;
};

/** Own update discovery, installation state, and the periodic refresh policy. */
export function useAppUpdates(): AppUpdates {
  const [availableUpdate, setAvailableUpdate] = useState<AppUpdate | null>(null);
  const [updateDialogOpen, setUpdateDialogOpen] = useState(false);
  const [updateCheckState, setUpdateCheckState] = useState<UpdateCheckState>("idle");
  const [installingUpdate, setInstallingUpdate] = useState(false);
  const [updateProgress, setUpdateProgress] = useState<UpdateProgress | null>(null);
  const [updateInstallError, setUpdateInstallError] = useState<UpdateInstallError>(null);
  const updateCheckInFlight = useRef<Promise<UpdateCheckState> | null>(null);
  const pendingCheckOptions = useRef<ResolvedUpdateCheckOptions>(resolveCheckOptions());
  const installInFlight = useRef(false);
  const checkedVersion = useRef<string | null>(null);

  const checkUpdates = useCallback(async (options: UpdateCheckOptions = {}): Promise<UpdateCheckState> => {
    if (installInFlight.current) return "available";
    const requestedOptions = resolveCheckOptions(options);
    if (updateCheckInFlight.current) {
      // The first caller owns the network request, but a later explicit user
      // action must still influence how its result is presented.
      pendingCheckOptions.current.openWhenAvailable ||= requestedOptions.openWhenAvailable;
      pendingCheckOptions.current.includeSkipped ||= requestedOptions.includeSkipped;
      return updateCheckInFlight.current;
    }
    pendingCheckOptions.current = requestedOptions;
    const pending = (async () => {
      setUpdateCheckState("checking");
      try {
        const update = await checkForUpdate();
        if (installInFlight.current) return "available" as const;
        if (checkedVersion.current !== (update?.version ?? null)) {
          checkedVersion.current = update?.version ?? null;
          setUpdateInstallError(null);
          setUpdateProgress(null);
        }
        if (!update) {
          setAvailableUpdate(null);
          setUpdateCheckState("current");
          return "current" as const;
        }
        const { includeSkipped, openWhenAvailable } = pendingCheckOptions.current;
        if (!includeSkipped && readRelayPreference(SKIPPED_UPDATE_KEY, "") === update.version) {
          setAvailableUpdate(null);
          setUpdateCheckState("skipped");
          return "skipped" as const;
        }
        setAvailableUpdate(update);
        setUpdateCheckState("available");
        if (openWhenAvailable) setUpdateDialogOpen(true);
        return "available" as const;
      } catch {
        if (installInFlight.current) return "available" as const;
        setUpdateCheckState("error");
        return "error" as const;
      }
    })();
    updateCheckInFlight.current = pending;
    try {
      return await pending;
    } finally {
      if (updateCheckInFlight.current === pending) {
        updateCheckInFlight.current = null;
        pendingCheckOptions.current = resolveCheckOptions();
      }
    }
  }, []);

  const applyUpdate = useCallback(async () => {
    if (!availableUpdate || installInFlight.current) return;
    installInFlight.current = true;
    setUpdateCheckState("available");
    setInstallingUpdate(true);
    setUpdateInstallError(null);
    setUpdateProgress({ downloaded: 0 });
    try {
      const installResult = await installUpdate(availableUpdate, (downloaded, total) => setUpdateProgress({ downloaded, ...(total !== undefined ? { total } : {}) }));
      if (installResult === "unavailable") {
        setUpdateCheckState("error");
        setUpdateInstallError("install");
        setUpdateDialogOpen(true);
      }
    } catch (error) {
      setUpdateInstallError(String(error).includes("portable_not_writable") ? "write" : "install");
    } finally {
      installInFlight.current = false;
      setInstallingUpdate(false);
    }
  }, [availableUpdate]);

  const skipUpdate = useCallback(() => {
    if (availableUpdate) writeRelayPreference(SKIPPED_UPDATE_KEY, availableUpdate.version);
    setAvailableUpdate(null);
    setUpdateDialogOpen(false);
    setUpdateCheckState("skipped");
  }, [availableUpdate]);

  const openUpdateDialog = useCallback(() => setUpdateDialogOpen(true), []);
  const closeUpdateDialog = useCallback(() => {
    if (!installingUpdate) setUpdateDialogOpen(false);
  }, [installingUpdate]);

  useEffect(() => {
    void checkUpdates();
  }, [checkUpdates]);

  useEffect(() => {
    const checkWhenActive = () => {
      if (document.visibilityState !== "visible") return;
      void checkUpdates();
    };
    const interval = window.setInterval(checkWhenActive, UPDATE_CHECK_INTERVAL_MS);
    window.addEventListener("focus", checkWhenActive);
    document.addEventListener("visibilitychange", checkWhenActive);
    return () => {
      window.clearInterval(interval);
      window.removeEventListener("focus", checkWhenActive);
      document.removeEventListener("visibilitychange", checkWhenActive);
    };
  }, [checkUpdates]);

  return {
    availableUpdate,
    updateDialogOpen,
    updateCheckState,
    installingUpdate,
    updateProgress,
    updateInstallError,
    checkUpdates,
    applyUpdate,
    skipUpdate,
    openUpdateDialog,
    closeUpdateDialog,
  };
}
