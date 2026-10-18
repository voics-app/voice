defmodule DiscordyVoice.Infrastructure.DrainControl do
  @moduledoc """
  Drain mode: a draining node keeps its live sessions but refuses new IDENTIFYs, so the gateway
  allocates other nodes. Triggered at boot (`VOICE_DRAIN_ON_START`), over the internal HTTP API
  or by the gateway's marker key in etcd.
  """

  use Agent

  @behaviour DiscordyVoice.Application.Ports.DrainState

  @spec start_link(keyword()) :: Agent.on_start()
  def start_link(opts) do
    Agent.start_link(fn -> Keyword.get(opts, :draining, false) end, name: __MODULE__)
  end

  @impl true
  def draining?, do: Agent.get(__MODULE__, & &1)

  @spec drain() :: :ok
  def drain, do: Agent.update(__MODULE__, fn _ -> true end)

  @spec undrain() :: :ok
  def undrain, do: Agent.update(__MODULE__, fn _ -> false end)
end
