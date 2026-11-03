export type RelayMode = "local" | "hosted";
export type PageId = "overview" | "connections" | "pool" | "integrations" | "usage" | "profiles" | "settings" | "help";
export type DefaultServiceTier = "standard" | "fast" | "ultrafast";
export type ToolPolicyMode = "pass_through" | "automatic";
export type ToolPolicy = {
  mode: ToolPolicyMode;
};
export type ObservedServiceTier = string;
export type OperationalStatus = "rotation" | "quotaWait" | "unavailable" | "disabled";
