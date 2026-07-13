import { Activity, ArrowRight, CircleAlert, Copy, Play, Server, Square, Users } from "lucide-react";
import { useTranslation } from "react-i18next";
import { relayCommands } from "../../api/commands";
import { Button, EmptyState, PageHeader, StatusBadge, copyText } from "../../components/Ui";
import { useRelayState } from "../../state/RelayStateProvider";

export function OverviewPage() {
  const { t } = useTranslation();
  const { mode, runtime, readyState, readyStats, localUsage, remoteUsage, readyUsage, setPage, perform, busy } = useRelayState();
  const running = mode === "zenith" ? Boolean(readyState?.providerActive) : Boolean(runtime?.gateway.running);
  const endpoint = mode === "zenith" ? "https://api.zenithmarket.dev/v1" : runtime?.gateway.baseUrl ?? "";
  const poolUsage = mode === "remote" ? remoteUsage : localUsage;
  const requests = mode === "zenith" ? readyUsage.length : poolUsage.length;
  const models = mode === "zenith" ? 0 : runtime?.gateway.visibleModelIds.length ?? 0;
  const healthy = mode === "zenith" ? (running ? 1 : 0) : [...(runtime?.sources ?? []), ...(runtime?.accounts ?? [])].filter((item) => item.enabled).length;
  const errors = mode === "zenith" ? 0 : poolUsage.filter((item) => !item.success).length;
  const activity = mode === "zenith"
    ? readyUsage.slice(0, 5).map((item) => ({ id: item.id, success: item.status === "success", model: item.model, latency: item.responseTimeDisplay }))
    : poolUsage.slice(0, 5).map((item) => ({ id: item.id, success: item.success, model: item.resolvedModel ?? item.requestedModel, latency: `${item.latencyMs} ms` }));

  const primary = mode === "local" ? <Button variant="primary" busy={busy === "gateway"} icon={running ? <Square aria-hidden /> : <Play aria-hidden />} onClick={() => perform("gateway", () => running ? relayCommands.stopGateway() : relayCommands.startGateway(), running ? "feedback.stopped" : "feedback.started")}>{running ? t("gateway.stop") : t("gateway.start")}</Button> : mode === "remote" ? <Button variant="primary" icon={<Server aria-hidden />} onClick={() => setPage("connections")}>{runtime ? t("overview.openServer") : t("remote.connect")}</Button> : <Button variant="primary" icon={<ArrowRight aria-hidden />} onClick={() => setPage("connections")}>{running ? t("overview.openConnection") : t("readyApi.connect")}</Button>;

  return <section className="relay-page"><PageHeader title={t("nav.overview")} subtitle={t(`overview.subtitles.${mode}`)} actions={primary} />
    {!running && !runtime && mode !== "zenith" ? <EmptyState title={t("overview.emptyTitle")} description={t("overview.emptyDescription")} action={<Button variant="primary" onClick={() => setPage("connections")}>{t("overview.openConnections")}</Button>} /> : <>
      <div className="overview-status-band"><StatusBadge status={running ? "ready" : "warning"} label={running ? t("common.ready") : t("common.offline")} /><div><span>{t("gateway.endpoint")}</span><code>{endpoint || t("common.notConfigured")}</code></div>{endpoint ? <Button variant="ghost" icon={<Copy aria-hidden />} onClick={() => copyText(endpoint)}>{t("common.copy")}</Button> : null}<div><span>{mode === "zenith" ? t("readyApi.balance") : t("pool.capacity")}</span><strong>{mode === "zenith" ? readyStats?.balance ?? "-" : t("pool.membersCount", { count: runtime?.gateway.candidateCount ?? 0 })}</strong></div></div>
      <div className="metric-band"><div><Activity aria-hidden /><span>{t("overview.requestsToday")}</span><strong>{requests}</strong></div><div><Users aria-hidden /><span>{t("overview.healthy")}</span><strong>{healthy}</strong></div><div><ArrowRight aria-hidden /><span>{t("overview.models")}</span><strong>{models || "-"}</strong></div><div><CircleAlert aria-hidden /><span>{t("overview.errors")}</span><strong>{errors}</strong></div></div>
      <div className="overview-split"><section><h2>{t("overview.runtimeTitle")}</h2><dl className="detail-list"><div><dt>{t("common.mode")}</dt><dd>{t(`modes.${mode}`)}</dd></div><div><dt>{t("common.status")}</dt><dd>{running ? t("common.ready") : t("common.offline")}</dd></div><div><dt>{t("gateway.endpoint")}</dt><dd><code>{endpoint || "-"}</code></dd></div></dl><Button variant="secondary" onClick={() => setPage("gateway")}>{t("overview.openGateway")}</Button></section><section><h2>{mode === "zenith" ? t("connections.api") : t("overview.healthTitle")}</h2>{mode === "zenith" ? <dl className="detail-list"><div><dt>{t("common.status")}</dt><dd>{running ? t("common.connected") : t("common.notConfigured")}</dd></div><div><dt>{t("usage.requests")}</dt><dd>{readyStats?.requestsDisplay ?? readyStats?.requests ?? "-"}</dd></div><div><dt>{t("readyApi.balance")}</dt><dd>{readyStats?.balance ?? "-"}</dd></div></dl> : <dl className="detail-list"><div><dt>{t("connections.sources")}</dt><dd>{runtime?.sources.length ?? 0}</dd></div><div><dt>{t("connections.accounts")}</dt><dd>{runtime?.accounts.length ?? 0}</dd></div><div><dt>{t("connections.automations")}</dt><dd>{runtime?.automations.length ?? 0}</dd></div></dl>}<Button variant="secondary" onClick={() => setPage("connections")}>{t("overview.openConnections")}</Button></section></div>
      <section className="activity-section"><header><h2>{t("overview.activity")}</h2><Button variant="ghost" onClick={() => setPage("usage")}>{t("overview.viewUsage")}</Button></header>{activity.length ? <ul>{activity.map((item) => <li key={item.id}><StatusBadge status={item.success ? "ready" : "error"} label={item.success ? t("common.success") : t("common.failed")} /><code>{item.model ?? "-"}</code><span>{item.latency}</span></li>)}</ul> : <p className="muted">{t("usage.empty")}</p>}</section>
    </>}
  </section>;
}
