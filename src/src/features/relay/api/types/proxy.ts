export type ProxyAssignmentResult = {
  assigned: number;
  unused: number;
};

export type ProxyPoolEntry = {
  id: string;
  endpoint: string;
  assignedAccountIds: string[];
  countryCode: string | null;
  region: string | null;
  createdAtMs: number;
  lastCheck?: ProxyCheckResult | null;
};

export type ProxyPoolSummary = {
  entries: ProxyPoolEntry[];
  total: number;
  free: number;
  assigned: number;
};

export type ProxyPoolImportResult = {
  added: number;
  duplicates: number;
  addedProxyIds: string[];
  pool: ProxyPoolSummary;
};

export type ProxyCheckResult = {
  proxyId: string;
  checkedAtMs: number;
  elapsedMs: number;
  ip: string | null;
  countryCode: string | null;
  errorCode: string | null;
};

export type StoredProxyAssignmentResult = {
  assigned: number;
  unchanged: number;
  unavailable: number;
  pool: ProxyPoolSummary;
};
