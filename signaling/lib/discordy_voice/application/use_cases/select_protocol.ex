defmodule DiscordyVoice.Application.UseCases.SelectProtocol do
  @moduledoc """
  SELECT_PROTOCOL: turns the client's ICE/DTLS parameters and codecs into an SFU session and
  answers SESSION_DESCRIPTION, from which the client synthesizes its remote SDP.
  """

  alias DiscordyVoice.Application.{Deps, Settings}
  alias DiscordyVoice.Application.UseCases.UpdateVideo
  alias DiscordyVoice.Contracts
  alias DiscordyVoice.Domain.{Codecs, VoiceSession}

  @spec run(VoiceSession.t(), term(), Deps.t()) ::
          {:ok, VoiceSession.t(), [{:reply, non_neg_integer(), term()}]} | {:close, atom()}
  def run(%VoiceSession{phase: :identified} = session, d, %Deps{} = deps) do
    with {:ok, offer} <- parse(d),
         session = adopt_client_ssrcs(session, offer.ssrcs),
         {:ok, negotiated} <- Codecs.negotiate(offer.codecs, session.video),
         {:ok, local} <- deps.sfu.create_session(sfu_payload(session, offer, negotiated)) do
      session = %{
        session
        | phase: :negotiated,
          audio_codec: negotiated.audio_codec,
          video_codec: negotiated.video_codec
      }

      announce_soundboard(session, deps)

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
         "data" => %{"ice_ufrag" => ufrag, "ice_pwd" => pwd, "fingerprint" => fp} = data,
         "codecs" => codecs
       })
       when is_binary(ufrag) and is_binary(pwd) and is_binary(fp) do
    {:ok,
     %{
       ice_ufrag: ufrag,
       ice_pwd: pwd,
       fingerprint: normalize_fingerprint(fp),
       codecs: codecs,
       ssrcs: client_ssrcs(data["ssrcs"])
     }}
  end

  defp parse(_d), do: {:error, :invalid_payload}

  # Browsers choose their own SSRCs and cannot be told to use READY's, so a client may send the
  # ones from its offer. They replace the plan: one audio SSRC, a single (non-simulcast) camera
  # layer and, separately, the screen share stream.
  defp client_ssrcs(%{"audio" => audio} = ssrcs)
       when is_integer(audio) and audio in 1..0xFFFFFFFF,
       do: %{
         audio: audio,
         soundboard: valid_ssrc(ssrcs["soundboard"]),
         video: valid_ssrc(ssrcs["video"]),
         rtx: valid_ssrc(ssrcs["rtx"]),
         screen: valid_ssrc(ssrcs["screen"]),
         screen_rtx: valid_ssrc(ssrcs["screen_rtx"])
       }

  defp client_ssrcs(_ssrcs), do: nil

  defp valid_ssrc(value) when is_integer(value) and value in 1..0xFFFFFFFF, do: value
  defp valid_ssrc(_value), do: 0

  defp adopt_client_ssrcs(session, nil), do: session

  defp adopt_client_ssrcs(%VoiceSession{ssrcs: plan} = session, ssrcs) do
    layers =
      case {ssrcs.video, plan.layers} do
        {0, _layers} ->
          plan.layers

        {video, layers} ->
          [%{rid: List.last(layers).rid, quality: 100, ssrc: video, rtx_ssrc: ssrcs.rtx}]
      end

    screen = if ssrcs.screen > 0, do: %{ssrc: ssrcs.screen, rtx_ssrc: ssrcs.screen_rtx}

    %{
      session
      | ssrcs: %{
          plan
          | audio: ssrcs.audio,
            layers: layers,
            screen: screen,
            soundboard: if(ssrcs.soundboard > 0, do: ssrcs.soundboard)
        }
    }
  end

  # Publish the accepted plan immediately, including for audio-only calls.
  defp announce_soundboard(%VoiceSession{ssrcs: %{soundboard: nil}}, _deps), do: :ok

  defp announce_soundboard(session, deps) do
    payload = UpdateVideo.payload(session, MapSet.new(), false)
    :ok = deps.rooms.put_video(session.channel_id, session.user_id, payload)
    deps.rooms.broadcast(session.channel_id, session.user_id, Contracts.op(:video), payload)
  end

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
      soundboard_ssrc: session.ssrcs.soundboard,
      codecs: Enum.map(negotiated.codecs, &wire_codec/1),
      video_layers: layers,
      screen: if(negotiated.video_codec, do: session.ssrcs.screen)
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
      candidates:
        Enum.map(
          Settings.candidate_ips(deps.settings),
          &%{ip: &1, port: deps.settings.udp_port, protocol: "udp"}
        ),
      audio_codec: session.audio_codec,
      video_codec: session.video_codec,
      e2ee:
        if(session.e2ee, do: %{protocol_version: Contracts.dave_protocol_version()}, else: nil)
    }
  end
end
