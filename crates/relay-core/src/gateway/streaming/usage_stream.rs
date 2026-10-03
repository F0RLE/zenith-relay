use super::*;

mod ingest;

pub(super) struct UsageStream<S> {
    pub(super) inner: Pin<Box<S>>,
    pub(super) runtime: Option<Arc<GatewayRuntime>>,
    pub(super) expected_model: Option<String>,
    pub(super) callback: crate::UsageCallback,
    pub(super) completion: CompletionCallback,
    pub(super) event: Option<UsageEvent>,
    pub(super) response_id: Option<String>,
    pub(super) native_response: Option<Arc<Mutex<Option<Value>>>>,
    pub(super) native_gemini: bool,
    native_gemini_incomplete: bool,
    native_gemini_finished: bool,
    native_replay_capture: NativeReplayCapture,
    pub(super) cooldown_hint: RateLimitBodyHint,
    pub(super) started: Instant,
    pub(super) sse_pending: Vec<u8>,
    pub(super) output_pending: VecDeque<Bytes>,
    // Track yielded bytes, not parsed deltas: even an incomplete SSE frame is
    // already owned by the client and cannot be replaced by a synthetic response.
    client_visible_output: bool,
    pub(super) heartbeat: Pin<Box<Sleep>>,
    pub(super) terminated: bool,
}

impl<S> UsageStream<S> {
    fn assemble(
        stream: S,
        runtime: Option<Arc<GatewayRuntime>>,
        callback: crate::UsageCallback,
        event: UsageEvent,
        started: Instant,
        completion: CompletionCallback,
        native_response: Option<Arc<Mutex<Option<Value>>>>,
    ) -> Self {
        let native_gemini = event.wire_api == WireApi::Gemini;
        Self {
            inner: Box::pin(stream),
            runtime,
            expected_model: None,
            callback,
            completion,
            event: Some(event),
            response_id: None,
            native_response,
            native_gemini,
            native_gemini_incomplete: false,
            native_gemini_finished: false,
            native_replay_capture: NativeReplayCapture::default(),
            cooldown_hint: RateLimitBodyHint::default(),
            started,
            sse_pending: Vec::new(),
            output_pending: VecDeque::new(),
            client_visible_output: false,
            heartbeat: Box::pin(sleep(SSE_HEARTBEAT_INTERVAL)),
            terminated: false,
        }
    }

    #[cfg(test)]
    pub(super) fn new(
        stream: S,
        callback: crate::UsageCallback,
        event: UsageEvent,
        started: Instant,
        completion: CompletionCallback,
    ) -> Self {
        Self::assemble(stream, None, callback, event, started, completion, None)
    }

    pub(super) fn with_runtime(
        stream: S,
        runtime: Arc<GatewayRuntime>,
        event: UsageEvent,
        started: Instant,
        completion: CompletionCallback,
        native_response: Option<Arc<Mutex<Option<Value>>>>,
    ) -> Self {
        let callback = runtime.usage.clone();
        Self::assemble(
            stream,
            Some(runtime),
            callback,
            event,
            started,
            completion,
            native_response,
        )
    }
}

impl<S, E> Stream for UsageStream<S>
where
    S: Stream<Item = std::result::Result<Bytes, E>>,
{
    type Item = std::result::Result<Bytes, E>;

    fn poll_next(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.as_mut().get_mut();
        loop {
            if let Some(bytes) = this.output_pending.pop_front() {
                return Poll::Ready(Some(Ok(bytes)));
            }
            if this.terminated {
                return Poll::Ready(None);
            }
            match this.inner.as_mut().poll_next(context) {
                Poll::Ready(Some(Ok(bytes))) => {
                    let now = TokioInstant::now();
                    this.heartbeat.as_mut().reset(now + SSE_HEARTBEAT_INTERVAL);
                    let (valid, forward_len) = if this.native_gemini {
                        (this.ingest_native_gemini(&bytes), bytes.len())
                    } else {
                        this.ingest_sse(&bytes)
                    };
                    if let Some(failure) = this.output_pending.pop_front() {
                        return Poll::Ready(Some(Ok(failure)));
                    }
                    if !valid {
                        return Poll::Ready(None);
                    }
                    let forwarded = bytes.slice(..forward_len);
                    this.client_visible_output |= !forwarded.is_empty();
                    if !forwarded.is_empty() {
                        return Poll::Ready(Some(Ok(forwarded)));
                    }
                }
                Poll::Ready(Some(Err(error))) => {
                    if this.fail_stream(error_codes::UPSTREAM_STREAM) {
                        continue;
                    }
                    return Poll::Ready(Some(Err(error)));
                }
                Poll::Ready(None) => {
                    if this.native_gemini {
                        if !this.sse_pending.is_empty() {
                            this.fail_stream(error_codes::STREAM_INCOMPLETE);
                        } else if this.native_gemini_incomplete {
                            this.finish(Some(false), Some(error_codes::RESPONSE_INCOMPLETE));
                        } else if this.native_gemini_finished {
                            this.finish(Some(true), None);
                        } else {
                            this.fail_stream(error_codes::STREAM_INCOMPLETE);
                        }
                        this.sse_pending.clear();
                        this.terminated = true;
                        return Poll::Ready(None);
                    }
                    if this.event.as_ref().is_some_and(|event| event.success) {
                        if this.fail_stream(error_codes::STREAM_INCOMPLETE) {
                            continue;
                        }
                    } else {
                        this.finish(None, None);
                    }
                    this.sse_pending.clear();
                    this.terminated = true;
                    return Poll::Ready(None);
                }
                Poll::Pending => {
                    // Keep the client connection alive without imposing a
                    // deadline on the provider's next output.
                    // Chunks are forwarded immediately, so a heartbeat is safe
                    // only between complete SSE events, never inside a frame.
                    if this.sse_pending.is_empty()
                        && this.heartbeat.as_mut().poll(context).is_ready()
                    {
                        this.heartbeat
                            .as_mut()
                            .reset(TokioInstant::now() + SSE_HEARTBEAT_INTERVAL);
                        return Poll::Ready(Some(Ok(Bytes::from_static(SSE_HEARTBEAT))));
                    }
                    return Poll::Pending;
                }
            }
        }
    }
}

impl<S> Drop for UsageStream<S> {
    fn drop(&mut self) {
        self.finish(Some(false), Some(error_codes::CLIENT_CANCELLED));
    }
}
