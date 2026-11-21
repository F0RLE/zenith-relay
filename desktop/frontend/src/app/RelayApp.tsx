import { ConfirmProvider } from "../features/relay/components/Ui";
import { QuickSetupWizard } from "../features/relay/onboarding/QuickSetupWizard";
import { RelayShell } from "../features/relay/shell/RelayShell";
import { RelayStateProvider, useRelayState } from "../features/relay/state/RelayStateProvider";

export function RelayApp() {
  return (
    <ConfirmProvider>
      <RelayStateProvider>
        <RelaySurface />
      </RelayStateProvider>
    </ConfirmProvider>
  );
}

function RelaySurface() {
  const { onboardingComplete } = useRelayState();
  return onboardingComplete ? <RelayShell /> : <QuickSetupWizard />;
}
