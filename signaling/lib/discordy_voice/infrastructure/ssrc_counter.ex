defmodule DiscordyVoice.Infrastructure.SsrcCounter do
  @moduledoc """
  Node-unique SSRC plans from an atomic counter that starts at a random point of the 32-bit
  space. Each plan consumes `SsrcPlan.size/0` consecutive values, so plans never overlap.
  """

  @behaviour DiscordyVoice.Application.Ports.SsrcAllocator

  alias DiscordyVoice.Domain.SsrcPlan

  @key {__MODULE__, :counter}

  @doc "Creates the counter; call once at boot."
  @spec init() :: :ok
  def init do
    ref = :atomics.new(1, signed: false)
    :atomics.put(ref, 1, :rand.uniform(0xFFFFFFFF))
    :persistent_term.put(@key, ref)
  end

  @impl true
  def allocate do
    ref = :persistent_term.get(@key)
    SsrcPlan.build(:atomics.add_get(ref, 1, SsrcPlan.size()) - SsrcPlan.size())
  end
end
