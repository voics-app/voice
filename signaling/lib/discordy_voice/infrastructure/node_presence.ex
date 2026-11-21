defmodule DiscordyVoice.Infrastructure.NodePresence do
  @moduledoc """
  Keeps this node discoverable (P8): registers it in etcd under a 10-second lease refreshed every
  3 seconds, rewriting the value with the current load each time (users, CPU, egress, draining).

  The gateway can drain the node by writing the node's drain marker key; the marker is polled on
  the same cadence. A missing lease (etcd restart, long pause) is re-granted on the next tick, and
  the key is deleted on graceful shutdown so allocation stops immediately.
  """

  use GenServer
  @behaviour DiscordyVoice.Application.Ports.NodeRegistration

  alias DiscordyVoice.Contracts
  alias DiscordyVoice.Domain.NodeStatus
  alias DiscordyVoice.Infrastructure.{DrainControl, NodeLoad}

  require Logger

  @spec start_link(keyword()) :: GenServer.on_start()
  def start_link(opts),
    do: GenServer.start_link(__MODULE__, opts, name: Keyword.get(opts, :name, __MODULE__))

  @doc "Whether the node's registration currently exists in the store."
  @spec registered?(GenServer.server()) :: boolean()
  def registered?(server \\ __MODULE__) do
    GenServer.call(server, :registered?)
  catch
    :exit, _reason -> false
  end

  @doc "The current live registration lease, for node-bound synthetic probes."
  @spec lease_id(GenServer.server()) :: {:ok, String.t()} | {:error, :unregistered}
  def lease_id(server \\ __MODULE__) do
    GenServer.call(server, :lease_id)
  catch
    :exit, _reason -> {:error, :unregistered}
  end

  @impl true
  def init(opts) do
    Process.flag(:trap_exit, true)
    settings = Keyword.fetch!(opts, :settings)
    registry = Keyword.fetch!(opts, :registry)

    state = %{
      settings: settings,
      registry: registry,
      interval: Keyword.get(opts, :interval_ms, Contracts.refresh_interval_ms()),
      key: Contracts.node_key(settings.region, settings.node_id),
      lease: nil,
      valid_until: 0,
      drained_by_marker: false
    }

    case registry.connect(settings.etcd_endpoints) do
      :ok -> {:ok, state, {:continue, :tick}}
      {:error, reason} -> {:stop, {:etcd_connect_failed, reason}}
    end
  end

  @impl true
  def handle_continue(:tick, state), do: {:noreply, tick(state)}

  @impl true
  def handle_call(:registered?, _from, state),
    do: {:reply, state.lease != nil and now() < state.valid_until, state}

  def handle_call(:lease_id, _from, state) do
    result =
      if state.lease != nil and now() < state.valid_until,
        do: {:ok, Integer.to_string(state.lease)},
        else: {:error, :unregistered}

    {:reply, result, state}
  end

  @impl true
  def handle_info(:tick, state), do: {:noreply, tick(state)}
  def handle_info({:EXIT, _from, reason}, state), do: {:stop, reason, state}

  # Late replies of timed-out etcd calls and other strays must never crash the presence process.
  def handle_info(message, state) do
    Logger.debug("node presence ignored message: #{inspect(message, limit: 5)}")
    {:noreply, state}
  end

  @impl true
  def terminate(_reason, %{lease: lease, registry: registry, key: key}) when lease != nil do
    safe(fn -> registry.release(key, lease) end)
    :ok
  end

  def terminate(_reason, _state), do: :ok

  defp tick(state) do
    state = state |> sync_drain_marker() |> register()
    Process.send_after(self(), :tick, state.interval)
    state
  end

  defp sync_drain_marker(state) do
    case safe(fn -> state.registry.exists?(Contracts.drain_key(state.settings.node_id)) end) do
      {:ok, true} ->
        DrainControl.drain()
        %{state | drained_by_marker: true}

      {:ok, false} when state.drained_by_marker ->
        DrainControl.undrain()
        %{state | drained_by_marker: false}

      _ ->
        state
    end
  end

  defp register(state) do
    deadline = now() + Contracts.lease_ttl_s() * 1_000

    with {:ok, lease} <- ensure_lease(state),
         :ok <- write_registration(state, lease) do
      %{state | lease: lease, valid_until: deadline}
    else
      {:error, reason} ->
        Logger.warning("etcd registration failed: #{inspect(reason)}")
        %{state | lease: nil}
    end
  end

  defp now, do: System.monotonic_time(:millisecond)

  defp write_registration(%{lease: lease} = state, lease) do
    safe(fn -> state.registry.refresh(state.key, status_json(state.settings, lease), lease) end)
  end

  defp write_registration(state, lease) do
    safe(fn -> state.registry.claim(state.key, status_json(state.settings, lease), lease) end)
  end

  defp ensure_lease(%{lease: nil, registry: registry}),
    do: safe(fn -> registry.grant(Contracts.lease_ttl_s()) end)

  defp ensure_lease(%{lease: lease, registry: registry}) do
    case safe(fn -> registry.keep_alive(lease) end) do
      :ok -> {:ok, lease}
      {:error, _reason} -> safe(fn -> registry.grant(Contracts.lease_ttl_s()) end)
    end
  end

  # eetcd exits (GenServer call timeout) when a request times out on a slow cluster; turn that into
  # an error so the presence process survives and retries on the next tick.
  defp safe(fun) do
    fun.()
  catch
    kind, reason -> {:error, {kind, reason}}
  end

  defp status_json(settings, lease) do
    NodeStatus.to_json(%NodeStatus{
      node_id: settings.node_id,
      region: settings.region,
      ws_url: settings.public_ws_url,
      control_ip: settings.control_ip,
      control_port: settings.http_port,
      registration_lease: Integer.to_string(lease),
      public_ip: settings.public_ip,
      udp_port: settings.udp_port,
      version: settings.version,
      users: NodeLoad.users(),
      cpu_pct: NodeLoad.cpu_pct(),
      egress_bps: NodeLoad.egress_bps(),
      draining: DrainControl.draining?(),
      probe_required: settings.probe_required
    })
  end
end
