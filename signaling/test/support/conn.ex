defmodule DiscordyVoice.Conn do
  @moduledoc """
  A stand-in for a voice WebSocket connection process: it owns a `VoiceSession`, runs frames
  through `Dispatch` like the socket does, and forwards what the room pushes to its owner.
  """

  alias DiscordyVoice.Application.UseCases.{Dispatch, Leave}
  alias DiscordyVoice.Domain.Protocol.Frame
  alias DiscordyVoice.Domain.VoiceSession

  def start(name, deps) do
    owner = self()
    pid = spawn(fn -> loop(name, owner, deps, VoiceSession.new()) end)
    {name, pid}
  end

  @doc "Sends a frame and returns the effects the dispatcher produced."
  def send_frame({_name, pid}, op, d) do
    ref = make_ref()
    send(pid, {:frame, op, d, self(), ref})

    receive do
      {^ref, effects} -> effects
    after
      2_000 -> raise "conn did not answer"
    end
  end

  def session({_name, pid}) do
    ref = make_ref()
    send(pid, {:session, self(), ref})

    receive do
      {^ref, session} -> session
    end
  end

  def stop({_name, pid}), do: Process.exit(pid, :kill)

  def leave({_name, pid}) do
    ref = make_ref()
    send(pid, {:leave, self(), ref})

    receive do
      {^ref, :ok} -> :ok
    end
  end

  defp loop(name, owner, deps, session) do
    receive do
      {:frame, op, d, from, ref} ->
        {session, effects} = Dispatch.run(session, %Frame{op: op, d: d}, deps)
        send(from, {ref, effects})
        loop(name, owner, deps, session)

      {:session, from, ref} ->
        send(from, {ref, session})
        loop(name, owner, deps, session)

      {:leave, from, ref} ->
        Leave.run(session, deps)
        send(from, {ref, :ok})
        loop(name, owner, deps, session)

      {:push, op, d} ->
        send(owner, {:pushed, name, op, d})
        loop(name, owner, deps, session)

      :evicted ->
        send(owner, {:evicted, name})
        loop(name, owner, deps, session)
    end
  end
end
