defmodule DiscordyVoice.Infrastructure.NodeLoad do
  @moduledoc """
  Lock-free gauges describing this node's load: connected users, rooms, the SFU's egress and
  whether the SFU control link is up. Written by the room processes and the SFU client, read by
  the etcd reporter and the metrics endpoint.
  """

  @key {__MODULE__, :gauges}
  @users 1
  @rooms 2
  @egress_bps 3
  @sfu_connected 4

  @doc "Creates the gauges; call once at boot."
  @spec init() :: :ok
  def init, do: :persistent_term.put(@key, :atomics.new(4, signed: true))

  @spec user_joined() :: :ok
  def user_joined, do: add(@users, 1)

  @spec user_left() :: :ok
  def user_left, do: add(@users, -1)

  @spec room_opened() :: :ok
  def room_opened, do: add(@rooms, 1)

  @spec room_closed() :: :ok
  def room_closed, do: add(@rooms, -1)

  @spec put_egress_bps(non_neg_integer()) :: :ok
  def put_egress_bps(bps), do: put(@egress_bps, bps)

  @spec put_sfu_connected(boolean()) :: :ok
  def put_sfu_connected(connected?), do: put(@sfu_connected, if(connected?, do: 1, else: 0))

  @spec users() :: non_neg_integer()
  def users, do: max(get(@users), 0)

  @spec rooms() :: non_neg_integer()
  def rooms, do: max(get(@rooms), 0)

  @spec egress_bps() :: non_neg_integer()
  def egress_bps, do: max(get(@egress_bps), 0)

  @spec sfu_connected?() :: boolean()
  def sfu_connected?, do: get(@sfu_connected) == 1

  @doc "CPU utilisation in percent (0.0 where the platform has no `cpu_sup`)."
  @spec cpu_pct() :: float()
  def cpu_pct do
    case :cpu_sup.util() do
      util when is_number(util) -> util * 1.0
      _ -> 0.0
    end
  catch
    _kind, _reason -> 0.0
  end

  defp add(index, delta), do: :atomics.add(:persistent_term.get(@key), index, delta)
  defp put(index, value), do: :atomics.put(:persistent_term.get(@key), index, value)
  defp get(index), do: :atomics.get(:persistent_term.get(@key), index)
end
