defmodule DiscordyVoice.Application.UseCases.SetMediaSinkWants do
  @moduledoc "MEDIA_SINK_WANTS: a viewer picks, per publisher SSRC, the quality (0-100) it wants."

  alias DiscordyVoice.Application.Deps
  alias DiscordyVoice.Domain.VoiceSession

  @spec run(VoiceSession.t(), term(), Deps.t()) :: :ok | {:close, atom()}
  def run(%VoiceSession{phase: :negotiated} = session, d, %Deps{} = deps) when is_map(d) do
    wants =
      for {key, quality} <- d, ssrc?(key), is_integer(quality), quality in 0..100, into: %{} do
        {key, quality}
      end

    case deps.sfu.set_sink_wants(session.session_id, wants) do
      :ok -> :ok
      {:error, _reason} -> :ok
    end
  end

  def run(%VoiceSession{phase: :negotiated}, _d, _deps), do: {:close, :invalid_payload}
  def run(_session, _d, _deps), do: :ok

  defp ssrc?(key) when is_binary(key) do
    match?({ssrc, ""} when ssrc in 1..0xFFFFFFFF, Integer.parse(key))
  end

  defp ssrc?(_key), do: false
end
