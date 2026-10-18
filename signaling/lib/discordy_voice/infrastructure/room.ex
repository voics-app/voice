defmodule DiscordyVoice.Infrastructure.Room do
  @moduledoc """
  The process owning one voice channel's membership on this node. It monitors every member
  connection, so a crashed socket still leaves: the SFU session is closed and the others are told.
  The room stops when its last member leaves.
  """

  use GenServer, restart: :temporary

  alias DiscordyVoice.Application.Deps
  alias DiscordyVoice.Contracts
  alias DiscordyVoice.Domain.Room, as: RoomState
  alias DiscordyVoice.Infrastructure.NodeLoad

  @spec start_link(String.t()) :: GenServer.on_start()
  def start_link(channel_id) do
    GenServer.start_link(__MODULE__, channel_id,
      name: {:via, Registry, {DiscordyVoice.RoomRegistry, channel_id}}
    )
  end

  @impl true
  def init(channel_id) do
    # The external sender's public key is announced to E2EE members (DAVE op 25). Key material
    # of the group itself never reaches the server.
    {public_key, _private_key} = :crypto.generate_key(:eddsa, :ed25519)
    NodeLoad.room_opened()

    {:ok,
     %{
       channel_id: channel_id,
       room: RoomState.new(),
       monitors: %{},
       external_sender: public_key,
       sfu: Deps.current().sfu
     }}
  end

  @impl true
  def handle_call({:join, member}, _from, state) do
    {room, evicted} = RoomState.join(state.room, member)
    state = %{state | room: room, monitors: monitor(state.monitors, member.pid)}

    if evicted do
      send(evicted.pid, :evicted)
      state.sfu.close_session(evicted.session_id)
    else
      NodeLoad.user_joined()
    end

    others = RoomState.others(room, member.user_id)
    push(others, Contracts.op(:clients_connect), %{user_ids: [member.user_id]})
    {:reply, {:ok, others, state.external_sender}, state}
  end

  def handle_call({:leave, user_id, pid}, _from, state) do
    {room, left} = RoomState.leave(state.room, user_id, pid)
    finish_leave(left, %{state | room: room})
  end

  @impl true
  def handle_cast({:put_video, user_id, video}, state) do
    {:noreply, %{state | room: RoomState.put_video(state.room, user_id, video)}}
  end

  def handle_cast({:broadcast, from, op, d}, state) do
    push(RoomState.others(state.room, from), op, d)
    {:noreply, state}
  end

  def handle_cast({:send_to, from, user_ids, op, d}, state) do
    push(RoomState.select(state.room, user_ids, from), op, d)
    {:noreply, state}
  end

  @impl true
  def handle_info({:DOWN, _ref, :process, pid, _reason}, state) do
    {room, left} = RoomState.leave_pid(state.room, pid)
    state = %{state | monitors: Map.delete(state.monitors, pid)}

    case finish_leave(left, %{state | room: room}) do
      {:reply, :ok, new_state} -> {:noreply, new_state}
      {:stop, reason, :ok, new_state} -> {:stop, reason, new_state}
    end
  end

  @impl true
  def terminate(_reason, state) do
    # Members still present when the room dies abnormally no longer count as connected.
    for _member <- Map.keys(state.room.members), do: NodeLoad.user_left()
    NodeLoad.room_closed()
  end

  defp finish_leave(nil, state), do: {:reply, :ok, state}

  defp finish_leave(left, state) do
    NodeLoad.user_left()
    state.sfu.close_session(left.session_id)

    push(RoomState.others(state.room, left.user_id), Contracts.op(:client_disconnect), %{
      user_id: left.user_id
    })

    case Map.pop(state.monitors, left.pid) do
      {nil, monitors} ->
        reply_or_stop(%{state | monitors: monitors})

      {ref, monitors} ->
        Process.demonitor(ref, [:flush])
        reply_or_stop(%{state | monitors: monitors})
    end
  end

  defp reply_or_stop(state) do
    if RoomState.empty?(state.room), do: {:stop, :normal, :ok, state}, else: {:reply, :ok, state}
  end

  defp monitor(monitors, pid) do
    if Map.has_key?(monitors, pid),
      do: monitors,
      else: Map.put(monitors, pid, Process.monitor(pid))
  end

  defp push(members, op, d) do
    Enum.each(members, &send(&1.pid, {:push, op, d}))
  end
end
