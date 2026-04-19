//! Tracing / logging subscriber initialization.
//!
//! Selects between plain text, JSON, and OTLP-exporting layouts based on
//! `BOLT_JSON_LOGS` and `OTEL_EXPORTER_OTLP_ENDPOINT`.

/// Initialize the global tracing subscriber.
///
/// - `BOLT_JSON_LOGS=1` switches to structured JSON output.
/// - `OTEL_EXPORTER_OTLP_ENDPOINT=...` enables OTLP span export alongside logs.
pub(crate) fn init() {
    let json_logs = std::env::var("BOLT_JSON_LOGS")
        .map(|v| v == "1")
        .unwrap_or(false);

    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));

    if std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT").is_ok() {
        use tracing_subscriber::prelude::*;

        let exporter = opentelemetry_otlp::SpanExporter::builder()
            .with_tonic()
            .build()
            .expect("failed to create OTLP span exporter");
        let provider = opentelemetry_sdk::trace::SdkTracerProvider::builder()
            .with_batch_exporter(exporter)
            .build();
        let tracer = opentelemetry::trace::TracerProvider::tracer(&provider, "sedandelivery-bolt");

        if json_logs {
            let otel_layer = tracing_opentelemetry::layer().with_tracer(tracer);
            tracing_subscriber::registry()
                .with(env_filter)
                .with(tracing_subscriber::fmt::layer().json())
                .with(otel_layer)
                .init();
        } else {
            let otel_layer = tracing_opentelemetry::layer().with_tracer(tracer);
            tracing_subscriber::registry()
                .with(env_filter)
                .with(tracing_subscriber::fmt::layer())
                .with(otel_layer)
                .init();
        }
        tracing::info!("OpenTelemetry OTLP tracing enabled");
    } else if json_logs {
        tracing_subscriber::fmt()
            .json()
            .with_env_filter(env_filter)
            .init();
    } else {
        tracing_subscriber::fmt().with_env_filter(env_filter).init();
    }
}
