use std::time::Duration;

pub(super) const CONFIRMATION_INTERVAL: Duration = Duration::from_secs(5);
const STABLE_INTERVAL: Duration = Duration::from_secs(30);
const UNAVAILABLE_INTERVAL: Duration = Duration::from_secs(60);

#[derive(Clone, Copy, Eq, PartialEq)]
pub(super) enum Observation {
    Unavailable,
    Available,
    LoginRequired,
}

pub(super) struct PollSchedule {
    previous: Option<Observation>,
    delay: Duration,
}

impl Default for PollSchedule {
    fn default() -> Self {
        Self {
            previous: None,
            delay: CONFIRMATION_INTERVAL,
        }
    }
}

impl PollSchedule {
    pub(super) fn confirming(&mut self) -> Duration {
        *self = Self::default();
        CONFIRMATION_INTERVAL
    }

    pub(super) fn observed(&mut self, observation: Observation) -> Duration {
        let maximum = match observation {
            Observation::Unavailable => UNAVAILABLE_INTERVAL,
            Observation::Available | Observation::LoginRequired => STABLE_INTERVAL,
        };
        self.delay = if self.previous == Some(observation) {
            self.delay.saturating_mul(2).min(maximum)
        } else {
            CONFIRMATION_INTERVAL
        };
        self.previous = Some(observation);
        self.delay
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_and_stable_observations_back_off_with_bounded_detection_delay() {
        for (observation, maximum) in [
            (Observation::Unavailable, UNAVAILABLE_INTERVAL),
            (Observation::Available, STABLE_INTERVAL),
            (Observation::LoginRequired, STABLE_INTERVAL),
        ] {
            let mut schedule = PollSchedule::default();
            assert_eq!(schedule.observed(observation), CONFIRMATION_INTERVAL);
            for _ in 0..12 {
                assert!(schedule.observed(observation) <= maximum);
            }
            assert_eq!(schedule.observed(observation), maximum);
        }
    }

    #[test]
    fn profile_and_auth_changes_return_to_fast_confirmation() {
        let mut schedule = PollSchedule::default();
        for _ in 0..8 {
            schedule.observed(Observation::Available);
        }
        assert_eq!(
            schedule.observed(Observation::LoginRequired),
            CONFIRMATION_INTERVAL
        );
        for _ in 0..8 {
            schedule.observed(Observation::Unavailable);
        }
        assert_eq!(schedule.confirming(), CONFIRMATION_INTERVAL);
        assert_eq!(
            schedule.observed(Observation::Available),
            CONFIRMATION_INTERVAL
        );
    }
}
