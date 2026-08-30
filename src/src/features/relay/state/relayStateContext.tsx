import { createContext, useContext } from "react";
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

export type { Feedback, PerformOptions } from "./relayOperationModel";

export type RelayContextValue = {
  mode: RelayMode;
  setMode: (mode: RelayMode) => void;
  page: PageId;
  setPage: (page: PageId) => void;
  runtime: RuntimeSnapshot | null;
  /** Latest route fact received from the local scheduler activity stream. */
  runtimeActivity: RuntimeActivityState;
  runtimeRevision: number;
  usageRevision: number;
  accountIdentitiesVisible: boolean;
  accountIdentitiesBusy: boolean;
  canRevealAccountIdentities: boolean;
  setAccountIdentitiesVisible: (visible: boolean) => void;
  accountValueVisible: boolean;
  setAccountValueVisible: (visible: boolean) => void;
  accountDisplayName: (accountId?: string | null, fallbackLabel?: string | null) => string | null;
  localUsagePage: LocalUsagePage | null;
  loadLocalUsage: (query: RemoteUsageQuery) => Promise<LocalUsagePage>;
  remoteUsage: RemoteUsage[];
  remoteUsagePage: RemoteUsagePage | null;
  loadRemoteUsage: (query: RemoteUsageQuery) => Promise<RemoteUsagePage | null>;
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

export const RelayContext = createContext<RelayContextValue | null>(null);

export function useRelayState() {
  const value = useContext(RelayContext);
  if (!value) throw new Error("RelayStateProvider is missing");
  return value;
}
