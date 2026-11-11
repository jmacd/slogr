// Copyright The OpenTelemetry Authors
// SPDX-License-Identifier: Apache-2.0

//! Subscriber integration without a runtime, queue, or global installation.

use crate::LogRecord;
use std::time::SystemTime;
use tracing::{Event, Subscriber};
use tracing_subscriber::layer::{Context, Layer};

/// Encodes events synchronously and delivers owned snapshots to a caller's sink.
///
/// The sink controls retention, queue limits, and error handling. It runs on the
/// emitting thread, so it should not block an async runtime thread. This layer
/// does not capture span context or install a global subscriber.
pub struct LogRecordLayer<F> {
    sink: F,
}

impl<F: Fn(SystemTime, LogRecord)> LogRecordLayer<F> {
    /// Construct a layer from a callback receiving timestamped records.
    #[must_use]
    pub const fn new(sink: F) -> Self {
        Self { sink }
    }

    /// Encode and deliver an event without going through a dispatcher.
    pub fn dispatch_event(&self, event: &Event<'_>) {
        let time = SystemTime::now();
        (self.sink)(time, LogRecord::new(event));
    }
}

impl<S, F> Layer<S> for LogRecordLayer<F>
where
    S: Subscriber,
    F: Fn(SystemTime, LogRecord) + 'static,
{
    fn on_event(&self, event: &Event<'_>, _: Context<'_, S>) {
        self.dispatch_event(event);
    }
}
