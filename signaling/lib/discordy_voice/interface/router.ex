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

  get "/internal/v1/status" do
    internal(conn, fn -> :ok end)
  end

  match _ do
    send_resp(conn, 404, "not found")
  end

  defp internal(conn, action) do
    expected = Deps.current().settings.internal_token

    with [given] <- get_req_header(conn, "x-internal-token"),
         true <- byte_size(given) == byte_size(expected),
         true <- :crypto.hash_equals(given, expected) do
      action.()

      json(conn, 200, %{
        draining: DrainControl.draining?(),
        users: NodeLoad.users(),
        rooms: NodeLoad.rooms()
      })
    else
      _ -> json(conn, 401, %{error: "unauthorized"})
    end
  end

  defp json(conn, status, body) do
    conn
    |> put_resp_content_type("application/json")
    |> send_resp(status, Jason.encode!(body))
  end
end
