defmodule DiscordyVoice.Domain.VoiceSession do
  @moduledoc "State of one voice WebSocket connection as it moves through the handshake."

  alias DiscordyVoice.Domain.SsrcPlan

  @type phase :: :connected | :identified | :negotiated

  defstruct phase: :connected,
            user_id: nil,
            session_id: nil,
            server_id: nil,
            channel_id: nil,
            e2ee: false,
            guarded_publish: false,
            video: false,
            ssrcs: nil,
            audio_codec: nil,
            video_codec: nil

  @type t :: %__MODULE__{
          phase: phase(),
          user_id: String.t() | nil,
          session_id: String.t() | nil,
          server_id: String.t() | nil,
          channel_id: String.t() | nil,
          e2ee: boolean(),
          guarded_publish: boolean(),
          video: boolean(),
          ssrcs: SsrcPlan.t() | nil,
          audio_codec: String.t() | nil,
          video_codec: String.t() | nil
        }

  @spec new() :: t()
  def new, do: %__MODULE__{}

  @doc "Whether the connection has completed IDENTIFY."
  @spec identified?(t()) :: boolean()
  def identified?(%__MODULE__{phase: phase}), do: phase in [:identified, :negotiated]
end
