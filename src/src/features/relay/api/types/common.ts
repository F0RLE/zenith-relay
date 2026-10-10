export type RelayMode = "local" | "remote" | "zenith";
export type PageId = "overview" | "connections" | "pool" | "integrations" | "gateway" | "usage" | "profiles" | "settings" | "help";
export type DefaultServiceTier = "standard" | "fast" | "ultrafast";
export type ToolPolicyMode = "pass_through" | "automatic";
export type ToolPolicy = {
  mode: ToolPolicyMode;
};
export type ToolPolicyUpdate = { policy: ToolPolicy; expectedPolicy: ToolPolicy };
export type ObservedServiceTier = string;
export type OperationalStatus = "rotation" | "quotaWait" | "unavailable" | "disabled";
