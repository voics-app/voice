defmodule DiscordyVoice.Application.UseCases.UpdateVideo do
  @moduledoc """
  VIDEO: a publisher turns simulcast layers or the screen share on or off. The server owns the
  SSRCs, so the relayed payload is rebuilt from the session's plan; only each stream's `active`
  flag comes from the client. Camera and screen are told apart by the stream `type`.
  """

  alias DiscordyVoice.Application.Deps
  alias DiscordyVoice.Contracts
  alias DiscordyVoice.Domain.{SsrcPlan, VoiceSession}

  @spec run(VoiceSession.t(), term(), Deps.t()) :: :ok | {:close, atom()}
  def run(%VoiceSession{video: false}, _d, _deps), do: :ok

  def run(%VoiceSession{} = session, d, deps) when is_map(d) do
    payload = payload(session, active_rids(d["streams"]), screen_active?(d["streams"]))
    :ok = deps.rooms.put_video(session.channel_id, session.user_id, payload)
    deps.rooms.broadcast(session.channel_id, session.user_id, Contracts.op(:video), payload)
  end

  def run(_session, _d, _deps), do: {:close, :invalid_payload}

  @doc "The VIDEO payload describing `session` with the given active camera layers and screen."
  @spec payload(VoiceSession.t(), MapSet.t(String.t()), boolean()) :: map()
  def payload(%VoiceSession{ssrcs: plan} = session, active, screen_active? \\ false) do
    any? = MapSet.size(active) > 0

    camera =
      for layer <- plan.layers do
        %{
          type: "video",
          rid: layer.rid,
          quality: layer.quality,
          ssrc: layer.ssrc,
          rtx_ssrc: layer.rtx_ssrc,
          active: active?(plan, active, layer.rid)
        }
      end

    %{
      user_id: session.user_id,
      audio_ssrc: plan.audio,
      video_ssrc: if(any?, do: SsrcPlan.primary_video(plan), else: 0),
      rtx_ssrc: if(any?, do: SsrcPlan.primary_rtx(plan), else: 0),
      streams: camera ++ screen_stream(plan.screen, screen_active?)
    }
  end

  defp screen_stream(nil, _active?), do: []

  defp screen_stream(screen, active?),
    do: [
      %{
        type: "screen",
        rid: "screen",
        quality: 100,
        ssrc: screen.ssrc,
        rtx_ssrc: screen.rtx_ssrc,
        active: active?
      }
    ]

  # A client-provided plan has one layer; any stream the client marks active refers to it.
  defp active?(%{layers: [_single]}, active, _rid), do: MapSet.size(active) > 0
  defp active?(_plan, active, rid), do: MapSet.member?(active, rid)

  defp active_rids(streams) when is_list(streams) do
    for %{"rid" => rid, "active" => true} = stream <- streams,
        is_binary(rid),
        stream["type"] != "screen",
        into: MapSet.new(),
        do: rid
  end

  defp active_rids(_streams), do: MapSet.new()

  defp screen_active?(streams) when is_list(streams),
    do: Enum.any?(streams, &match?(%{"type" => "screen", "active" => true}, &1))

  defp screen_active?(_streams), do: false
end
