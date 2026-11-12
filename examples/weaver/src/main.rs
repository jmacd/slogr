use slogr::{ConsoleWriter, RawLoggingLayer, otel_info};
use tracing_subscriber::prelude::*;

include!(concat!(env!("OUT_DIR"), "/slogr_attributes.rs"));

use attributes::{example_requests, example_retryable, server_port, service_name};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::registry()
        .with(RawLoggingLayer::new(ConsoleWriter::no_color()))
        .try_init()?;
    let body = String::from("Weaver-generated attributes");
    let service = String::from("slogr-example");
    otel_info!(
        &body,
        service_name = &service,
        server_port = 4317u64,
        example_requests = 1u64,
        example_retryable = true,
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use slogr::views::{AnyValueView, AttributeView, LogRecordView, RawLogRecord};

    /// Scenario: the build script generates custom declarations and annotated standard references.
    /// Guarantees: generated statics accept borrowed strings and native numeric/boolean values with the registry's wire names.
    #[test]
    fn generated_attributes_encode_native_values() {
        let service = String::from("example-service");
        let record = slogr::__log_record_impl!(
            slogr::Level::INFO,
            "Generated schema",
            service_name = &service,
            server_port = 4317u64,
            example_requests = 2u64,
            example_retryable = false,
        );
        let view = record.as_view();
        let raw = RawLogRecord::new(view.body_attrs_bytes);
        assert_eq!(
            raw.body().unwrap().as_string(),
            Some(b"Generated schema".as_slice())
        );
        let attrs = raw.attributes().collect::<Vec<_>>();
        assert_eq!(attrs.len(), 4);
        assert_eq!(attrs[0].key(), b"service.name");
        assert_eq!(
            attrs[0].value().unwrap().as_string(),
            Some(service.as_bytes())
        );
        assert_eq!(attrs[1].key(), b"server.port");
        assert_eq!(attrs[1].value().unwrap().as_int64(), Some(4317));
        assert_eq!(attrs[2].key(), b"example.requests");
        assert_eq!(attrs[2].value().unwrap().as_int64(), Some(2));
        assert_eq!(attrs[3].key(), b"example.retryable");
        assert_eq!(attrs[3].value().unwrap().as_bool(), Some(false));
        assert_eq!(view.dropped_attributes_count, 0);
    }
}
