defmodule DiscordyVoice.RoomsTest do
  use ExUnit.Case, async: false

  alias DiscordyVoice.Domain.Room.Member
  alias DiscordyVoice.Infrastructure.Rooms

  setup do
    start_supervised!(DiscordyVoice.FakeSfu)
    DiscordyVoice.Fixtures.deps()
    on_exit(&stop_rooms/0)
    :ok
  end

  defp stop_rooms do
    for {_, pid, _, _} <- DynamicSupervisor.which_children(DiscordyVoice.RoomSupervisor) do
      DynamicSupervisor.terminate_child(DiscordyVoice.RoomSupervisor, pid)
    end
  end

  defp member(n), do: %Member{user_id: "u#{n}", session_id: "s#{n}", pid: self(), audio_ssrc: n}

  # `Registry.lookup/2` can still return the entry of a process that has just died (the registry
  # removes it asynchronously). A join right after a room stopped must start a fresh room instead
  # of giving up on the stale pid.
  test "a join right after the room process died starts a fresh room" do
    for n <- 1..500 do
      assert {:ok, _, _} = Rooms.join("ch-stale", member(n))
      stop_rooms()
    end
  end
end
