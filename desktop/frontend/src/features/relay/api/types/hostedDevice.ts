export interface HostedDeviceReview {
  destination: { origin:string; installation:string; device:string };
  invitation:string;
  approval_code:string;
}
export interface HostedDevicePage {
  installation_id:string; device_id:string; expires_at_s:number; scope:"connections.read"; next_after:string|null;
  items:{id:string;label:string|null;credential_state:"stored"|"revoked";proxy_bound:boolean;verified_models:string[]}[];
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
