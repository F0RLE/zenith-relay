import type { ReactNode } from "react";
import { LanguageSwitcher } from "../../components/LanguageSwitcher";
import { RelayMark } from "../../components/RelayMark";
import { useTheme } from "../../components/useTheme";

export function EntryFrame({ brand, children, workspace, utilities }: { brand: string; children: ReactNode; workspace?: boolean; utilities?: ReactNode }) {
  const { theme } = useTheme();
  return <div className={workspace ? "relay-app landing hosted-entry hosted-app hosted-report-entry" : "relay-app landing hosted-entry"}
    data-theme={theme} style={{ colorScheme: theme }}>
    <header className="hosted-entry-header"><a href="/" className="landing-brand"><RelayMark className="landing-mark" /><strong>{brand}</strong></a>
      <div className="entry-header-actions">{utilities}<LanguageSwitcher /></div></header>
    <main className="integration-unavailable hosted-entry-content">{children}</main>
  </div>;
}
