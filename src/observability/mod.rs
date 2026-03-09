use std::env;
use std::sync::OnceLock;

use opentelemetry::KeyValue;
use opentelemetry::global;
use opentelemetry::trace::TracerProvider as _;
use opentelemetry_otlp::{SpanExporter, WithExportConfig};
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::trace::SdkTracerProvider;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

pub const COMPONENT_RUNTIME: &str = "runtime";
pub const COMPONENT_AGENT: &str = "agent";
pub const COMPONENT_SUBAGENT: &str = "subagent";
pub const COMPONENT_BUS: &str = "bus";
pub const COMPONENT_CHANNELS: &str = "channels";
pub const COMPONENT_CRON: &str = "cron";
pub const COMPONENT_HEARTBEAT: &str = "heartbeat";
pub const COMPONENT_PROVIDER: &str = "provider";
pub const COMPONENT_TOOLS: &str = "tools";
pub const COMPONENT_SESSION: &str = "session";

pub const TARGET_RUNTIME: &str = "nanobot.runtime";
pub const TARGET_AGENT: &str = "nanobot.agent";
pub const TARGET_SUBAGENT: &str = "nanobot.subagent";
pub const TARGET_BUS: &str = "nanobot.bus";
pub const TARGET_CHANNELS: &str = "nanobot.channels";
pub const TARGET_CRON: &str = "nanobot.cron";
pub const TARGET_HEARTBEAT: &str = "nanobot.heartbeat";
pub const TARGET_PROVIDER: &str = "nanobot.provider";
pub const TARGET_TOOLS: &str = "nanobot.tools";
pub const TARGET_SESSION: &str = "nanobot.session";

struct ObservabilityProviders {
    #[allow(dead_code)]
    tracer_provider: Option<SdkTracerProvider>,
}

static INIT: OnceLock<()> = OnceLock::new();
static PROVIDERS: OnceLock<ObservabilityProviders> = OnceLock::new();

pub const ENV_NANOBOT_OTLP_TRACES: &str = "NANOBOT_OTLP_TRACES_ENABLED";

pub fn init() {
    INIT.get_or_init(|| {
        let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
        let service_name =
            env::var("NANOBOT_SERVICE_NAME").unwrap_or_else(|_| "nanobot-rs".to_string());

        let traces_endpoint = env_var_first(&[
            "NANOBOT_OTLP_TRACES_ENDPOINT",
            "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT",
            "OTEL_EXPORTER_OTLP_ENDPOINT",
        ]);
        let traces_enabled = env_flag(ENV_NANOBOT_OTLP_TRACES, true);

        let resource = Resource::builder_empty()
            .with_attributes([
                KeyValue::new("service.name", service_name.clone()),
                KeyValue::new("service.version", env!("CARGO_PKG_VERSION")),
            ])
            .build();

        let tracer_provider = if traces_enabled {
            build_tracer_provider(traces_endpoint.as_deref(), &resource)
        } else {
            None
        };

        if let Some(provider) = tracer_provider.as_ref() {
            global::set_tracer_provider(provider.clone());
            let tracer = provider.tracer(service_name.clone());
            let otel = tracing_opentelemetry::layer().with_tracer(tracer);
            let _ = tracing_subscriber::registry()
                .with(filter)
                .with(tracing_subscriber::fmt::layer().with_target(true))
                .with(otel)
                .try_init();
        } else {
            let _ = tracing_subscriber::registry()
                .with(filter)
                .with(tracing_subscriber::fmt::layer().with_target(true))
                .try_init();
        }

        let _ = PROVIDERS.set(ObservabilityProviders { tracer_provider });
    });
}

fn env_var_first(names: &[&str]) -> Option<String> {
    names
        .iter()
        .find_map(|name| env::var(name).ok().filter(|v| !v.trim().is_empty()))
}

fn env_flag(name: &str, default: bool) -> bool {
    match env::var(name) {
        Ok(raw) => match raw.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "y" | "on" => true,
            "0" | "false" | "no" | "n" | "off" => false,
            _ => default,
        },
        Err(_) => default,
    }
}

fn build_tracer_provider(endpoint: Option<&str>, resource: &Resource) -> Option<SdkTracerProvider> {
    let endpoint = endpoint?;
    let exporter = SpanExporter::builder()
        .with_http()
        .with_endpoint(endpoint.to_string())
        .build()
        .ok()?;
    Some(
        SdkTracerProvider::builder()
            .with_resource(resource.clone())
            .with_batch_exporter(exporter)
            .build(),
    )
}
