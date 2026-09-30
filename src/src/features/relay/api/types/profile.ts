import type { DefaultServiceTier, ToolPolicy } from "./common";
import type { ApiModelPriceOverride } from "./pricing";
import type { SourceProtocolBinding, SourceWireApi } from "./source";

export type OpenCodeConfigStatus = {
  configured: boolean;
  modelCount: number;
  hasBackup: boolean;
  backupCreatedAtMs?: number | null;
  backupName?: string | null;
  path: string;
};

export type ProfileSnapshot = {
  id: string;
  name: string;
  profileDir: string;
  createdAtMs: number;
  configAvailable: boolean;
  authAvailable: boolean;
  isOriginal?: boolean;
};

export type ProfileSnapshotList = {
  snapshots: ProfileSnapshot[];
  invalidCount: number;
};

type ConfigurationPresetMemberRule = {
  id: string;
  enabled: boolean;
  inPool: boolean;
  allowedModels: string[];
  excludedModels: string[];
  priority: number;
  weight: number;
};

export type ConfigurationPresetSourceRule = ConfigurationPresetMemberRule & {
  name: string;
  baseUrl: string;
  pricingProvider?: string | null;
  officialProviderFamily?: string | null;
  wireApi: SourceWireApi;
  protocolBindings?: SourceProtocolBinding[];
  serviceTier?: DefaultServiceTier;
  recoveryDelaySeconds: number;
  modelPriceOverrides: Record<string, ApiModelPriceOverride>;
};

export type ConfigurationPresetAccountRule = ConfigurationPresetMemberRule & {
  identityHint: string;
  proxyId: string | null;
  bypassCommonProxy?: boolean;
};

export type ConfigurationPreset = {
  format: "zenith-relay-configuration";
  schemaVersion: number;
  settings: {
    sources: ConfigurationPresetSourceRule[];
    accounts: ConfigurationPresetAccountRule[];
    routing: {
      toolPolicy?: ToolPolicy;
      maxRetryCandidates: number;
      defaultServiceTier: DefaultServiceTier;
      imageBaseModel: string | null;
    };
    quota: {
      requestTimeoutSeconds: number;
      accountProxyRequired: boolean;
      commonProxyId: string | null;
    };
    hiddenModels: string[];
    modelPriceOverrides: Record<string, ApiModelPriceOverride>;
    modelServiceTierOverrides?: Record<string, DefaultServiceTier>;
    modelDisplayOrder?: string[];
    modelReasoningAllowedLevels?: Record<string, string[]>;
  };
};

export type ConfigurationPresetChange = {
  path: string;
  before: unknown;
  after: unknown;
};

export type ConfigurationPresetPreview = {
  baseRevision: string;
  preset: ConfigurationPreset;
  changes: ConfigurationPresetChange[];
};

export type ConfigurationPresetApplyResult = {
  previousRevision: string;
  revision: string;
  changes: ConfigurationPresetChange[];
};

export type ProfileBinding = {
  profileDir: string;
  credentialKind: "oauth_account" | "api_key" | "local_gateway";
  credentialId: string;
  boundOauthAccountId: string | null;
  active: boolean;
};

export type ProfileActivation = {
  binding: ProfileBinding;
};
