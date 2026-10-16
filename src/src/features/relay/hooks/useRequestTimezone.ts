import { useCallback } from "react";
import { useTranslation } from "react-i18next";
import { relayCommands } from "../api/commands";
import type { RequestTimezonePolicy } from "../api/types";
import { useRelayState } from "../state/RelayStateProvider";
import type { PerformOptions } from "../state/relayOperationModel";

export const DEFAULT_REQUEST_TIMEZONE_POLICY: RequestTimezonePolicy = {
  enabled: false,
  accountTimezones: {},
  accountTimezoneBindings: {},
  accountTimezoneSources: {},
};

export function normalizeRequestTimezonePolicy(
  policy: Partial<RequestTimezonePolicy> | undefined,
): RequestTimezonePolicy {
  return {
    enabled: policy?.enabled ?? false,
    accountTimezones: policy?.accountTimezones ?? {},
    accountTimezoneBindings: policy?.accountTimezoneBindings ?? {},
    accountTimezoneSources: policy?.accountTimezoneSources ?? {},
  };
}

export function useRequestTimezone() {
  const { t } = useTranslation();
  const { mode, runtime, perform, busy } = useRelayState();
  const policy = runtime?.gateway.requestTimezone
    ? normalizeRequestTimezonePolicy(runtime.gateway.requestTimezone)
    : DEFAULT_REQUEST_TIMEZONE_POLICY;
  const supported = Boolean(runtime) && (mode === "local"
    || (mode === "remote" && runtime?.capabilities.features.includes("request_timezone_v1")));
  const loadCatalog = useCallback(async () => {
    const catalog = mode === "local"
      ? await relayCommands.requestTimezoneCatalog()
      : await relayCommands.remoteAction({ type: "request_timezone_catalog" });
    if (!Array.isArray(catalog) || !catalog.length || !catalog.every((zone): zone is string => typeof zone === "string" && zone.length > 0)) {
      throw new Error(t("gateway.timezone.catalogUnavailable"));
    }
    return [...new Set<string>(catalog)];
  }, [mode, t]);
  const persist = (next: RequestTimezonePolicy, expectedPolicy = policy, options?: PerformOptions) => perform("request-timezone", () => {
    const input = { policy: next, expectedPolicy };
    return mode === "local"
      ? relayCommands.setRequestTimezone(input)
      : relayCommands.remoteAction({ type: "set_request_timezone" }, input);
  }, "feedback.saved", { backgroundRefresh: true, ...options });
  return { supported, policy, busy, loadCatalog, persist };
}
