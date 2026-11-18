defmodule DiscordyVoice.Application.Ports.NodeRegistration do
  @moduledoc "Whether this process still owns its advertised node identity."
  @callback registered?() :: boolean()
end
