defmodule DiscordyVoice.Application.Ports.Rooms do
  @moduledoc """
  The room registry: who is in which voice channel on this node, and message delivery between
  the members' connection processes.
  """

  alias DiscordyVoice.Domain.Room.Member

  @type pushed :: {:push, op :: non_neg_integer(), d :: term()}

  @doc "Adds the member; returns the members already present and the room's external sender key."
  @callback join(channel_id :: String.t(), Member.t()) ::
              {:ok, existing :: [Member.t()], external_sender :: binary()} | {:error, term()}
  @callback leave(channel_id :: String.t(), user_id :: String.t(), pid()) :: :ok
  @callback put_video(channel_id :: String.t(), user_id :: String.t(), video :: map()) :: :ok
  @doc "Delivers `{op, d}` to every other member."
  @callback broadcast(
              channel_id :: String.t(),
              from :: String.t(),
              op :: non_neg_integer(),
              d :: term()
            ) :: :ok
  @doc "Delivers `{op, d}` to the listed members."
  @callback send_to(
              channel_id :: String.t(),
              from :: String.t(),
              user_ids :: [String.t()],
              op :: non_neg_integer(),
              d :: term()
            ) :: :ok
end
