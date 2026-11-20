defmodule DiscordyVoice.Application.Ports.SfuControl do
  @moduledoc "Commands to the SFU over its NDJSON control socket (P8)."

  @type local_description :: %{
          local_ufrag: String.t(),
          local_pwd: String.t(),
          fingerprint: String.t()
        }

  @callback create_session(payload :: map()) :: {:ok, local_description()} | {:error, term()}
  @callback set_publish_audio(session_id :: String.t(), allowed :: boolean()) :: :ok | {:error, term()}
  @callback set_publish_video(session_id :: String.t(), allowed :: boolean()) :: :ok | {:error, term()}
  @callback set_publish_permissions(session_id :: String.t(), update :: map()) :: :ok | {:error, term()}
  @callback close_session(session_id :: String.t()) :: :ok
  @callback set_sink_wants(session_id :: String.t(), wants :: %{String.t() => 0..100}) ::
              :ok | {:error, term()}
end
