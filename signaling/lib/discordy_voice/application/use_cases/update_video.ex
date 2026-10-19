defmodule DiscordyVoice.Application.UseCases.UpdateVideo do
  @moduledoc """
  VIDEO: a publisher turns simulcast layers on or off. The server owns the SSRCs, so the relayed
  payload is rebuilt from the session's plan; only each layer's `active` flag comes from the client.
  """

  alias DiscordyVoice.Application.Deps
  alias DiscordyVoice.Contracts
  alias DiscordyVoice.Domain.{SsrcPlan, VoiceSession}

  @spec run(VoiceSession.t(), term(), Deps.t()) :: :ok | {:close, atom()}
  def run(%VoiceSession{video: false}, _d, _deps), do: :ok

  def run(%VoiceSession{} = session, d, deps) when is_map(d) do
    payload = payload(session, active_rids(d["streams"]))
    :ok = deps.rooms.put_video(session.channel_id, session.user_id, payload)
    deps.rooms.broadcast(session.channel_id, session.user_id, Contracts.op(:video), payload)
  end

  def run(_session, _d, _deps), do: {:close, :invalid_payload}

  @doc "The VIDEO payload describing `session` with the given active layers."
  @spec payload(VoiceSession.t(), MapSet.t(String.t())) :: map()
  def payload(%VoiceSession{ssrcs: plan} = session, active) do
    any? = MapSet.size(active) > 0

    %{
      user_id: session.user_id,
      audio_ssrc: plan.audio,
      video_ssrc: if(any?, do: SsrcPlan.primary_video(plan), else: 0),
      rtx_ssrc: if(any?, do: SsrcPlan.primary_rtx(plan), else: 0),
      streams:
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
    }
  end

  # A client-provided plan has one layer; any stream the client marks active refers to it.
  defp active?(%{layers: [_single]}, active, _rid), do: MapSet.size(active) > 0
  defp active?(_plan, active, rid), do: MapSet.member?(active, rid)

  defp active_rids(streams) when is_list(streams) do
    for %{"rid" => rid, "active" => true} <- streams, is_binary(rid), into: MapSet.new(), do: rid
  end

  defp active_rids(_streams), do: MapSet.new()
end
