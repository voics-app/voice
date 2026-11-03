defmodule DiscordyVoice.Interface.VoiceSocket do
  @moduledoc """
  The voice WebSocket (`/voice`): decodes `{op, d}` frames, hands them to the use cases and writes
  back the effects. Also receives what other processes push to this connection (room relays,
  SFU events, eviction) and enforces the identify and heartbeat deadlines.
  """

  @behaviour WebSock

  alias DiscordyVoice.Application.Deps
  alias DiscordyVoice.Application.UseCases.{Dispatch, Leave, RelaySpeaking}
  alias DiscordyVoice.Contracts
  alias DiscordyVoice.Domain.Protocol.Frame
  alias DiscordyVoice.Domain.VoiceSession

  @impl true
  def init(_args) do
    state = %{
      session: VoiceSession.new(),
      deps: Deps.current(),
      identify_timer:
        Process.send_after(self(), :identify_timeout, Contracts.identify_timeout_ms()),
      heartbeat_timer: nil
    }

    hello =
      {:text,
       Frame.encode(Contracts.op(:hello), %{heartbeat_interval: Contracts.heartbeat_interval_ms()})}

    {:push, hello, arm_heartbeat(state)}
  end

  @impl true
  def handle_in({text, [opcode: :text]}, state) do
    case Frame.decode(text) do
      {:ok, frame} ->
        frame |> then(&Dispatch.run(state.session, &1, state.deps)) |> apply_effects(frame, state)

      {:error, _} ->
        close(:invalid_payload, [], state)
    end
  end

  def handle_in({_binary, [opcode: :binary]}, state), do: close(:invalid_payload, [], state)

  @impl true
  def handle_info({:push, op, d}, state), do: {:push, {:text, Frame.encode(op, d)}, state}
  def handle_info(:evicted, state), do: close(:disconnected, [], state)

  def handle_info(:identify_timeout, %{session: %{phase: :connected}} = state),
    do: close(:session_timeout, [], state)

  def handle_info(:identify_timeout, state), do: {:ok, state}
  def handle_info(:heartbeat_timeout, state), do: close(:session_timeout, [], state)

  def handle_info({:sfu, {:closed, _reason}}, state), do: close(:disconnected, [], state)

  def handle_info(
        {:sfu, {:speaking, ssrc, speaking?}},
        %{session: %{ssrcs: %{audio: ssrc}} = session} = state
      ) do
    RelaySpeaking.from_sfu(session, speaking?, state.deps)
    {:ok, state}
  end

  def handle_info(
        {:sfu, {:stream_viewers, ssrc, user_ids}},
        %{session: %{ssrcs: %{screen: %{ssrc: ssrc}}}} = state
      ) do
    d = %{ssrc: ssrc, user_ids: user_ids}
    {:push, {:text, Frame.encode(Contracts.op(:stream_viewers), d)}, state}
  end

  def handle_info(_other, state), do: {:ok, state}

  @impl true
  def terminate(_reason, state) do
    Leave.run(state.session, state.deps)
    :ok
  end

  defp apply_effects({session, effects}, frame, state) do
    was_identified = VoiceSession.identified?(state.session)
    state = %{state | session: session}
    state = if frame.op == Contracts.op(:heartbeat), do: arm_heartbeat(state), else: state
    register_session(was_identified, session)

    replies = for {:reply, op, d} <- effects, do: {:text, Frame.encode(op, d)}

    case Enum.find(effects, &match?({:close, _}, &1)) do
      {:close, reason} -> close(reason, replies, state)
      nil when replies == [] -> {:ok, state}
      nil -> {:push, replies, state}
    end
  end

  # SFU events are routed by session id, so the connection subscribes once identified.
  defp register_session(false, %VoiceSession{phase: :identified, session_id: id}) do
    {:ok, _} = Registry.register(DiscordyVoice.SessionRegistry, id, nil)
    :ok
  end

  defp register_session(_was_identified, _session), do: :ok

  defp close(reason, replies, state) do
    {:stop, :normal, {Contracts.close_code(reason), Atom.to_string(reason)}, replies, state}
  end

  defp arm_heartbeat(state) do
    if state.heartbeat_timer, do: Process.cancel_timer(state.heartbeat_timer)
    timeout = round(Contracts.heartbeat_interval_ms() * Contracts.heartbeat_grace_factor())
    %{state | heartbeat_timer: Process.send_after(self(), :heartbeat_timeout, timeout)}
  end
end
