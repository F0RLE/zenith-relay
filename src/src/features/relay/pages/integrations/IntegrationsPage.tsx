import { useLayoutEffect, useRef, useState } from "react";
import { Radio, SquareTerminal, UserRound } from "lucide-react";
import { useTranslation } from "react-i18next";
import { PageHeader, Tabs } from "../../components/Ui";
import { GatewaySettings } from "../gateway/GatewayPage";
import { IntegrationWorkspace, type IntegrationOption } from "./IntegrationWorkspace";
import { ChatGPTApplicationSettings } from "./ChatGPTApplicationSettings";
import { OpenCodeApplicationSettings } from "./OpenCodeApplicationSettings";
import { ClaudeDesktopApplicationSettings } from "./ClaudeDesktopApplicationSettings";
import { ChatGPTAccountSettings } from "./ChatGPTAccountSettings";

const applications = [
  {
    id: "chatgpt",
    label: "ChatGPT",
    icon: "/icons/chatgpt.svg",
    Component: ChatGPTApplicationSettings,
  },
  {
    id: "claude-desktop",
    label: "Claude Desktop",
    icon: "/icons/claude-desktop.svg",
    Component: ClaudeDesktopApplicationSettings,
  },
  {
    id: "opencode",
    label: "OpenCode",
    icon: "/icons/opencode.svg",
    Component: OpenCodeApplicationSettings,
  },
] as const satisfies readonly IntegrationOption[];

const accountProviders = [
  { id: "chatgpt", label: "ChatGPT", icon: "/icons/chatgpt.svg", Component: ChatGPTAccountSettings },
] as const satisfies readonly IntegrationOption[];

export function IntegrationsPage() {
  const { t } = useTranslation();
  const pageRef = useRef<HTMLElement>(null);
  const [section, setSection] = useState("api");
  const [applicationId, setApplicationId] = useState("chatgpt");
  const [accountProviderId, setAccountProviderId] = useState("chatgpt");
  const apiLabel = t("integrations.localApi");

  useLayoutEffect(() => {
    const page = pageRef.current;
    const header = page?.querySelector<HTMLElement>(":scope > .relay-workspace-header");
    if (!page || !header) return;
    const update = () => page.style.setProperty("--integration-header-height", `${header.getBoundingClientRect().height}px`);
    update();
    const observer = new ResizeObserver(update);
    observer.observe(header);
    return () => observer.disconnect();
  }, []);

  return <section ref={pageRef} className="relay-page relay-workspace-page integrations-page">
    <PageHeader
      title={t("nav.integrations")}
      navigation={<Tabs
        value={section}
        onChange={setSection}
        label={t("integrations.sections")}
        items={[
          { id: "api", label: apiLabel, icon: <Radio aria-hidden /> },
          { id: "applications", label: t("integrations.applications"), icon: <SquareTerminal aria-hidden /> },
          { id: "accounts", label: t("integrations.accounts"), icon: <UserRound aria-hidden /> },
        ]}
      />}
    />
    {section === "api" ? <section className="integrations-category" role="tabpanel" aria-label={apiLabel}>
      <GatewaySettings />
    </section> : null}
    {section === "applications" ? <section className="integrations-category" role="tabpanel" aria-label={t("integrations.applications")}>
      <IntegrationWorkspace
        value={applicationId}
        onChange={setApplicationId}
        label={t("integrations.applicationClients")}
        className="application-workspace"
        items={applications}
      />
    </section> : null}
    {section === "accounts" ? <section className="integrations-category" role="tabpanel" aria-label={t("integrations.accounts")}>
      <IntegrationWorkspace
        value={accountProviderId}
        onChange={setAccountProviderId}
        label={t("integrations.accountProviders")}
        showPickerWhenSingle
        items={accountProviders}
      />
    </section> : null}
  </section>;
}
