import Config

config :discordy_voice, edge: false

config :discordy_voice, DiscordyVoice.Interface.Endpoint,
  http: [ip: {127, 0, 0, 1}, port: 0],
  server: false

config :opentelemetry, traces_exporter: :none

config :logger, level: :warning
