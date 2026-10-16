import { useState } from "react";
import { Radio, SquareTerminal, UserRound } from "lucide-react";
import { useTranslation } from "react-i18next";
import { PageHeader, Tabs } from "../../components/Ui";
import { useRelayState } from "../../state/RelayStateProvider";
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
  const { mode } = useRelayState();
  const [section, setSection] = useState("api");
  const [applicationId, setApplicationId] = useState("chatgpt");
  const [accountProviderId, setAccountProviderId] = useState("chatgpt");
  const apiLabel = t(mode === "remote" ? "integrations.serverApi" : "integrations.localApi");

  return <section className="relay-page relay-workspace-page integrations-page">
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
