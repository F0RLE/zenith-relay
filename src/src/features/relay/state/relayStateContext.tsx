import { createContext, useContext } from "react";
import type {
  LocalUsage,
  LocalUsagePage,
  PageId,
  ProfileActivation,
  ProfileBinding,
  RelayMode,
  RemoteUsage,
  RemoteUsagePage,
  RemoteUsageQuery,
  RuntimeSnapshot,
} from "../api/types";
import type { UiState } from "../api/commands";
import type { FeedbackError } from "./feedback";
import type { AccountQuotaCalculationMode } from "./relayPreferences";

export type Feedback = { kind: "success" | "error"; key: string; error?: FeedbackError } | null;

export type RelayContextValue = {
  mode: RelayMode;
  setMode: (mode: RelayMode) => void;
  page: PageId;
  setPage: (page: PageId) => void;
  runtime: RuntimeSnapshot | null;
  runtimeRevision: number;
  usageRevision: number;
  accountIdentitiesVisible: boolean;
  accountIdentitiesBusy: boolean;
  canRevealAccountIdentities: boolean;
  setAccountIdentitiesVisible: (visible: boolean) => void;
  accountEconomicsVisible: boolean;
  setAccountEconomicsVisible: (visible: boolean) => void;
  accountQuotaCalculationMode: AccountQuotaCalculationMode;
  setAccountQuotaCalculationMode: (mode: AccountQuotaCalculationMode) => void;
  accountDisplayName: (accountId?: string | null, fallbackLabel?: string | null) => string | null;
  localUsage: LocalUsage[];
  localUsagePage: LocalUsagePage | null;
  loadLocalUsage: (query: RemoteUsageQuery) => Promise<LocalUsagePage>;
  remoteUsage: RemoteUsage[];
  remoteUsagePage: RemoteUsagePage | null;
  loadRemoteUsage: (query: RemoteUsageQuery) => Promise<RemoteUsagePage | null>;
  readyState: UiState | null;
  loading: boolean;
  busy: string | null;
  feedback: Feedback;
  refresh: () => Promise<void>;
  perform: (id: string, work: () => Promise<unknown>, successKey?: string) => Promise<boolean>;
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
  profileSnapshotBackupBeforeRestore: boolean;
  setProfileSnapshotBackupBeforeRestore: (enabled: boolean) => void;
  codexPoolOauthSelection: string;
  setCodexPoolOauthSelection: (selection: string) => void;
};

export const RelayContext = createContext<RelayContextValue | null>(null);

export function useRelayState() {
  const value = useContext(RelayContext);
  if (!value) throw new Error("RelayStateProvider is missing");
  return value;
}
