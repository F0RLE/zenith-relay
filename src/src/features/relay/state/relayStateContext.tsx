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
  perform: (operationId: string, work: () => Promise<unknown>, successKey?: string, options?: PerformOptions) => Promise<boolean>;
  activateCodexProfile: (profileId: string, work: () => Promise<ProfileActivation>, launchAfter?: boolean) => Promise<boolean>;
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
  routeRecoveryEnabled: boolean;
  setRouteRecoveryEnabled: (enabled: boolean) => Promise<boolean>;
  blockDegradedRoutesEnabled: boolean;
  setBlockDegradedRoutesEnabled: (enabled: boolean) => Promise<boolean>;
};

export type RelayUsageContextValue = {
  localUsagePage: LocalUsagePage | null;
  loadLocalUsage: (query: RemoteUsageQuery, options?: UsageLoadOptions) => Promise<LocalUsagePage>;
  remoteUsage: RemoteUsage[];
  remoteUsagePage: RemoteUsagePage | null;
  loadRemoteUsage: (query: RemoteUsageQuery, options?: UsageLoadOptions) => Promise<RemoteUsagePage | null>;
  usageRevision: number;
};

export const RelayContext = createContext<RelayContextValue | null>(null);
const RelayActivityContext = createContext<RuntimeActivityState | null>(null);
const RelayUsageContext = createContext<RelayUsageContextValue | null>(null);

export function RelayStateContexts({
  contextValue,
  activity,
  usage,
  children,
}: {
  contextValue: RelayContextValue;
  activity: RuntimeActivityState;
  usage: RelayUsageContextValue;
  children: ReactNode;
}) {
  return <RelayContext.Provider value={contextValue}>
    <RelayActivityContext.Provider value={activity}>
      <RelayUsageContext.Provider value={usage}>{children}</RelayUsageContext.Provider>
    </RelayActivityContext.Provider>
  </RelayContext.Provider>;
}

export function useRelayState() {
  const relayState = useContext(RelayContext);
  if (!relayState) throw new Error("RelayStateProvider is missing");
  return relayState;
}

export function useRelayActivity() {
  const activityState = useContext(RelayActivityContext);
  if (!activityState) throw new Error("RelayStateProvider is missing");
  return activityState;
}

export function useRelayUsageContext() {
  const usageContext = useContext(RelayUsageContext);
  if (!usageContext) throw new Error("RelayStateProvider is missing");
  return usageContext;
}
