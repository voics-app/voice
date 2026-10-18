defmodule DiscordyVoice.FakeSfuServer do
  @moduledoc """
  A scripted SFU control server: speaks the P8 NDJSON protocol over a real TCP socket. It
  remembers the sessions it was given so tests can simulate an SFU restart (`forget_sessions/1`).
  """

  use GenServer

  def start_link(opts \\ []), do: GenServer.start_link(__MODULE__, opts)
  def port(server), do: GenServer.call(server, :port)
  def received(server), do: GenServer.call(server, :received)
  def push(server, event), do: GenServer.call(server, {:push, event})
  def drop_connection(server), do: GenServer.call(server, :drop)
  def forget_sessions(server), do: GenServer.call(server, :forget)
  def reject_creates(server, code), do: GenServer.call(server, {:reject, code})

  @impl true
  def init(opts) do
    {:ok, listen} =
      :gen_tcp.listen(Keyword.get(opts, :port, 0), [
        :binary,
        active: false,
        packet: :line,
        reuseaddr: true
      ])

    {:ok, port} = :inet.port(listen)
    parent = self()
    spawn_link(fn -> accept_loop(listen, parent) end)

    {:ok,
     %{listen: listen, port: port, client: nil, received: [], sessions: MapSet.new(), reject: nil}}
  end

  defp accept_loop(listen, parent) do
    case :gen_tcp.accept(listen) do
      {:ok, socket} ->
        :ok = :gen_tcp.controlling_process(socket, parent)
        send(parent, {:accepted, socket})
        accept_loop(listen, parent)

      {:error, _} ->
        :ok
    end
  end

  @impl true
  def handle_call(:port, _from, state), do: {:reply, state.port, state}
  def handle_call(:received, _from, state), do: {:reply, Enum.reverse(state.received), state}

  def handle_call({:push, event}, _from, state) do
    reply(state.client, event)
    {:reply, :ok, state}
  end

  def handle_call(:drop, _from, state) do
    if state.client, do: :gen_tcp.close(state.client)
    {:reply, :ok, %{state | client: nil}}
  end

  def handle_call(:forget, _from, state), do: {:reply, :ok, %{state | sessions: MapSet.new()}}
  def handle_call({:reject, code}, _from, state), do: {:reply, :ok, %{state | reject: code}}

  @impl true
  def handle_info({:accepted, socket}, state) do
    :inet.setopts(socket, active: :once)
    {:noreply, %{state | client: socket}}
  end

  def handle_info({:tcp, socket, line}, state) do
    :inet.setopts(socket, active: :once)
    command = Jason.decode!(line)
    state = %{state | received: [command | state.received]}
    {:noreply, handle_command(command, state)}
  end

  def handle_info({:tcp_closed, socket}, state) do
    {:noreply, if(state.client == socket, do: %{state | client: nil}, else: state)}
  end

  defp handle_command(%{"cmd" => "create_session", "req" => req, "session_id" => id} = cmd, state) do
    if state.reject do
      reply(state.client, %{evt: "error", req: req, code: state.reject, message: "rejected"})
      state
    else
      reply(state.client, %{
        evt: "session_created",
        req: req,
        session_id: id,
        local_ufrag: cmd["local_ufrag"] || "fakeUfrag",
        local_pwd: cmd["local_pwd"] || "fakePasswordFakePassword",
        fingerprint: "sha-256 AA:BB:CC"
      })

      %{state | sessions: MapSet.put(state.sessions, id)}
    end
  end

  defp handle_command(%{"cmd" => "sync", "req" => req, "session_ids" => ids}, state) do
    missing = Enum.reject(ids, &MapSet.member?(state.sessions, &1))
    reply(state.client, %{evt: "synced", req: req, missing: missing})
    %{state | sessions: MapSet.intersection(state.sessions, MapSet.new(ids))}
  end

  defp handle_command(%{"cmd" => "close_session", "req" => req, "session_id" => id}, state) do
    reply(state.client, %{
      evt: "closed",
      req: req,
      session_id: id,
      existed: MapSet.member?(state.sessions, id)
    })

    %{state | sessions: MapSet.delete(state.sessions, id)}
  end

  defp handle_command(%{"cmd" => "set_sink_wants", "req" => req, "session_id" => id}, state) do
    reply(state.client, %{evt: "sink_wants_applied", req: req, session_id: id})
    state
  end

  defp reply(nil, _event), do: :ok
  defp reply(socket, event), do: :gen_tcp.send(socket, Jason.encode!(event) <> "\n")
end
