import { useEffect, useRef, useState } from "react";
import { ControlError, uuidText } from "../../api/control";
import { beginCustomer, logoutCustomer, readCustomer } from "../../api/customer";
import { useLocale } from "../../i18n/LocaleProvider";
import { customerPages, type CustomerPage } from "../app/routes";
import { formatMoney } from "../usage/model";
import { EntryFrame } from "./EntryFrame";
import { OwnKeys } from "../keys/OwnKeys";
import { CustomerOverview } from "../overview/CustomerOverview";
import { OwnUsage } from "../usage/OwnUsage";
import { OwnWalletHistory } from "../usage/OwnWalletHistory";
import { OwnTopUps } from "../billing/OwnTopUps";
import { PublishedModels } from "../models/PublishedModels";
import { StatusPage } from "../status/StatusPage";
import { CustomerInbox } from "../notifications/CustomerInbox";

export function CustomerEntry({ brand, provider, login, page }: { brand: string; provider: string; login: boolean; page: CustomerPage }) {
  const { t, locale } = useLocale();
  const [fragment] = useState(() => location.hash.slice(1));
  const candidate = login && fragment.startsWith("review=") && uuidText.test(fragment.slice(7)) ? fragment.slice(7) : "";
  const [error, setError] = useState(login && ["denied", "unavailable"].includes(fragment));
  const [account, setAccount] = useState<Awaited<ReturnType<typeof readCustomer>>>();
  const [usageOverview, setUsageOverview] = useState<string>();
  const [busy, setBusy] = useState(false);
  const [reload, setReload] = useState(0);
  const running = useRef(false);
  useEffect(() => {
    if (location.hash) history.replaceState(null, "", location.pathname + location.search);
    const restore = (e: PageTransitionEvent) => { if (e.persisted) location.reload(); };
    window.addEventListener("pageshow", restore);
    return () => window.removeEventListener("pageshow", restore);
  }, []);
  useEffect(() => {
    if (login) return;
    const c = new AbortController();
    const timer = setTimeout(() => c.abort(), 10000);
    let active = true;
    void readCustomer(c.signal).then(v => { if (active) setAccount(v); }).catch(err => {
      if (!active) return;
      if (err instanceof ControlError && err.status === 401) location.replace("/login");
      else setError(true);
    }).finally(() => clearTimeout(timer));
    return () => { active = false; clearTimeout(timer); c.abort(); };
  }, [login, reload]);
  async function act() {
    if (running.current) return;
    running.current = true; setBusy(true); setError(false);
    try {
      if (login) await beginCustomer(provider);
      else if (account) { await logoutCustomer(account.csrf); setAccount(undefined); location.replace("/login"); }
      else setReload(n => n + 1);
    } catch (err) {
      if (!login && err instanceof ControlError && err.status === 401) location.replace("/login");
      else setError(true);
    } finally { running.current = false; setBusy(false); }
  }
  const overview = !!account && page === "overview";
  const status = !!account && page === "status";
  const requestQuery = new URLSearchParams(location.search);
  const report = page === "usage" ? requestQuery.get("report") : null;
  const overviewUrl = report && uuidText.test(report) ? "/app/overview?" + new URLSearchParams(
    [...requestQuery].filter(([k]) => k === "report" || k === "hours" || k === "model"),
  ) : "/app/overview";
  return <EntryFrame brand={brand} workspace={overview || status}
    utilities={account && <CustomerInbox key={account.workspaceId} csrf={account.csrf} />}>
    {!overview && !status && <h1>{t(login ? candidate ? "customer.review" : "customer.signIn" : customerPages.find(p => p.id === page)!.name)}</h1>}
    {candidate && <><p>{t("customer.shareReview")}</p><code className="entry-review-id">{candidate}</code></>}
    {!login && !account && !error && <p role="status">{t("unavailable.connecting")}</p>}
    {overview && <CustomerOverview key={account.workspaceId} csrf={account.csrf} />}
    {account && (page==="overview" || page==="billing") && <dl className="entry-wallet"><dt>{t("billing.availableBalance")}</dt>
      <dd>{formatMoney(account.available, account.available % 10000n === 0n ? 2 : 6, locale)} USD</dd>
      {account.reserved>0n && <><dt>{t("refund.totalReserved")}</dt><dd>{formatMoney(account.reserved,6,locale)} USD</dd></>}</dl>}
    {account && page==="billing" && account.reserved>0n && <dl className="wallet-reserve-breakdown">
      {account.inference>0n && <><dt>{t("refund.inferenceReserved")}</dt><dd>{formatMoney(account.inference,6,locale)} USD</dd></>}
      {account.refund>0n && <><dt>{t("refund.refundReserved")}</dt><dd>{formatMoney(account.refund,6,locale)} USD</dd></>}
    </dl>}
    {account && page==="keys" && <OwnKeys key={account.workspaceId} csrf={account.csrf} workspace={account.workspaceId} />}
    {account && page==="models" && <PublishedModels key={account.workspaceId} />}
    {status && <StatusPage key={account.workspaceId} live />}
    {account && page==="usage" && <OwnUsage key={account.workspaceId} csrf={account.csrf} onOverview={setUsageOverview} />}
    {account && page==="billing" && <>
      {!requestQuery.has("id") && <OwnTopUps key={account.workspaceId} csrf={account.csrf}
        workspace={account.workspaceId} onRefresh={() => setReload(n => n + 1)} />}
      <OwnWalletHistory key={account.workspaceId} refresh={reload} />
    </>}
    {account && <nav className="entry-links" aria-label={t("nav.overview")}>{customerPages
      .filter(p => p.id !== page).map(p => <a className="entry-account-link" key={p.id}
        href={p.id === "overview" ? usageOverview ?? overviewUrl : "/app/"+p.id}>{t(p.name)}</a>)}</nav>}
    {error && <p role="alert">{t(login && fragment === "denied" ? "customer.notApproved" : "unavailable.server")}</p>}
    {(login || account || error) && <button type="button" className={`key-button ${login ? "key-primary" : "key-secondary"}`}
      disabled={busy} onClick={() => void act()}>{t(login ? "customer.continue" : account ? "shell.signOut" : "common.retry")}</button>}
  </EntryFrame>;
}
