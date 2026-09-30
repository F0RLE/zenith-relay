use super::*;
use crate::gateway::test_support::test_usage_event;
use std::convert::Infallible;
use std::sync::Mutex;

mod sse_bootstrap;
mod terminal_markers;
mod usage_forwarding;

fn usage_stream_with_events<S>(input: S, events: Arc<Mutex<Vec<UsageEvent>>>) -> UsageStream<S>
where
    S: Stream<Item = Result<Bytes, Infallible>>,
{
    let captured = events.clone();
    UsageStream::new(
        input,
        Arc::new(move |event| captured.lock().unwrap().push(event)),
        test_usage_event(),
        Instant::now(),
        Arc::new(|_, _, _| {}),
    )
}
