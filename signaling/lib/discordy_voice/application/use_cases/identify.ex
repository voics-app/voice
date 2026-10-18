defmodule DiscordyVoice.Application.UseCases.Identify do
  @moduledoc """
  IDENTIFY: authenticates the voice token minted by the gateway, joins the member to its channel
  room and answers READY. Existing members are announced to the newcomer, the newcomer to them.
  """

  alias DiscordyVoice.Application.Deps
  alias DiscordyVoice.Contracts
  alias DiscordyVoice.Domain.{Dave, Room.Member, SsrcPlan, VoiceSession}

  @type effect :: {:reply, non_neg_integer(), term()} | {:close, atom()}

  @spec run(VoiceSession.t(), term(), Deps.t()) ::
          {:ok, VoiceSession.t(), [effect()]} | {:close, atom()}
  def run(%VoiceSession{phase: :connected} = session, d, %Deps{} = deps) do
    with {:ok, request} <- parse(d),
         {:ok, claims} <- authenticate(request, deps),
         :ok <- check_node(claims, deps),
         :ok <- check_not_draining(deps) do
      join(session, request, claims, deps)
    end
  end

  def run(_session, _d, _deps), do: {:close, :already_authenticated}

  defp parse(%{"server_id" => s, "user_id" => u, "session_id" => sid, "token" => t} = d)
       when is_binary(s) and is_binary(u) and is_binary(sid) and is_binary(t) do
    case Map.get(d, "video", false) do
      video when is_boolean(video) ->
        {:ok, %{server_id: s, user_id: u, session_id: sid, token: t, video: video}}

      _ ->
        {:close, :invalid_payload}
    end
  end

  defp parse(_d), do: {:close, :invalid_payload}

  defp authenticate(request, deps) do
    with {:ok, claims} <- deps.token_verifier.verify(request.token),
         true <- claims["sub"] == request.user_id,
         true <- claims["sid"] == request.session_id,
         true <- claims["srv"] == request.server_id,
         true <- is_binary(claims["chn"]) do
      {:ok, claims}
    else
      _ -> {:close, :authentication_failed}
    end
  end

  defp check_node(%{"node" => node}, %Deps{settings: %{node_id: node}}), do: :ok
  defp check_node(_claims, _deps), do: {:close, :wrong_node}

  defp check_not_draining(deps),
    do: if(deps.drain.draining?(), do: {:close, :server_draining}, else: :ok)

  defp join(session, request, claims, deps) do
    plan = deps.ssrc.allocate()
    channel_id = claims["chn"]

    member = %Member{
      user_id: request.user_id,
      session_id: request.session_id,
      pid: self(),
      audio_ssrc: plan.audio
    }

    case deps.rooms.join(channel_id, member) do
      {:ok, existing, external_sender} ->
        session = %{
          session
          | phase: :identified,
            user_id: request.user_id,
            session_id: request.session_id,
            server_id: request.server_id,
            channel_id: channel_id,
            e2ee: claims["e2ee"] == true,
            video: request.video,
            ssrcs: plan
        }

        {:ok, session, replies(session, existing, external_sender, deps)}

      {:error, _reason} ->
        {:close, :sfu_unavailable}
    end
  end

  defp replies(session, existing, external_sender, deps) do
    settings = deps.settings

    ready =
      {:reply, Contracts.op(:ready),
       %{
         ssrc: session.ssrcs.audio,
         ip: settings.public_ip,
         port: settings.udp_port,
         modes: Contracts.modes(),
         streams: SsrcPlan.streams(session.ssrcs, session.video)
       }}

    announce =
      case existing do
        [] ->
          []

        members ->
          [{:reply, Contracts.op(:clients_connect), %{user_ids: Enum.map(members, & &1.user_id)}}]
      end

    videos =
      for %Member{video: video} when is_map(video) <- existing,
          do: {:reply, Contracts.op(:video), video}

    dave =
      if session.e2ee do
        [
          {:reply, Contracts.op(:dave_mls_external_sender),
           Dave.external_sender(session.channel_id, external_sender)}
        ]
      else
        []
      end

    [ready] ++ announce ++ videos ++ dave
  end
end
