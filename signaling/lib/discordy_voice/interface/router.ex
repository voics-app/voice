defmodule DiscordyVoice.Interface.Router do
  @moduledoc "HTTP surface: the voice WebSocket, health and metrics, and the internal drain API."

  use Plug.Router

  alias DiscordyVoice.Application.Deps
  alias DiscordyVoice.Infrastructure.{DrainControl, NodeLoad, NodePresence, SfuClient}
  alias DiscordyVoice.Interface.VoiceSocket

  plug :match
  plug :dispatch

  get "/healthz" do
    send_resp(conn, 200, "ok")
  end

  get "/readyz" do
    checks = %{
      sfu: SfuClient.connected?(),
      discovery: NodePresence.registered?(),
      draining: DrainControl.draining?()
    }

    status = if checks.sfu and checks.discovery, do: 200, else: 503
    json(conn, status, checks)
  end

  get "/metrics" do
    conn
    |> put_resp_content_type("text/plain")
    |> send_resp(200, TelemetryMetricsPrometheus.Core.scrape(:discordy_voice_metrics))
  end

  get "/voice" do
    conn
    |> WebSockAdapter.upgrade(VoiceSocket, [], timeout: 120_000, max_frame_size: 262_144)
    |> halt()
  end

  post "/internal/v1/drain" do
    internal(conn, fn -> DrainControl.drain() end)
  end

  post "/internal/v1/undrain" do
    internal(conn, fn -> DrainControl.undrain() end)
  end

  post "/internal/v1/sessions/:session_id/publish-permissions" do
    deps = Deps.current()

    with true <- authorized?(conn, deps.settings.internal_token),
         [node_id] <- get_req_header(conn, "x-voice-node-id"),
         true <- node_id == deps.settings.node_id,
         [lease] <- get_req_header(conn, "x-voice-registration-lease"),
         {:ok, ^lease} <- deps.registration.lease_id(),
         {:ok, body, conn} <- read_body(conn, length: 4096),
         {:ok, payload} <- Jason.decode(body),
         {:ok, update} <- permission_update(payload),
         :ok <- deps.sfu.set_publish_permissions(session_id, update),
         {:ok, ^lease} <- deps.registration.lease_id() do
      json(conn, 200, %{applied: true, session_id: session_id, revision: update.revision})
    else
      _ -> json(conn, 503, %{error: "permission_update_failed"})
    end
  end

  # Revoke-only until a leased, ordered grant protocol is ready. A stale registration cannot
  # operate on a replacement node. Both denials are attempted even when one command fails.
  post "/internal/v1/sessions/:session_id/revoke" do
    deps = Deps.current()
    with true <- authorized?(conn, deps.settings.internal_token),
         [node_id] <- get_req_header(conn, "x-voice-node-id"),
         true <- node_id == deps.settings.node_id,
         [lease] <- get_req_header(conn, "x-voice-registration-lease"),
         {:ok, ^lease} <- deps.registration.lease_id() do
      audio = deps.sfu.set_publish_audio(session_id, false)
      video = deps.sfu.set_publish_video(session_id, false)
      if audio == :ok and video == :ok,
        do: json(conn, 200, %{revoked: true}),
        else: json(conn, 503, %{error: "sfu_unavailable"})
    else
      _ -> json(conn, 403, %{error: "invalid_control_identity"})
    end
  end

  get "/internal/v1/status" do
    internal(conn, fn -> :ok end)
  end

  match _ do
    send_resp(conn, 404, "not found")
  end

  defp internal(conn, action) do
    expected = Deps.current().settings.internal_token

    if authorized?(conn, expected) do
      action.()

      json(conn, 200, %{
        draining: DrainControl.draining?(),
        users: NodeLoad.users(),
        rooms: NodeLoad.rooms()
      })
    else
      json(conn, 401, %{error: "unauthorized"})
    end
  end

  defp authorized?(conn, expected) do
    case get_req_header(conn, "x-internal-token") do
      [given] -> byte_size(given) == byte_size(expected) and :crypto.hash_equals(given, expected)
      _ -> false
    end
  end

  defp permission_update(%{"revision" => revision, "expires_at" => expires_at,
         "can_publish_audio" => audio, "can_publish_video" => video,
         "can_publish_soundboard" => soundboard})
       when is_integer(revision) and revision >= 0 and is_integer(expires_at) and expires_at > 0 and
              is_boolean(audio) and is_boolean(video) and is_boolean(soundboard) and
              (not soundboard or audio) do
    {:ok, %{revision: revision, expires_at: expires_at, can_publish_audio: audio,
      can_publish_video: video, can_publish_soundboard: soundboard}}
  end

  defp permission_update(_), do: {:error, :invalid_permissions}

  defp json(conn, status, body) do
    conn
    |> put_resp_content_type("application/json")
    |> send_resp(status, Jason.encode!(body))
  end
end
