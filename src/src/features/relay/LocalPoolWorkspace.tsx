import { FormEvent, ReactNode, useCallback, useEffect, useMemo, useState } from "react";
import {
  CheckCircle2,
  CircleAlert,
  Copy,
  KeyRound,
  Link2,
  Loader2,
  Play,
  RefreshCw,
  RotateCcw,
  Square,
} from "lucide-react";
import { useTranslation } from "react-i18next";
import {
  attachCodexToLocalGateway,
  createLocalGatewayKey,
  createLocalSource,
  getLocalPoolState,
  getLocalUsage,
  LocalPoolState,
  LocalUsageLog,
  restoreCodexProfile,
  startLocalGateway,
  stopLocalGateway,
  testLocalSource,
} from "../../tauri";

type Action =
  | "attach"
  | "copyEndpoint"
  | "copyKey"
  | "createKey"
  | "createSource"
  | "refreshUsage"
  | "restore"
  | "start"
  | "stop"
  | "testSource";

export function LocalPoolWorkspace() {
  const { i18n, t } = useTranslation();
  const [state, setState] = useState<LocalPoolState | null>(null);
  const [usage, setUsage] = useState<LocalUsageLog[]>([]);
  const [loading, setLoading] = useState(true);
  const [action, setAction] = useState<Action | null>(null);
  const [errorCode, setErrorCode] = useState<string | null>(null);
  const [noticeKey, setNoticeKey] = useState<string | null>(null);
  const [sourceName, setSourceName] = useState("");
  const [sourceUrl, setSourceUrl] = useState("");
  const [sourceKey, setSourceKey] = useState("");
  const [keyLabel, setKeyLabel] = useState(() => t("localPool.key.defaultLabel"));
  const [generatedKey, setGeneratedKey] = useState("");

  const source = state?.sources[0] ?? null;
  const localKey = state?.keys[0] ?? null;
  const running = state?.runtimeTarget.connected ?? false;
  const endpoint = useMemo(
    () =>
      state
        ? `http://${state.gateway.clientHost}:${state.gateway.port}/v1`
        : "",
    [state],
  );

  const refresh = useCallback(async () => {
    const snapshot = await getLocalPoolState();
    setState(snapshot);
    const logs = await getLocalUsage(25).catch(() => null);
    if (logs) setUsage(logs);
  }, []);

  useEffect(() => {
    let active = true;
    refresh()
      .catch((error) => {
        if (active) setErrorCode(commandErrorCode(error));
      })
      .finally(() => {
        if (active) setLoading(false);
      });
    return () => {
      active = false;
    };
  }, [refresh]);

  async function perform(currentAction: Action, task: () => Promise<void>, successKey?: string) {
    setAction(currentAction);
    setErrorCode(null);
    setNoticeKey(null);
    try {
      await task();
      if (successKey) setNoticeKey(successKey);
    } catch (error) {
      setErrorCode(commandErrorCode(error));
    } finally {
      setAction(null);
    }
  }

  async function handleCreateSource(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    await perform(
      "createSource",
      async () => {
        await createLocalSource({
          name: sourceName,
          baseUrl: sourceUrl,
          apiKey: sourceKey,
          wireApi: "responses",
        });
        setSourceKey("");
        await refresh();
      },
      "localPool.notices.sourceSaved",
    );
  }

  async function handleCreateKey(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    await perform(
      "createKey",
      async () => {
        const generated = await createLocalGatewayKey(keyLabel);
        setGeneratedKey(generated.secret);
        await refresh();
      },
      "localPool.notices.keyCreated",
    );
  }

  async function copy(value: string, currentAction: Action, successKey: string) {
    await perform(currentAction, () => navigator.clipboard.writeText(value), successKey);
  }

  if (loading) {
    return (
      <section className="local-pool-loading" aria-busy="true">
        <Loader2 className="spin" aria-hidden />
        <span>{t("localPool.loading")}</span>
      </section>
    );
  }

  return (
    <section className="local-pool-workspace" aria-labelledby="local-pool-title">
      <header className="local-pool-header">
        <div>
          <h1 id="local-pool-title">{t("localPool.title")}</h1>
          <p>{t("localPool.subtitle")}</p>
        </div>
        {running ? (
          <ActionButton
            action={action}
            current="stop"
            className="danger-secondary"
            icon={<Square aria-hidden />}
            label={t("localPool.gateway.stop")}
            loadingLabel={t("localPool.gateway.stopping")}
            onClick={() =>
              perform(
                "stop",
                async () => {
                  setState(await stopLocalGateway());
                  setUsage(await getLocalUsage(25).catch(() => usage));
                },
                "localPool.notices.gatewayStopped",
              )
            }
          />
        ) : (
          <ActionButton
            action={action}
            current="start"
            className="primary-action"
            disabled={!source || !localKey}
            icon={<Play aria-hidden />}
            label={t("localPool.gateway.start")}
            loadingLabel={t("localPool.gateway.starting")}
            onClick={() =>
              perform(
                "start",
                async () => {
                  setState(await startLocalGateway());
                  setUsage(await getLocalUsage(25).catch(() => usage));
                },
                "localPool.notices.gatewayStarted",
              )
            }
            title={!source || !localKey ? t("localPool.gateway.requiresSetup") : undefined}
          />
        )}
      </header>

      <div className={`local-runtime-strip ${running ? "is-running" : ""}`}>
        <div className="runtime-status">
          {running ? <CheckCircle2 aria-hidden /> : <CircleAlert aria-hidden />}
          <span>{running ? t("localPool.gateway.running") : t("localPool.gateway.stopped")}</span>
        </div>
        <div className="runtime-endpoint">
          <span>{t("localPool.gateway.endpoint")}</span>
          <code>{endpoint}</code>
          <button
            className="compact-icon-button"
            type="button"
            aria-label={t("localPool.actions.copyEndpoint")}
            title={t("localPool.actions.copyEndpoint")}
            disabled={!endpoint || action !== null}
            onClick={() => copy(endpoint, "copyEndpoint", "localPool.notices.endpointCopied")}
          >
            {action === "copyEndpoint" ? <Loader2 className="spin" aria-hidden /> : <Copy aria-hidden />}
          </button>
        </div>
        <div className="runtime-metric">
          <span>{t("localPool.gateway.models")}</span>
          <strong>{source?.models.length ?? 0}</strong>
        </div>
        <div className="runtime-metric">
          <span>{t("localPool.gateway.requests")}</span>
          <strong>{usage.length}</strong>
        </div>
      </div>

      <div className="local-feedback" aria-live="polite">
        {errorCode ? (
          <p className="local-message error-message">
            <CircleAlert aria-hidden />
            {t(`localPool.errors.${errorCode}`)}
          </p>
        ) : null}
        {noticeKey ? (
          <p className="local-message success-message">
            <CheckCircle2 aria-hidden />
            {t(noticeKey)}
          </p>
        ) : null}
        {state?.warnings.map((warning) => (
          <p className="local-message warning-message" key={warning}>
            <CircleAlert aria-hidden />
            {t(warningTranslationKey(warning))}
          </p>
        ))}
      </div>

      <div className="local-pool-grid">
        <section className="local-section" aria-labelledby="source-title">
          <div className="local-section-heading">
            <div>
              <span className="section-step">1</span>
              <h2 id="source-title">{t("localPool.source.title")}</h2>
            </div>
            {source ? (
              <ActionButton
                action={action}
                current="testSource"
                className="secondary-action"
                disabled={running}
                icon={<RefreshCw aria-hidden />}
                label={t("localPool.source.refreshModels")}
                loadingLabel={t("localPool.source.testing")}
                onClick={() =>
                  perform(
                    "testSource",
                    async () => {
                      await testLocalSource(source.id);
                      await refresh();
                    },
                    "localPool.notices.modelsRefreshed",
                  )
                }
                title={running ? t("localPool.source.stopToRefresh") : undefined}
              />
            ) : null}
          </div>

          {source ? (
            <div className="source-summary">
              <div className="source-summary-line">
                <span className={`status-badge ${source.lastTestStatus === "ok" ? "success" : "warning"}`}>
                  {source.lastTestStatus === "ok" ? t("localPool.source.ready") : t("localPool.source.checkNeeded")}
                </span>
                <strong>{source.name}</strong>
              </div>
              <code>{source.baseUrl}</code>
              <dl className="compact-details">
                <div>
                  <dt>{t("localPool.source.protocol")}</dt>
                  <dd>{t("localPool.source.protocolResponses")}</dd>
                </div>
                <div>
                  <dt>{t("localPool.source.lastCheck")}</dt>
                  <dd>{formatDate(source.lastTestAt, i18n.language, t("localPool.common.never"))}</dd>
                </div>
              </dl>
              <div className="model-list" aria-label={t("localPool.source.models")}>
                {source.models.map((model) => (
                  <code key={model}>{model}</code>
                ))}
              </div>
            </div>
          ) : (
            <form className="compact-form" onSubmit={handleCreateSource}>
              <label>
                <span>{t("localPool.source.name")}</span>
                <input
                  required
                  value={sourceName}
                  onChange={(event) => setSourceName(event.target.value)}
                  placeholder={t("localPool.source.namePlaceholder")}
                  disabled={action !== null}
                />
              </label>
              <label>
                <span>{t("localPool.source.baseUrl")}</span>
                <input
                  required
                  type="url"
                  value={sourceUrl}
                  onChange={(event) => setSourceUrl(event.target.value)}
                  placeholder={t("localPool.source.baseUrlPlaceholder")}
                  spellCheck={false}
                  disabled={action !== null}
                />
              </label>
              <label>
                <span>{t("localPool.source.apiKey")}</span>
                <input
                  required
                  type="password"
                  value={sourceKey}
                  onChange={(event) => setSourceKey(event.target.value)}
                  autoComplete="off"
                  spellCheck={false}
                  disabled={action !== null}
                />
              </label>
              <p className="form-hint">{t("localPool.source.discoveryHint")}</p>
              <ActionButton
                action={action}
                current="createSource"
                className="primary-action"
                icon={<Link2 aria-hidden />}
                label={t("localPool.source.add")}
                loadingLabel={t("localPool.source.adding")}
                submit
              />
            </form>
          )}
        </section>

        <section className="local-section" aria-labelledby="key-title">
          <div className="local-section-heading">
            <div>
              <span className="section-step">2</span>
              <h2 id="key-title">{t("localPool.key.title")}</h2>
            </div>
          </div>

          {localKey ? (
            <div className="key-summary">
              <div className="key-record">
                <KeyRound aria-hidden />
                <div>
                  <strong>{localKey.label}</strong>
                  <code>zlr_••••••••••••</code>
                </div>
                <span className="status-badge success">{t("localPool.key.active")}</span>
              </div>
              {generatedKey ? (
                <div className="generated-secret">
                  <div>
                    <strong>{t("localPool.key.generated")}</strong>
                    <span>{t("localPool.key.shownOnce")}</span>
                  </div>
                  <div className="secret-output">
                    <code>{generatedKey}</code>
                    <button
                      className="compact-icon-button"
                      type="button"
                      aria-label={t("localPool.actions.copyKey")}
                      title={t("localPool.actions.copyKey")}
                      disabled={action !== null}
                      onClick={() => copy(generatedKey, "copyKey", "localPool.notices.keyCopied")}
                    >
                      {action === "copyKey" ? <Loader2 className="spin" aria-hidden /> : <Copy aria-hidden />}
                    </button>
                  </div>
                </div>
              ) : (
                <p className="form-hint">{t("localPool.key.stored")}</p>
              )}
            </div>
          ) : (
            <form className="compact-form" onSubmit={handleCreateKey}>
              <label>
                <span>{t("localPool.key.label")}</span>
                <input
                  required
                  value={keyLabel}
                  onChange={(event) => setKeyLabel(event.target.value)}
                  disabled={action !== null}
                />
              </label>
              <p className="form-hint">{t("localPool.key.hint")}</p>
              <ActionButton
                action={action}
                current="createKey"
                className="primary-action"
                icon={<KeyRound aria-hidden />}
                label={t("localPool.key.create")}
                loadingLabel={t("localPool.key.creating")}
                submit
              />
            </form>
          )}
        </section>

        <section className="local-section client-section" aria-labelledby="client-title">
          <div className="local-section-heading">
            <div>
              <span className="section-step">3</span>
              <h2 id="client-title">{t("localPool.client.title")}</h2>
            </div>
          </div>
          <p>{t("localPool.client.description")}</p>
          <div className="client-actions">
            <ActionButton
              action={action}
              current="attach"
              className="primary-action"
              disabled={!running || !localKey}
              icon={<Link2 aria-hidden />}
              label={t("localPool.client.attach")}
              loadingLabel={t("localPool.client.attaching")}
              onClick={() =>
                perform(
                  "attach",
                  () => attachCodexToLocalGateway(localKey!.id),
                  "localPool.notices.codexAttached",
                )
              }
              title={!running ? t("localPool.client.requiresRunning") : undefined}
            />
            <ActionButton
              action={action}
              current="restore"
              className="secondary-action"
              icon={<RotateCcw aria-hidden />}
              label={t("localPool.client.restore")}
              loadingLabel={t("localPool.client.restoring")}
              onClick={() =>
                perform("restore", restoreCodexProfile, "localPool.notices.codexRestored")
              }
            />
          </div>
          <div className="manual-config">
            <span>{t("localPool.client.manual")}</span>
            <code>{endpoint}</code>
          </div>
        </section>

        <section className="local-section usage-section" aria-labelledby="local-usage-title">
          <div className="local-section-heading">
            <div>
              <span className="section-step">4</span>
              <h2 id="local-usage-title">{t("localPool.usage.title")}</h2>
            </div>
            <ActionButton
              action={action}
              current="refreshUsage"
              className="secondary-action"
              icon={<RefreshCw aria-hidden />}
              label={t("localPool.usage.refresh")}
              loadingLabel={t("localPool.usage.refreshing")}
              onClick={() =>
                perform("refreshUsage", async () => setUsage(await getLocalUsage(25)))
              }
            />
          </div>

          {usage.length ? (
            <div className="local-usage-table-wrap">
              <table className="local-usage-table">
                <thead>
                  <tr>
                    <th>{t("localPool.usage.time")}</th>
                    <th>{t("localPool.usage.status")}</th>
                    <th>{t("localPool.usage.model")}</th>
                    <th>{t("localPool.usage.latency")}</th>
                    <th>{t("localPool.usage.tokens")}</th>
                  </tr>
                </thead>
                <tbody>
                  {usage.map((entry) => (
                    <tr key={entry.id}>
                      <td>{formatDate(entry.createdAt, i18n.language, t("localPool.common.never"))}</td>
                      <td>
                        <span className={`status-badge ${entry.success ? "success" : "error"}`}>
                          {entry.success ? t("localPool.usage.success") : t("localPool.usage.failed")}
                        </span>
                      </td>
                      <td><code>{entry.resolvedModel ?? entry.requestedModel ?? "-"}</code></td>
                      <td>{t("localPool.usage.milliseconds", { value: entry.latencyMs })}</td>
                      <td>{entry.totalTokens ?? "-"}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          ) : (
            <p className="usage-empty">{t("localPool.usage.empty")}</p>
          )}
        </section>
      </div>
    </section>
  );
}

function ActionButton({
  action,
  className,
  current,
  disabled,
  icon,
  label,
  loadingLabel,
  onClick,
  submit = false,
  title,
}: {
  action: Action | null;
  className: string;
  current: Action;
  disabled?: boolean;
  icon: ReactNode;
  label: string;
  loadingLabel: string;
  onClick?: () => void;
  submit?: boolean;
  title?: string;
}) {
  const loading = action === current;
  return (
    <button
      className={`local-action ${className}`}
      type={submit ? "submit" : "button"}
      disabled={disabled || action !== null}
      onClick={onClick}
      title={title}
    >
      {loading ? <Loader2 className="spin" aria-hidden /> : icon}
      <span>{loading ? loadingLabel : label}</span>
    </button>
  );
}

function commandErrorCode(error: unknown) {
  if (typeof error === "object" && error && "code" in error && typeof error.code === "string") {
    const supported = new Set([
      "conflict",
      "gateway_unavailable",
      "io",
      "invalid_state",
      "not_found",
      "profile_restore_blocked",
      "recovery_required",
      "secret_store_unavailable",
      "unsupported_schema",
    ]);
    if (supported.has(error.code)) return error.code;
  }
  return "general";
}

function warningTranslationKey(warning: string) {
  if (warning === "gateway_configured_but_not_running" || warning === "usage_persistence_failed") {
    return `localPool.warnings.${warning}`;
  }
  return "localPool.warnings.general";
}

function formatDate(value: string | null, locale: string, fallback: string) {
  if (!value) return fallback;
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return fallback;
  return new Intl.DateTimeFormat(locale, {
    dateStyle: "short",
    timeStyle: "short",
  }).format(date);
}
