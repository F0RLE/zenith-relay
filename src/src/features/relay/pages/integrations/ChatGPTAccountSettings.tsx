import { RequestTimezoneControl } from "../connections/RequestTimezoneControl";
import { DegradedRoutesControl } from "./DegradedRoutesControl";

export function ChatGPTAccountSettings() {
  return <div className="integration-account-settings">
    <RequestTimezoneControl />
    <DegradedRoutesControl />
  </div>;
}
