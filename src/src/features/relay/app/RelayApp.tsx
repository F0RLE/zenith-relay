import { QuickSetupWizard } from "../onboarding/QuickSetupWizard";
import { RelayShell } from "../shell/RelayShell";
import { RelayStateProvider, useRelayState } from "../state/RelayStateProvider";

export function RelayApp() {
  return <RelayStateProvider><RelaySurface /></RelayStateProvider>;
}

function RelaySurface() {
  const { onboardingComplete } = useRelayState();
  return onboardingComplete ? <RelayShell /> : <QuickSetupWizard />;
}
