export interface HostedDeviceReview {
  destination: { origin:string; installation:string; device:string };
  invitation:string;
  approval_code:string;
}
export interface HostedDevicePage {
  installation_id:string; device_id:string; expires_at_s:number; scope:"connections.read"; next_after:string|null;
  items: HostedSavedSource[];
  pool_access?: boolean;
  pool_review?: HostedPoolReview;
}

export type HostedPoolAction = "include" | "exclude";
export interface HostedSavedSource {
  id: string; label: string | null; credential_state: "stored" | "revoked"; proxy_bound: boolean; verified_models: string[];
  pool?: {
    state: "enabled" | "paused" | "trashed";
    in_pool: boolean; draining: boolean; action: HostedPoolAction | null; revision: string | null;
  };
}
export interface HostedPoolChange { source_id: string; action: HostedPoolAction; expected_revision: string }
export interface HostedPoolReview {
  operation_id: string; source_id: string; action: HostedPoolAction; expires_at_s: number;
}

export type HostedReadScope = "overview.read" | "usage.read";
export type HostedHours = 24 | 168 | 720;
export type HostedExecution = "reserved" | "dispatched" | "completed" | "incomplete" | "failed" | "uncertain";
export interface HostedDeviceRead<T> {
  installation_id: string;
  device_id: string;
  expires_at_s: number;
  result: T;
}
export interface HostedOverview {
  hours: HostedHours;
  as_of_ms: number;
  requests: string;
  errors: string;
  total_tokens: string | null;
  token_samples: string;
  partial_tokens: string;
  active_requests: string;
  active_attempts: string;
  metrics?: {
    pool: { members: string; ready: string; busy: string } | null;
    generation: { samples: string; tokens_per_second: number } | null;
  };
}
export interface HostedUsageFilter {
  hours: HostedHours;
  model: string | null;
  execution: HostedExecution | null;
  as_of_ms: number | null;
  before: string | null;
}
export interface HostedUsage {
  hours: HostedHours;
  as_of_ms: number;
  model: string | null;
  execution: HostedExecution | null;
  items: {
    request_id: string;
    started_at_ms: number;
    model: string;
    execution: HostedExecution;
    tokens: string | null;
    partial_tokens: boolean;
    diagnostic_code: string | null;
  }[];
  next_before: string | null;
}

export type HostedCopyState = "awaiting_approval" | "approved" | "expired" | "committed" | "conflict";

export interface HostedCopyReview {
  code: string;
  plan: {
    origin: string;
    installation_id: string;
    device_id: string;
    operation_id: string;
    source_id: string;
    label: string;
    endpoint: string;
    proxy: { endpoint: string; authenticated: boolean } | null;
  };
}
