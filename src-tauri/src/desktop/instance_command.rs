const SHUTDOWN_FOR_UPDATE: &str = "--shutdown-for-update";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum InstanceCommand {
    OpenWindow,
    ShutdownForUpdate(u32),
    InvalidShutdown,
}

impl InstanceCommand {
    pub(super) fn parse(args: &[String]) -> Self {
        let arguments = args.get(1..).unwrap_or_default();
        if !arguments.iter().any(|arg| arg == SHUTDOWN_FOR_UPDATE) {
            return Self::OpenWindow;
        }
        match arguments {
            [flag, pid] if flag == SHUTDOWN_FOR_UPDATE => pid
                .parse::<u32>()
                .ok()
                .filter(|pid| *pid > 0)
                .map_or(Self::InvalidShutdown, Self::ShutdownForUpdate),
            _ => Self::InvalidShutdown,
        }
    }

    pub(super) fn targets(self, process_id: u32) -> bool {
        matches!(self, Self::ShutdownForUpdate(target) if target == process_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> InstanceCommand {
        InstanceCommand::parse(&args.iter().map(|arg| (*arg).to_owned()).collect::<Vec<_>>())
    }

    #[test]
    fn ordinary_launches_keep_the_existing_window_behavior() {
        assert_eq!(parse(&["relay.exe"]), InstanceCommand::OpenWindow);
        assert_eq!(parse(&["relay.exe", "--tray"]), InstanceCommand::OpenWindow);
    }

    #[test]
    fn shutdown_requests_target_only_the_observed_process() {
        let command = parse(&["relay.exe", SHUTDOWN_FOR_UPDATE, "123"]);
        assert_eq!(command, InstanceCommand::ShutdownForUpdate(123));
        assert!(command.targets(123));
        assert!(!command.targets(124));
    }

    #[test]
    fn malformed_shutdown_requests_never_open_a_window_or_exit_a_primary() {
        for args in [
            vec!["relay.exe", SHUTDOWN_FOR_UPDATE],
            vec!["relay.exe", SHUTDOWN_FOR_UPDATE, "0"],
            vec!["relay.exe", SHUTDOWN_FOR_UPDATE, "-1"],
            vec!["relay.exe", SHUTDOWN_FOR_UPDATE, "4294967296"],
            vec!["relay.exe", SHUTDOWN_FOR_UPDATE, "not-a-pid"],
            vec!["relay.exe", SHUTDOWN_FOR_UPDATE, "123", "extra"],
            vec!["relay.exe", "--tray", SHUTDOWN_FOR_UPDATE, "123"],
        ] {
            let command = parse(&args);
            assert_eq!(command, InstanceCommand::InvalidShutdown);
            assert!(!command.targets(123));
        }
    }
}
