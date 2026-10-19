defmodule DiscordyVoice.Interface.Endpoint do
  @moduledoc "Phoenix endpoint (Bandit) serving the router."

  use Phoenix.Endpoint, otp_app: :discordy_voice

  plug Plug.Telemetry, event_prefix: [:discordy_voice, :endpoint]
  plug DiscordyVoice.Interface.Router
end
