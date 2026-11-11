use slogr::{ConsoleWriter, RawLoggingLayer, otel_info};
use tracing::instrument::WithSubscriber;
use tracing_subscriber::prelude::*;

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let subscriber =
        tracing_subscriber::registry().with(RawLoggingLayer::new(ConsoleWriter::no_color()));
    async {
        otel_info!("worker.start", message = "standalone logging", workers = 1);
        tokio::task::yield_now().await;
        otel_info!("worker.ready", ready = true);
    }
    .with_subscriber(subscriber)
    .await;
}
