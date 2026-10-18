defmodule DiscordyVoice.Application.UseCases.Dispatch do
  @moduledoc "Routes a decoded client frame to its use case, enforcing the handshake order."

  alias DiscordyVoice.Application.Deps

  alias DiscordyVoice.Application.UseCases.{
    Identify,
    RelayDave,
    RelaySpeaking,
    SelectProtocol,
    SetMediaSinkWants,
    UpdateVideo
  }

  alias DiscordyVoice.Contracts
  alias DiscordyVoice.Domain.{Dave, VoiceSession}
  alias DiscordyVoice.Domain.Protocol.Frame

  @type effect :: {:reply, non_neg_integer(), term()} | {:close, atom()}

  @doc "Applies `frame`; returns the new session state and the effects the connection must perform."
  @spec run(VoiceSession.t(), Frame.t(), Deps.t()) :: {VoiceSession.t(), [effect()]}
  def run(session, %Frame{op: op, d: d}, deps) do
    cond do
      op == Contracts.op(:heartbeat) -> {session, heartbeat_ack(d)}
      op == Contracts.op(:identify) -> session |> Identify.run(d, deps) |> settle(session)
      VoiceSession.identified?(session) -> identified(session, op, d, deps)
      client_op?(op) -> {session, [{:close, :not_authenticated}]}
      true -> {session, []}
    end
  end

  defp identified(session, op, d, deps) do
    cond do
      op == Contracts.op(:select_protocol) ->
        session |> SelectProtocol.run(d, deps) |> settle(session)

      op == Contracts.op(:speaking) ->
        session |> RelaySpeaking.from_client(d, deps) |> done(session)

      op == Contracts.op(:video) ->
        session |> UpdateVideo.run(d, deps) |> done(session)

      op == Contracts.op(:media_sink_wants) ->
        session |> SetMediaSinkWants.run(d, deps) |> done(session)

      Dave.dave_op?(op) ->
        session |> RelayDave.run(op, d, deps) |> done(session)

      true ->
        {session, []}
    end
  end

  defp client_op?(op) do
    op in [
      Contracts.op(:select_protocol),
      Contracts.op(:speaking),
      Contracts.op(:video),
      Contracts.op(:media_sink_wants)
    ] or Dave.dave_op?(op)
  end

  defp heartbeat_ack(%{"t" => nonce}), do: [{:reply, Contracts.op(:heartbeat_ack), %{t: nonce}}]
  defp heartbeat_ack(_d), do: [{:reply, Contracts.op(:heartbeat_ack), %{t: nil}}]

  defp settle({:ok, new_session, effects}, _old), do: {new_session, effects}
  defp settle({:close, reason}, old), do: {old, [{:close, reason}]}

  defp done(:ok, session), do: {session, []}
  defp done({:close, reason}, session), do: {session, [{:close, reason}]}
end
