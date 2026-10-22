defmodule DiscordyVoice.Infrastructure.Rooms do
  @moduledoc "The room registry: finds or starts the process of a voice channel and talks to it."

  @behaviour DiscordyVoice.Application.Ports.Rooms

  alias DiscordyVoice.Infrastructure.Room

  @attempts 3
  @retry_pause_ms 5

  @impl true
  def join(channel_id, member), do: join(channel_id, member, @attempts)

  defp join(_channel_id, _member, 0), do: {:error, :room_unavailable}

  defp join(channel_id, member, attempts) do
    with {:ok, pid} <- ensure_room(channel_id) do
      GenServer.call(pid, {:join, member})
    end
  catch
    # The room stopped between lookup and call (its last member left): start a fresh one.
    :exit, _reason ->
      Process.sleep(@retry_pause_ms)
      join(channel_id, member, attempts - 1)
  end

  @impl true
  def leave(channel_id, user_id, pid) do
    case lookup(channel_id) do
      {:ok, room} -> GenServer.call(room, {:leave, user_id, pid})
      :error -> :ok
    end
  catch
    :exit, _reason -> :ok
  end

  @impl true
  def put_video(channel_id, user_id, video), do: cast(channel_id, {:put_video, user_id, video})

  @impl true
  def broadcast(channel_id, from, op, d), do: cast(channel_id, {:broadcast, from, op, d})

  @impl true
  def send_to(channel_id, from, user_ids, op, d),
    do: cast(channel_id, {:send_to, from, user_ids, op, d})

  defp cast(channel_id, message) do
    with {:ok, room} <- lookup(channel_id), do: GenServer.cast(room, message)
    :ok
  end

  # The registry drops the entry of a dead process asynchronously, so `lookup/2` can still return
  # the pid of a room that has just stopped: only a live process counts as the channel's room.
  defp lookup(channel_id) do
    case Registry.lookup(DiscordyVoice.RoomRegistry, channel_id) do
      [{pid, _}] -> if Process.alive?(pid), do: {:ok, pid}, else: :error
      [] -> :error
    end
  end

  defp ensure_room(channel_id) do
    case lookup(channel_id) do
      {:ok, pid} ->
        {:ok, pid}

      :error ->
        case DynamicSupervisor.start_child(DiscordyVoice.RoomSupervisor, {Room, channel_id}) do
          {:ok, pid} -> {:ok, pid}
          {:error, {:already_started, pid}} -> {:ok, pid}
          {:error, reason} -> {:error, reason}
        end
    end
  end
end
