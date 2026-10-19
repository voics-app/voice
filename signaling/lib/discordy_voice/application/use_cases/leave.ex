defmodule DiscordyVoice.Application.UseCases.Leave do
  @moduledoc "Disconnect: leaves the room, which closes the SFU session and tells the other members."

  alias DiscordyVoice.Application.Deps
  alias DiscordyVoice.Domain.VoiceSession

  @spec run(VoiceSession.t(), Deps.t()) :: :ok
  def run(%VoiceSession{phase: :connected}, _deps), do: :ok

  def run(%VoiceSession{} = session, %Deps{} = deps),
    do: deps.rooms.leave(session.channel_id, session.user_id, self())
end
