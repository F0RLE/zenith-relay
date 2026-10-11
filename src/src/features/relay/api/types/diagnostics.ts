export type RemoteTarget = {
  origin: string;
  serverId: string;
  identityFingerprint: string;
  serverVersion: string;
  protocolVersion: number;
  allowInsecureHttp: boolean;
  connectedAtMs: number;
};

export type RelayStorageInfo = {
  dataPath: string;
  logsPath: string;
  errorLogsPath: string;
  crashLogsPath: string;
  operationLogsPath: string;
};

export type DiagnosticSettings = {
  debugEnabled: boolean;
};
