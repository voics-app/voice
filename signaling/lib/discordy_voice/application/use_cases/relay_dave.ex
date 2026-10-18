defmodule DiscordyVoice.Application.UseCases.RelayDave do
  @moduledoc "DAVE opcodes 21-31: relays MLS messages between members when the channel is end-to-end encrypted."

  alias DiscordyVoice.Application.Deps
  alias DiscordyVoice.Domain.{Dave, VoiceSession}

  @spec run(VoiceSession.t(), non_neg_integer(), term(), Deps.t()) :: :ok
  def run(%VoiceSession{e2ee: false}, _op, _d, _deps), do: :ok

  def run(%VoiceSession{} = session, op, d, %Deps{} = deps) do
    deliver(session, Dave.route(op, d, session.user_id), deps)
  end

  defp deliver(_session, :drop, _deps), do: :ok

  defp deliver(session, {:broadcast, op, d}, deps),
    do: deps.rooms.broadcast(session.channel_id, session.user_id, op, d)

  defp deliver(session, {:to, user_ids, op, d}, deps),
    do: deps.rooms.send_to(session.channel_id, session.user_id, user_ids, op, d)

  defp deliver(session, {:fanout, routes}, deps) do
    Enum.each(routes, &deliver(session, &1, deps))
  end
end
