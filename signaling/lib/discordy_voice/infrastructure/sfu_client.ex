defmodule DiscordyVoice.Infrastructure.SfuClient do
  @moduledoc """
  Client of the SFU control socket (P8: TCP, one JSON document per line).

  It keeps the connection alive with exponential back-off and, on every (re)connect, sends
  `sync` with the sessions it knows. The SFU closes the ones signaling does not know, and answers
  with the ones it lost (it restarted), which are replayed with `create_session` under their
  original ICE credentials: state is rebuilt without clients rejoining.

  Unsolicited events are dispatched to the owning connection process through
  `DiscordyVoice.SessionRegistry`: `{:sfu, {:speaking, ssrc, speaking?}}` and
  `{:sfu, {:closed, reason}}`.
  """

  use GenServer

  @behaviour DiscordyVoice.Application.Ports.SfuControl

  alias DiscordyVoice.Infrastructure.NodeLoad

  require Logger

  @call_timeout 6_000
  @connect_timeout 2_000
  @min_backoff 250
  @max_backoff 5_000
  @max_line 1_048_576

  # -- Port implementation ------------------------------------------------------------------

  @impl DiscordyVoice.Application.Ports.SfuControl
  def create_session(payload), do: call({:create_session, payload})

  @impl DiscordyVoice.Application.Ports.SfuControl
  def close_session(session_id) do
    GenServer.cast(__MODULE__, {:close_session, session_id})
  end

  @impl DiscordyVoice.Application.Ports.SfuControl
  def set_sink_wants(session_id, wants), do: call({:set_sink_wants, session_id, wants})

  @doc "Whether the control socket is currently connected."
  @spec connected?(GenServer.server()) :: boolean()
  def connected?(server \\ __MODULE__), do: GenServer.call(server, :connected?)

  defp call(message) do
    GenServer.call(__MODULE__, message, @call_timeout)
  catch
    :exit, _reason -> {:error, :sfu_unavailable}
  end

  # -- Process ------------------------------------------------------------------------------

  @spec start_link(keyword()) :: GenServer.on_start()
  def start_link(opts) do
    GenServer.start_link(__MODULE__, opts, name: Keyword.get(opts, :name, __MODULE__))
  end

  @impl true
  def init(opts) do
    {host, port} = Keyword.fetch!(opts, :address)
    send(self(), :connect)

    {:ok,
     %{
       host: String.to_charlist(host),
       port: port,
       socket: nil,
       backoff: @min_backoff,
       next_req: 1,
       pending: %{},
       sessions: %{}
     }}
  end

  @impl true
  def handle_call(:connected?, _from, state), do: {:reply, state.socket != nil, state}

  def handle_call(_message, _from, %{socket: nil} = state),
    do: {:reply, {:error, :sfu_unavailable}, state}

  def handle_call({:create_session, payload}, from, state) do
    {_req, state} = send_command(state, "create_session", payload, {:create, from, payload})
    {:noreply, state}
  end

  def handle_call({:set_sink_wants, session_id, wants}, _from, state) do
    {_req, state} =
      send_command(state, "set_sink_wants", %{session_id: session_id, wants: wants}, :ignore)

    {:reply, :ok, state}
  end

  @impl true
  def handle_cast({:close_session, session_id}, state) do
    state = %{state | sessions: Map.delete(state.sessions, session_id)}

    if state.socket do
      {_req, state} = send_command(state, "close_session", %{session_id: session_id}, :ignore)
      {:noreply, state}
    else
      {:noreply, state}
    end
  end

  @impl true
  def handle_info(:connect, state) do
    opts = [:binary, active: :once, packet: :line, packet_size: @max_line, nodelay: true]

    case :gen_tcp.connect(state.host, state.port, opts, @connect_timeout) do
      {:ok, socket} ->
        Logger.info("sfu control connected")
        NodeLoad.put_sfu_connected(true)
        state = %{state | socket: socket, backoff: @min_backoff}
        {:noreply, sync(state)}

      {:error, reason} ->
        Logger.warning("sfu control connect failed: #{inspect(reason)}")
        {:noreply, schedule_reconnect(state)}
    end
  end

  def handle_info({:tcp, socket, line}, %{socket: socket} = state) do
    :ok = :inet.setopts(socket, active: :once)

    case Jason.decode(line) do
      {:ok, message} -> {:noreply, handle_message(message, state)}
      {:error, _} -> {:noreply, state}
    end
  end

  def handle_info({:tcp_closed, socket}, %{socket: socket} = state),
    do: {:noreply, disconnected(state)}

  def handle_info({:tcp_error, socket, _reason}, %{socket: socket} = state),
    do: {:noreply, disconnected(state)}

  def handle_info(_stale, state), do: {:noreply, state}

  # -- Replies and events -------------------------------------------------------------------

  defp handle_message(%{"evt" => "session_created", "req" => req} = reply, state) do
    case Map.pop(state.pending, req) do
      {{:create, from, payload}, pending} ->
        local = %{
          local_ufrag: reply["local_ufrag"],
          local_pwd: reply["local_pwd"],
          fingerprint: reply["fingerprint"]
        }

        GenServer.reply(from, {:ok, local})

        replayable =
          Map.merge(payload, %{local_ufrag: local.local_ufrag, local_pwd: local.local_pwd})

        %{
          state
          | pending: pending,
            sessions: Map.put(state.sessions, payload.session_id, replayable)
        }

      {{:replay, _session_id}, pending} ->
        %{state | pending: pending}

      {_other, pending} ->
        %{state | pending: pending}
    end
  end

  defp handle_message(%{"evt" => "error", "req" => req} = reply, state) when is_binary(req) do
    case Map.pop(state.pending, req) do
      {{:create, from, _payload}, pending} ->
        GenServer.reply(from, {:error, {:sfu, reply["code"], reply["message"]}})
        %{state | pending: pending}

      {{:replay, session_id}, pending} ->
        Logger.warning("sfu refused to replay #{session_id}: #{inspect(reply)}")
        notify(session_id, {:closed, "replay_failed"})
        %{state | pending: pending, sessions: Map.delete(state.sessions, session_id)}

      {_other, pending} ->
        Logger.warning("sfu rejected a command: #{inspect(reply)}")
        %{state | pending: pending}
    end
  end

  defp handle_message(%{"evt" => "synced", "req" => req, "missing" => missing}, state) do
    {_sync, pending} = Map.pop(state.pending, req)
    replay(missing, %{state | pending: pending})
  end

  defp handle_message(
         %{"evt" => "speaking", "session_id" => id, "ssrc" => ssrc, "speaking" => speaking},
         state
       ) do
    notify(id, {:speaking, ssrc, speaking})
    state
  end

  defp handle_message(%{"evt" => "session_closed", "session_id" => id, "reason" => reason}, state) do
    if reason == "requested" do
      state
    else
      notify(id, {:closed, reason})
      %{state | sessions: Map.delete(state.sessions, id)}
    end
  end

  defp handle_message(%{"evt" => "stats", "egress_bps" => bps}, state) when is_integer(bps) do
    NodeLoad.put_egress_bps(bps)
    state
  end

  # Acknowledgements of fire-and-forget commands (`closed`, `sink_wants_applied`).
  defp handle_message(%{"req" => req}, state) when is_binary(req) do
    %{state | pending: Map.delete(state.pending, req)}
  end

  defp handle_message(_other, state), do: state

  defp notify(session_id, message) do
    Registry.dispatch(DiscordyVoice.SessionRegistry, session_id, fn entries ->
      for {pid, _} <- entries, do: send(pid, {:sfu, message})
    end)
  end

  # -- Connection management ----------------------------------------------------------------

  defp sync(state) do
    ids = Map.keys(state.sessions)
    {_req, state} = send_command(state, "sync", %{session_ids: ids}, :sync)
    state
  end

  defp replay(missing, state) do
    Enum.reduce(missing, state, fn session_id, acc ->
      case Map.fetch(acc.sessions, session_id) do
        {:ok, payload} ->
          Logger.info("replaying session #{session_id} to the sfu")
          {_req, acc} = send_command(acc, "create_session", payload, {:replay, session_id})
          acc

        :error ->
          acc
      end
    end)
  end

  defp disconnected(state) do
    Logger.warning("sfu control disconnected")
    NodeLoad.put_sfu_connected(false)

    for {_req, pending} <- state.pending do
      with {:create, from, _payload} <- pending,
           do: GenServer.reply(from, {:error, :sfu_unavailable})
    end

    schedule_reconnect(%{state | socket: nil, pending: %{}})
  end

  defp schedule_reconnect(state) do
    Process.send_after(self(), :connect, state.backoff)
    %{state | backoff: min(state.backoff * 2, @max_backoff)}
  end

  defp send_command(state, cmd, payload, tag) do
    req = Integer.to_string(state.next_req)
    line = Jason.encode!(Map.merge(payload, %{req: req, cmd: cmd})) <> "\n"

    case :gen_tcp.send(state.socket, line) do
      :ok -> :ok
      {:error, _reason} -> send(self(), {:tcp_closed, state.socket})
    end

    {req, %{state | next_req: state.next_req + 1, pending: Map.put(state.pending, req, tag)}}
  end
end
