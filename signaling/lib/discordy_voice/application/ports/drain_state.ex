defmodule DiscordyVoice.Application.Ports.DrainState do
  @moduledoc "Whether this node is draining: it keeps existing sessions but accepts no new ones."

  @callback draining?() :: boolean()
end
