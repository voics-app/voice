import Config

# OpenTelemetry is exported only when OTEL_EXPORTER_OTLP_ENDPOINT is set.
if endpoint = System.get_env("OTEL_EXPORTER_OTLP_ENDPOINT") do
  config :opentelemetry, traces_exporter: :otlp
  config :opentelemetry_exporter, otlp_protocol: :http_protobuf, otlp_endpoint: endpoint
end

config :opentelemetry, resource: [service: [name: "voice-signaling"]]
