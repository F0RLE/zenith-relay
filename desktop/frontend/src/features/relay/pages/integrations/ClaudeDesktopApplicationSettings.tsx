import { ApplicationLaunchSettings } from "./ApplicationLaunchPath";
import { ApplicationProtocolControl } from "./ApplicationProtocolControl";
import { ClaudeDesktopConnectionControl } from "./ClaudeDesktopConnectionControl";

export function ClaudeDesktopApplicationSettings() {
  return <section className="gateway-tab-panel" role="tabpanel" aria-label="Claude Desktop">
    <ApplicationLaunchSettings application="claude-desktop" />
    <div className="gateway-settings-panel">
      <ClaudeDesktopConnectionControl />
      <ApplicationProtocolControl application="claude-desktop" />
    </div>
  </section>;
}
