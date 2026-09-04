import { createContext, useContext, type ReactNode } from "react";
import type {
  LocalUsagePage,
  PageId,
  ProfileActivation,
  ProfileBinding,
  RelayMode,
  RemoteUsage,
  RemoteUsagePage,
  RemoteUsageQuery,
  RuntimeActivityState,
  RuntimeSnapshot,
} from "../api/types";
import type { UiState } from "../api/commands";
import type { Feedback, PerformOptions } from "./relayOperationModel";
import type { UsageLoadOptions } from "./useRelayUsage";

export type { Feedback, PerformOptions } from "./relayOperationModel";

export type RelayContextValue = {
  mode: RelayMode;
  setMode: (mode: RelayMode) => void;
  page: PageId;
  setPage: (page: PageId) => void;
  runtime: RuntimeSnapshot | null;
  runtimeRevision: number;
  accountIdentitiesVisible: boolean;
  accountIdentitiesBusy: boolean;
  canRevealAccountIdentities: boolean;
  setAccountIdentitiesVisible: (visible: boolean) => void;
  accountValueVisible: boolean;
  setAccountValueVisible: (visible: boolean) => void;
  accountDisplayName: (accountId?: string | null, fallbackLabel?: string | null) => string | null;
  readyState: UiState | null;
  loading: boolean;
  busy: string | null;
  feedback: Feedback;
  refresh: (force?: boolean) => Promise<void>;
  perform: (id: string, work: () => Promise<unknown>, successKey?: string, options?: PerformOptions) => Promise<boolean>;
  activateCodexProfile: (id: string, work: () => Promise<ProfileActivation>, launchAfter?: boolean) => Promise<boolean>;
  launchCodexProfile: (binding: ProfileBinding) => Promise<boolean>;
  clearFeedback: () => void;
  onboardingComplete: boolean;
  finishOnboarding: (mode: RelayMode) => void;
  resetOnboarding: () => void;
  theme: "system" | "light" | "dark";
  setTheme: (theme: "system" | "light" | "dark") => void;
  profileSwitchBackupPrompt: boolean;
  setProfileSwitchBackupPrompt: (enabled: boolean) => void;
  codexPoolOauthSelection: string;
  setCodexPoolOauthSelection: (selection: string) => void;
  codexBackgroundTasksEnabled: boolean;
  setCodexBackgroundTasksEnabled: (enabled: boolean) => Promise<boolean>;
  codexWebsocketsEnabled: boolean;
  setCodexWebsocketsEnabled: (enabled: boolean) => Promise<boolean>;
};

export type RelayUsageContextValue = {
  localUsagePage: LocalUsagePage | null;
  loadLocalUsage: (query: RemoteUsageQuery, options?: UsageLoadOptions) => Promise<LocalUsagePage>;
  remoteUsage: RemoteUsage[];
  remoteUsagePage: RemoteUsagePage | null;
  loadRemoteUsage: (query: RemoteUsageQuery, options?: UsageLoadOptions) => Promise<RemoteUsagePage | null>;
  revision: number;
};

export const RelayContext = createContext<RelayContextValue | null>(null);
const RelayActivityContext = createContext<RuntimeActivityState | null>(null);
const RelayUsageContext = createContext<RelayUsageContextValue | null>(null);

export function RelayStateContexts({
  value,
  activity,
  usage,
  children,
}: {
  value: RelayContextValue;
  activity: RuntimeActivityState;
  usage: RelayUsageContextValue;
  children: ReactNode;
}) {
  return <RelayContext.Provider value={value}>
    <RelayActivityContext.Provider value={activity}>
      <RelayUsageContext.Provider value={usage}>{children}</RelayUsageContext.Provider>
    </RelayActivityContext.Provider>
  </RelayContext.Provider>;
}

export function useRelayState() {
  const value = useContext(RelayContext);
  if (!value) throw new Error("RelayStateProvider is missing");
  return value;
}

export function useRelayActivity() {
  const value = useContext(RelayActivityContext);
  if (!value) throw new Error("RelayStateProvider is missing");
  return value;
}

export function useRelayUsageContext() {
  const value = useContext(RelayUsageContext);
  if (!value) throw new Error("RelayStateProvider is missing");
  return value;
}
