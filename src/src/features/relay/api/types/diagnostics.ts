export type RemoteTarget = {
  origin: string;
  serverId: string;
  identityFingerprint: string;
  serverVersion: string;
  protocolVersion: number;
  allowInsecureHttp: boolean;
  connectedAtMs: number;
};

export type SupportBundlePreview = {
  bundle: {
    generatedAt: string;
    appVersion: string;
    platform: string;
    mode: "local" | "remote" | "zenith";
    schemaVersion: number | null;
    gatewayRunning: boolean;
    sourceCount: number;
    accountCount: number;
    automationCount: number;
    usageCount: number;
    warningCount: number;
  };
  excluded: string[];
};

export type RelayStorageInfo = {
  dataPath: string;
  logsPath: string;
  errorLogsPath: string;
  crashLogsPath: string;
  operationLogsPath: string;
};

export type DiagnosticPaths = {
  logsPath: string;
  errorLogsPath: string;
  crashLogsPath: string;
  operationLogsPath: string;
};

export type DiagnosticSettings = {
  debugEnabled: boolean;
};
