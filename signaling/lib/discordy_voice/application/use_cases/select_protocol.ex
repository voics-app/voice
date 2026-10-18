defmodule DiscordyVoice.Application.UseCases.SelectProtocol do
  @moduledoc """
  SELECT_PROTOCOL: turns the client's ICE/DTLS parameters and codecs into an SFU session and
  answers SESSION_DESCRIPTION, from which the client synthesizes its remote SDP.
  """

  alias DiscordyVoice.Application.Deps
  alias DiscordyVoice.Contracts
  alias DiscordyVoice.Domain.{Codecs, VoiceSession}

  @spec run(VoiceSession.t(), term(), Deps.t()) ::
          {:ok, VoiceSession.t(), [{:reply, non_neg_integer(), term()}]} | {:close, atom()}
  def run(%VoiceSession{phase: :identified} = session, d, %Deps{} = deps) do
    with {:ok, offer} <- parse(d),
         {:ok, negotiated} <- Codecs.negotiate(offer.codecs, session.video),
         {:ok, local} <- deps.sfu.create_session(sfu_payload(session, offer, negotiated)) do
      session = %{
        session
        | phase: :negotiated,
          audio_codec: negotiated.audio_codec,
          video_codec: negotiated.video_codec
      }

      {:ok, session,
       [{:reply, Contracts.op(:session_description), description(session, local, deps)}]}
    else
      {:error, :sfu_unavailable} -> {:close, :sfu_unavailable}
      {:error, _reason} -> {:close, :invalid_payload}
    end
  end

  def run(%VoiceSession{phase: :negotiated}, _d, _deps), do: {:close, :already_authenticated}
  def run(_session, _d, _deps), do: {:close, :not_authenticated}

  defp parse(%{
         "protocol" => "webrtc",
         "data" => %{"ice_ufrag" => ufrag, "ice_pwd" => pwd, "fingerprint" => fp},
         "codecs" => codecs
       })
       when is_binary(ufrag) and is_binary(pwd) and is_binary(fp) do
    {:ok,
     %{ice_ufrag: ufrag, ice_pwd: pwd, fingerprint: normalize_fingerprint(fp), codecs: codecs}}
  end

  defp parse(_d), do: {:error, :invalid_payload}

  # Browsers report "sha-256 AB:CD:.."; a bare colon-hex value is accepted too.
  defp normalize_fingerprint(fp) do
    if String.contains?(fp, " "), do: String.trim(fp), else: "sha-256 " <> String.trim(fp)
  end

  defp sfu_payload(session, offer, negotiated) do
    layers =
      if negotiated.video_codec do
        for layer <- session.ssrcs.layers,
            do: %{
              rid: layer.rid,
              quality: layer.quality,
              ssrc: layer.ssrc,
              rtx_ssrc: layer.rtx_ssrc
            }
      else
        []
      end

    %{
      session_id: session.session_id,
      room_id: session.channel_id,
      user_id: session.user_id,
      ice_ufrag: offer.ice_ufrag,
      ice_pwd: offer.ice_pwd,
      remote_fingerprint: offer.fingerprint,
      audio_ssrc: session.ssrcs.audio,
      codecs: Enum.map(negotiated.codecs, &wire_codec/1),
      video_layers: layers
    }
  end

  defp wire_codec(codec) do
    base = %{
      name: codec.name,
      type: codec.type,
      priority: codec.priority,
      payload_type: codec.payload_type
    }

    if codec.rtx_payload_type,
      do: Map.put(base, :rtx_payload_type, codec.rtx_payload_type),
      else: base
  end

  defp description(session, local, deps) do
    %{
      ice_ufrag: local.local_ufrag,
      ice_pwd: local.local_pwd,
      fingerprint: local.fingerprint,
      candidates: [%{ip: deps.settings.public_ip, port: deps.settings.udp_port, protocol: "udp"}],
      audio_codec: session.audio_codec,
      video_codec: session.video_codec,
      e2ee:
        if(session.e2ee, do: %{protocol_version: Contracts.dave_protocol_version()}, else: nil)
    }
  end
end
