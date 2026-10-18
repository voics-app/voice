defmodule DiscordyVoice.Application.UseCases.RelaySpeaking do
  @moduledoc """
  SPEAKING: relays a client's speaking state to the rest of the room, and the SFU's own
  audio-level detection the same way. The SSRC is always the server-assigned one.
  """

  alias DiscordyVoice.Application.Deps
  alias DiscordyVoice.Contracts
  alias DiscordyVoice.Domain.VoiceSession

  @spec from_client(VoiceSession.t(), term(), Deps.t()) :: :ok | {:close, atom()}
  def from_client(%VoiceSession{} = session, %{"speaking" => bits}, deps)
      when is_integer(bits) and bits in 0..7 do
    broadcast(session, bits, deps)
  end

  def from_client(_session, _d, _deps), do: {:close, :invalid_payload}

  @spec from_sfu(VoiceSession.t(), boolean(), Deps.t()) :: :ok
  def from_sfu(%VoiceSession{} = session, speaking?, deps) do
    broadcast(session, if(speaking?, do: Contracts.speaking_flag(:microphone), else: 0), deps)
  end

  defp broadcast(session, bits, deps) do
    deps.rooms.broadcast(session.channel_id, session.user_id, Contracts.op(:speaking), %{
      speaking: bits,
      ssrc: session.ssrcs.audio,
      user_id: session.user_id
    })
  end
end
