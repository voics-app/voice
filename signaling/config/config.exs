import Config

config :discordy_voice, DiscordyVoice.Interface.Endpoint,
  adapter: Bandit.PhoenixAdapter,
  url: [host: "localhost"],
  server: true,
  render_errors: [formats: [json: DiscordyVoice.Interface.ErrorJSON], layout: false]

config :phoenix, :json_library, Jason

import_config "#{config_env()}.exs"
