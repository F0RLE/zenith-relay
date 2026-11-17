import { ApplicationLaunchPath } from "./ApplicationLaunchPath";
import { ApplicationProtocolControl } from "./ApplicationProtocolControl";

export function ClaudeDesktopApplicationSettings() {
  return <section className="gateway-tab-panel" role="tabpanel" aria-label="Claude Desktop">
    <ApplicationLaunchPath application="claude-desktop" />
    <div className="gateway-settings-panel"><ApplicationProtocolControl application="claude-desktop" /></div>
  </section>;
}
