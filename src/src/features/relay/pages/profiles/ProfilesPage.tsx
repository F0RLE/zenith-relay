import type { ComponentType } from "react";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import { EmptyState, PageHeader, Tabs } from "../../components/Ui";
import { useRelayState } from "../../state/RelayStateProvider";
import { ChatGptRecoveryHeaderAction, ChatGptRecoveryTab } from "./recovery/ChatGptRecoveryTab";
import { OpenCodeRecoveryHeaderAction, OpenCodeRecoveryTab } from "./recovery/OpenCodeRecoveryTab";

type RecoveryTab = "chatgpt" | "opencode";

type RecoveryApplication = {
  id: RecoveryTab;
  labelKey: "profiles.tabs.chatgpt" | "profiles.tabs.opencode";
  Content: ComponentType;
  HeaderAction?: ComponentType;
};

// A new integration adds one application adapter and one entry here.
const RECOVERY_APPLICATIONS: readonly [RecoveryApplication, ...RecoveryApplication[]] = [
  { id: "chatgpt", labelKey: "profiles.tabs.chatgpt", Content: ChatGptRecoveryTab, HeaderAction: ChatGptRecoveryHeaderAction },
  { id: "opencode", labelKey: "profiles.tabs.opencode", Content: OpenCodeRecoveryTab, HeaderAction: OpenCodeRecoveryHeaderAction },
];

export function ProfilesPage() {
  const { t } = useTranslation();
  const { mode } = useRelayState();
  const [activeTab, setActiveTab] = useState<RecoveryTab>("chatgpt");
  const application = RECOVERY_APPLICATIONS.find(({ id }) => id === activeTab) ?? RECOVERY_APPLICATIONS[0];
  const HeaderAction = application.HeaderAction;
  const Content = application.Content;

  return <section className="relay-page relay-workspace-page profile-recovery-page">
    <PageHeader title={t("nav.profiles")} navigation={<Tabs value={activeTab} onChange={(selectedTab) => setActiveTab(selectedTab as RecoveryTab)} label={t("profiles.tabs.label")} items={RECOVERY_APPLICATIONS.map(({ id: recoveryTabId, labelKey }) => ({ id: recoveryTabId, label: t(labelKey) }))} />} actions={mode === "local" && HeaderAction ? <HeaderAction key={application.id} /> : null} />
    {mode === "local" ? <Content key={application.id} /> : <EmptyState title={t("profiles.localOnlyTitle")} description={t("profiles.localOnlyDescription")} />}
  </section>;
}
