import { QuickSetupWizard } from "../onboarding/QuickSetupWizard";
import { RelayShell } from "../shell/RelayShell";
import { RelayStateProvider, useRelayState } from "../state/RelayStateProvider";
import { ConfirmProvider } from "../components/Ui";

export function RelayApp() {
  return <ConfirmProvider><RelayStateProvider><RelaySurface /></RelayStateProvider></ConfirmProvider>;
}

function RelaySurface() {
  const { onboardingComplete } = useRelayState();
  return onboardingComplete ? <RelayShell /> : <QuickSetupWizard />;
}
