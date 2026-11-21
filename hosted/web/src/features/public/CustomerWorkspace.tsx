import { useEffect, useState } from "react";
import { uuidText } from "../../api/control";
import type { readCustomer } from "../../api/customer";
import { UserRound, Wallet } from "../../components/Icons";
import { useTheme } from "../../components/useTheme";
import { useLocale } from "../../i18n/LocaleProvider";
import { HostedShell, type HostedNavItem } from "../app/HostedShell";
import { customerAccountPages, customerMainPages, customerPages, type CustomerPage } from "../app/routes";
import { OwnTopUps } from "../billing/OwnTopUps";
import { OwnKeys } from "../keys/OwnKeys";
import { PublishedModels } from "../models/PublishedModels";
import { CustomerInbox } from "../notifications/CustomerInbox";
import { CustomerOverview } from "../overview/CustomerOverview";
import { StatusPage } from "../status/StatusPage";
import { OwnUsage } from "../usage/OwnUsage";
import { OwnWalletHistory } from "../usage/OwnWalletHistory";
import { formatMoney } from "../usage/model";
import { AccountSessions, accountPanelUrl } from "./AccountSessions";
import "./customer-workspace.css";

type Account = Awaited<ReturnType<typeof readCustomer>>;

export function CustomerWorkspace({ brand, page, account, support, signingOut, failed, onSignOut, refresh, onRefresh }: {
  brand: string; page: CustomerPage; account: Account; support: string | undefined;
  signingOut: boolean; failed: boolean; onSignOut: () => void; refresh: number; onRefresh: () => void;
}) {
  const { t, locale } = useLocale();
  const appearance = useTheme();
  const [query] = useState(() => new URLSearchParams(location.search));
  const [usageOverview, setUsageOverview] = useState<string>();
  const security = page === "overview" && query.get("panel") === "account";
  const title = t(security ? "sessions.accountSecurity" : page === "models" ? "modelCatalog.title"
    : customerPages.find(p => p.id === page)!.name);
  const report = page === "usage" ? query.get("report") : null;
  const overviewUrl = report && uuidText.test(report) ? "/app/overview?" + new URLSearchParams(
    [...query].filter(([key]) => key === "report" || key === "hours" || key === "model"),
  ) : "/app/overview";
  const navItem = ({ id, name }: (typeof customerPages)[number]): HostedNavItem => ({
    id, name, href: id === "overview" ? usageOverview ?? overviewUrl : `/app/${id}`,
    current: id === page ? security ? "location" : "page" : undefined,
  });
  const balance = formatMoney(account.available, account.available % 10000n === 0n ? 2 : 6, locale);
  const balanceLabel = t("shell.balanceAmount", { amount: balance });

  useEffect(() => { document.title = `${title} · ${brand}`; }, [title, brand]);

  return <HostedShell brand={brand} page={security ? "account" : page} serverWorkspace="customer" appearance={appearance}
    navigation={customerMainPages.map(navItem)} accountNavigation={customerAccountPages.map(navItem)}
    signingOut={signingOut} onSignOut={onSignOut} beforeLanguage={languageOpen => <>
      <a className="hosted-balance" href="/app/billing" aria-label={balanceLabel} data-relay-tooltip={balanceLabel}>
        <Wallet aria-hidden="true" /><span>${balance}</span></a>
      <a href={accountPanelUrl} className="icon-button customer-account-button" aria-label={t("sessions.accountSecurity")}
        data-relay-tooltip={t("sessions.accountSecurity")} aria-current={security ? "page" : undefined}><UserRound /></a>
      <CustomerInbox csrf={account.csrf} languageOpen={languageOpen} />
    </>}>
    {(page === "usage" || page === "keys" || page === "billing") && <header className="page-heading customer-page-heading"><h1>{title}</h1></header>}
    {failed && <p className="customer-workspace-error" role="alert">{t("unavailable.server")}</p>}
    {security ? <AccountSessions workspace={account.workspaceId} csrf={account.csrf} support={support} />
      : page === "overview" && <CustomerOverview csrf={account.csrf} />}
    {page === "keys" && <OwnKeys csrf={account.csrf} workspace={account.workspaceId} />}
    {page === "models" && <PublishedModels />}
    {page === "status" && <StatusPage live />}
    {page === "usage" && <OwnUsage csrf={account.csrf} onOverview={setUsageOverview} />}
    {page === "billing" && <>
      <dl className="entry-wallet"><dt>{t("billing.availableBalance")}</dt><dd>{balance} USD</dd>
        {account.reserved > 0n && <><dt>{t("refund.totalReserved")}</dt><dd>{formatMoney(account.reserved, 6, locale)} USD</dd></>}
      </dl>
      {account.reserved > 0n && <dl className="wallet-reserve-breakdown">
        {account.inference > 0n && <><dt>{t("refund.inferenceReserved")}</dt><dd>{formatMoney(account.inference, 6, locale)} USD</dd></>}
        {account.refund > 0n && <><dt>{t("refund.refundReserved")}</dt><dd>{formatMoney(account.refund, 6, locale)} USD</dd></>}
      </dl>}
      {!query.has("id") && <OwnTopUps csrf={account.csrf} workspace={account.workspaceId} onRefresh={onRefresh} />}
      <OwnWalletHistory refresh={refresh} />
    </>}
  </HostedShell>;
}
