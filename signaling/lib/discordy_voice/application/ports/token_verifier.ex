defmodule DiscordyVoice.Application.Ports.TokenVerifier do
  @moduledoc "Verifies a voice token (P3: HS256, 60 s lifetime) and returns its claims."

  @callback verify(token :: String.t()) :: {:ok, claims :: map()} | {:error, reason :: atom()}
end
