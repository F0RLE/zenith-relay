import { useState } from "react";
import { useTranslation } from "react-i18next";
import { PageHeader, Tabs } from "../../components/Ui";
import { DegradedRoutesControl } from "./DegradedRoutesControl";
import { ChatGPTApplicationSettings } from "./ChatGPTApplicationSettings";
import { OpenCodeApplicationSettings } from "./OpenCodeApplicationSettings";

const applications = [
  {
    id: "chatgpt",
    label: "ChatGPT",
    Component: ChatGPTApplicationSettings,
  },
  {
    id: "opencode",
    label: "OpenCode",
    Component: OpenCodeApplicationSettings,
  },
] as const;

const accountProviders = [
  { id: "chatgpt", label: "ChatGPT", Component: ChatGPTAccountSettings },
] as const;

function ChatGPTAccountSettings() {
  return <section className="gateway-tab-panel" role="tabpanel" aria-label="ChatGPT">
    <div className="gateway-workspace"><DegradedRoutesControl /></div>
  </section>;
}

export function IntegrationsPage() {
  const { t } = useTranslation();
  const [section, setSection] = useState("applications");
  const [applicationId, setApplicationId] = useState("chatgpt");
  const [accountProviderId, setAccountProviderId] = useState("chatgpt");
  const accountSettings = section === "accounts";
  const application = applications.find((item) => item.id === applicationId) ?? applications[0];
  const provider = accountProviders.find((item) => item.id === accountProviderId) ?? accountProviders[0];
  const Settings = accountSettings ? provider.Component : application.Component;
  const categoryLabel = t(accountSettings ? "integrations.accounts" : "integrations.applications");

  return <section className="relay-page relay-workspace-page integrations-page">
    <PageHeader
      title={t("nav.integrations")}
      navigation={<Tabs
        value={section}
        onChange={setSection}
        label={t("integrations.sections")}
        items={[
          { id: "applications", label: t("integrations.applications") },
          { id: "accounts", label: t("integrations.accounts") },
        ]}
      />}
    />
    <section className="integrations-category" role="tabpanel" aria-label={categoryLabel}>
      <div className="integrations-toolbar">
        <Tabs
          value={accountSettings ? provider.id : application.id}
          onChange={accountSettings ? setAccountProviderId : setApplicationId}
          label={t(accountSettings ? "integrations.accountProviders" : "integrations.applicationClients")}
          items={(accountSettings ? accountProviders : applications).map(({ id, label }) => ({ id, label }))}
        />
      </div>
      <div className="integrations-settings"><Settings /></div>
    </section>
  </section>;
}
