defmodule DiscordyVoice.Application.Ports.SsrcAllocator do
  @moduledoc "Hands out node-unique SSRC plans."

  alias DiscordyVoice.Domain.SsrcPlan

  @callback allocate() :: SsrcPlan.t()
end
