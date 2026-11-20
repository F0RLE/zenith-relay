use serde::Deserialize;

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Diagnostics {
    pub capture: Option<CaptureSettings>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaptureSettings {
    default_seconds: Option<i32>,
    max_seconds: Option<i32>,
    max_events: Option<i32>,
    max_bytes: Option<i32>,
    max_events_per_second: Option<i32>,
}

#[derive(Clone, Copy, serde::Serialize)]
pub struct CaptureLimits {
    pub default_seconds: i32,
    pub max_seconds: i32,
    pub max_events: i32,
    pub max_bytes: i32,
    pub max_events_per_second: i32,
}

impl Diagnostics {
    pub fn limits(&self) -> Option<CaptureLimits> {
        let c = self.capture.as_ref()?;
        Some(CaptureLimits {
            default_seconds: c.default_seconds.unwrap_or(600),
            max_seconds: c.max_seconds.unwrap_or(1800),
            max_events: c.max_events?,
            max_bytes: c.max_bytes?,
            max_events_per_second: c.max_events_per_second?,
        })
    }

    pub(super) fn validate(&self) -> Result<(), &'static str> {
        let Some(c) = &self.capture else {
            return Ok(());
        };
        let max = c.max_seconds.unwrap_or(1800);
        if !(1..=1800).contains(&max)
            || !(1..=max).contains(&c.default_seconds.unwrap_or(600))
            || c.max_events.is_some_and(|n| !(1..=128).contains(&n))
            || c.max_bytes.is_some_and(|n| !(128..=32768).contains(&n))
            || c.max_events_per_second
                .is_some_and(|n| !(1..=16).contains(&n))
        {
            return Err("invalid diagnostic capture limits");
        }
        Ok(())
    }
}
