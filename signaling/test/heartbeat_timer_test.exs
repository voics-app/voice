defmodule DiscordyVoice.HeartbeatTimerTest do
  use ExUnit.Case, async: false

  alias DiscordyVoice.{Contracts, Fixtures}
  alias DiscordyVoice.Domain.Protocol.Frame
  alias DiscordyVoice.Interface.VoiceSocket

  setup do
    Fixtures.deps()
    {:push, _hello, state} = VoiceSocket.init([])
    Process.cancel_timer(state.identify_timer)
    on_exit(fn -> Process.cancel_timer(state.heartbeat_timer) end)
    {:ok, state: state}
  end

  test "a queued timeout from before a heartbeat cannot close the renewed session", %{
    state: state
  } do
    # Replays the old untagged timeout already queued when cancel_timer/1 loses the race.
    send(self(), :heartbeat_timeout)
    {:push, _ack, renewed} = heartbeat(state)
    assert_receive expired
    assert {:ok, ^renewed} = VoiceSocket.handle_info(expired, renewed)
    Process.cancel_timer(renewed.heartbeat_timer)
  end

  test "only the current timer reference can close the session", %{state: state} do
    expired = {:timeout, state.heartbeat_timer, :heartbeat_timeout}
    {:push, _ack, renewed} = heartbeat(state)
    assert renewed.heartbeat_timer != state.heartbeat_timer
    assert {:ok, ^renewed} = VoiceSocket.handle_info(expired, renewed)

    assert {:stop, :normal, {4009, "session_timeout"}, [], ^renewed} =
             VoiceSocket.handle_info(
               {:timeout, renewed.heartbeat_timer, :heartbeat_timeout},
               renewed
             )

    Process.cancel_timer(renewed.heartbeat_timer)
  end

  test "the deadline keeps the existing negotiated heartbeat allowance", %{state: state} do
    expected = round(Contracts.heartbeat_interval_ms() * Contracts.heartbeat_grace_factor())
    remaining = Process.read_timer(state.heartbeat_timer)
    assert remaining <= expected and remaining > expected - 1_000
  end

  defp heartbeat(state),
    do: VoiceSocket.handle_in({Frame.encode(3, %{t: 1}), [opcode: :text]}, state)
end
