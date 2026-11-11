use slogr::{Attribute, ConsoleWriter, RawLoggingLayer, otel_info};
use tracing::instrument::WithSubscriber;
use tracing_subscriber::prelude::*;

static WORKERS: Attribute<u64> = Attribute::new("workers", "Worker count");
static READY: Attribute<bool> = Attribute::new("ready", "Worker readiness");

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let subscriber =
        tracing_subscriber::registry().with(RawLoggingLayer::new(ConsoleWriter::no_color()));
    async {
        otel_info!("Starting standalone logging", WORKERS = 1);
        tokio::task::yield_now().await;
        otel_info!("Worker ready", READY = true);
    }
    .with_subscriber(subscriber)
    .await;
}
