import { relayCommands } from "../api/commands";

export function useClaudeDesktopLaunch() {
  const launch = async () => {
    await relayCommands.launchClaudeDesktop();
  };
  return { launch };
}
