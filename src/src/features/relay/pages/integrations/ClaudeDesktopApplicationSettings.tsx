import { ApplicationLaunchPath } from "./ApplicationLaunchPath";

export function ClaudeDesktopApplicationSettings() {
  return <section className="gateway-tab-panel" role="tabpanel" aria-label="Claude Desktop">
    <ApplicationLaunchPath application="claude-desktop" />
  </section>;
}
