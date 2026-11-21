import { useEffect, useRef, useState } from "react";
import { ControlError, uuidText } from "../../api/control";
import { beginCustomer, logoutCustomer, readCustomer } from "../../api/customer";
import { useLocale } from "../../i18n/LocaleProvider";
import { customerPages, type CustomerPage } from "../app/routes";
import { EntryFrame } from "./EntryFrame";
import { CustomerWorkspace } from "./CustomerWorkspace";

type EntryProblem = "denied" | "unavailable" | "busy";

export function CustomerEntry({ brand, provider, login, page, support }: {
  brand: string; provider: string; login: boolean; page: CustomerPage; support: string | undefined;
}) {
  const { t } = useLocale();
  const [fragment] = useState(() => location.hash.slice(1));
  const candidate = login && fragment.startsWith("review=") && uuidText.test(fragment.slice(7)) ? fragment.slice(7) : "";
  const [problem, setProblem] = useState<EntryProblem | undefined>(
    login && (fragment === "denied" || fragment === "unavailable" || fragment === "busy") ? fragment : undefined,
  );
  const [account, setAccount] = useState<Awaited<ReturnType<typeof readCustomer>>>();
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
      else setProblem("unavailable");
    }).finally(() => clearTimeout(timer));
    return () => { active = false; clearTimeout(timer); c.abort(); };
  }, [login, reload]);
  async function act() {
    if (running.current) return;
    running.current = true; setBusy(true); setProblem(undefined);
    try {
      if (login) await beginCustomer(provider);
      else if (account) { await logoutCustomer(account.csrf); setAccount(undefined); location.replace("/login"); }
      else setReload(n => n + 1);
    } catch (err) {
      if (!login && err instanceof ControlError && err.status === 401) location.replace("/login");
      else setProblem(login && err instanceof ControlError && err.status === 429 ? "busy" : "unavailable");
    } finally { running.current = false; setBusy(false); }
  }
  if (!login && account) return <CustomerWorkspace key={account.workspaceId} brand={brand} page={page}
    account={account} support={support} signingOut={busy} failed={problem === "unavailable"} onSignOut={() => void act()}
    refresh={reload} onRefresh={() => setReload(n => n + 1)} />;

  return <EntryFrame brand={brand}>
    <h1>{t(login ? candidate ? "customer.review" : "customer.signIn" : customerPages.find(p => p.id === page)!.name)}</h1>
    {candidate && <><p>{t("customer.shareReview")}</p><code className="entry-review-id">{candidate}</code></>}
    {!login && !problem && <p role="status">{t("unavailable.connecting")}</p>}
    {problem && <p role="alert">{t(problem === "denied" ? "customer.notApproved"
      : problem === "busy" ? "customer.rateLimited" : "unavailable.server")}</p>}
    {(login || problem) && <button type="button" className={`key-button ${login ? "key-primary" : "key-secondary"}`}
      disabled={busy} onClick={() => void act()}>{t(login ? "customer.continue" : "common.retry")}</button>}
  </EntryFrame>;
}
