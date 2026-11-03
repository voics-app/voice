defmodule DiscordyVoice.WebsocketTest do
  @moduledoc "End to end over real sockets: gun WebSocket clients, the endpoint, the SFU client and a scripted SFU."

  use ExUnit.Case, async: false

  alias DiscordyVoice.{Contracts, FakeNodeRegistry, FakeSfuServer, Fixtures}
  alias DiscordyVoice.Infrastructure.{DrainControl, NodePresence, SfuClient}
  alias DiscordyVoice.Interface.{Endpoint, Metrics}

  setup do
    {:ok, sfu} = FakeSfuServer.start_link()
    start_supervised!({SfuClient, address: {"127.0.0.1", FakeSfuServer.port(sfu)}})
    start_supervised!(FakeNodeRegistry)

    start_supervised!(
      {NodePresence, settings: Fixtures.settings(), registry: FakeNodeRegistry, interval_ms: 20}
    )

    Fixtures.deps(sfu: SfuClient)

    Application.put_env(:discordy_voice, Endpoint,
      adapter: Bandit.PhoenixAdapter,
      http: [ip: {127, 0, 0, 1}, port: 0],
      server: true
    )

    start_supervised!(Metrics)
    start_supervised!(Endpoint)
    {:ok, {_ip, port}} = Bandit.PhoenixAdapter.server_info(Endpoint, :http)
    wait_until(fn -> SfuClient.connected?() and NodePresence.registered?() end)

    on_exit(fn ->
      DrainControl.undrain()

      for {_, pid, _, _} <- DynamicSupervisor.which_children(DiscordyVoice.RoomSupervisor),
          do: DynamicSupervisor.terminate_child(DiscordyVoice.RoomSupervisor, pid)
    end)

    {:ok, port: port, sfu: sfu}
  end

  defp wait_until(fun, attempts \\ 100) do
    cond do
      fun.() -> :ok
      attempts == 0 -> flunk("condition not reached")
      true -> Process.sleep(25) && wait_until(fun, attempts - 1)
    end
  end

  defp connect(port) do
    {:ok, conn} = :gun.open(~c"127.0.0.1", port, %{protocols: [:http]})
    {:ok, :http} = :gun.await_up(conn)
    stream = :gun.ws_upgrade(conn, "/voice")
    assert_receive {:gun_upgrade, ^conn, ^stream, ["websocket"], _}, 2_000
    {conn, stream}
  end

  defp send_op({conn, stream}, op, d) do
    :gun.ws_send(conn, stream, {:text, Jason.encode!(%{op: op, d: d})})
  end

  defp recv({conn, stream}, timeout \\ 2_000) do
    receive do
      {:gun_ws, ^conn, ^stream, {:text, text}} -> Jason.decode!(text)
      {:gun_ws, ^conn, ^stream, {:close, code, reason}} -> {:close, code, reason}
      {:gun_ws, ^conn, ^stream, :close} -> {:close, nil, nil}
    after
      timeout -> :timeout
    end
  end

  defp http_get(port, path, headers \\ []) do
    {:ok, conn} = :gun.open(~c"127.0.0.1", port, %{protocols: [:http]})
    {:ok, _} = :gun.await_up(conn)
    ref = :gun.get(conn, path, headers)
    {:response, fin, status, _headers} = :gun.await(conn, ref)
    body = if fin == :nofin, do: elem(:gun.await_body(conn, ref), 1), else: ""
    :gun.close(conn)
    {status, body}
  end

  defp http_post(port, path, headers) do
    {:ok, conn} = :gun.open(~c"127.0.0.1", port, %{protocols: [:http]})
    {:ok, _} = :gun.await_up(conn)
    ref = :gun.post(conn, path, headers, "")
    {:response, fin, status, _} = :gun.await(conn, ref)
    body = if fin == :nofin, do: elem(:gun.await_body(conn, ref), 1), else: ""
    :gun.close(conn)
    {status, body}
  end

  test "sends HELLO, then runs the full handshake", %{port: port, sfu: sfu} do
    ws = connect(port)
    assert %{"op" => 8, "d" => %{"heartbeat_interval" => 13_750}} = recv(ws)

    send_op(ws, 0, Fixtures.identify("alice", "sess-alice", "chan-1", video: true))

    assert %{"op" => 2, "d" => %{"ssrc" => ssrc, "ip" => "203.0.113.7", "streams" => [_, _, _]}} =
             recv(ws)

    send_op(ws, 1, Fixtures.select_protocol(video: true))

    assert %{
             "op" => 4,
             "d" => %{
               "ice_ufrag" => "fakeUfrag",
               "fingerprint" => "sha-256 AA:BB:CC",
               "video_codec" => "VP9",
               "e2ee" => nil
             }
           } =
             recv(ws)

    [create] = sfu |> FakeSfuServer.received() |> Enum.filter(&(&1["cmd"] == "create_session"))
    assert create["audio_ssrc"] == ssrc and create["room_id"] == "chan-1"

    send_op(ws, 3, %{"t" => 99})
    assert %{"op" => 6, "d" => %{"t" => 99}} = recv(ws)
  end

  test "the streamer is told who watches its screen share", %{port: port, sfu: sfu} do
    alice = connect(port)
    recv(alice)
    send_op(alice, 0, Fixtures.identify("alice", "sess-alice", "chan-1", video: true))
    %{"op" => 2} = recv(alice)

    ssrcs = %{"audio" => 111, "video" => 222, "rtx" => 333, "screen" => 444, "screen_rtx" => 555}
    offer = put_in(Fixtures.select_protocol(video: true), ["data", "ssrcs"], ssrcs)
    send_op(alice, 1, offer)
    %{"op" => 4} = recv(alice)

    FakeSfuServer.push(sfu, %{
      evt: "stream_viewers",
      session_id: "sess-alice",
      ssrc: 444,
      viewers: ["bob"]
    })

    assert %{"op" => 40, "d" => %{"ssrc" => 444, "user_ids" => ["bob"]}} = recv(alice)

    # A report for an SSRC that is not this session's screen share is ignored.
    FakeSfuServer.push(sfu, %{
      evt: "stream_viewers",
      session_id: "sess-alice",
      ssrc: 999,
      viewers: ["bob"]
    })

    assert :timeout = recv(alice, 200)
  end

  test "two users see each other, speaking and disconnects", %{port: port, sfu: sfu} do
    alice = connect(port)
    recv(alice)
    send_op(alice, 0, Fixtures.identify("alice", "sess-alice", "chan-1"))
    %{"op" => 2, "d" => %{"ssrc" => alice_ssrc}} = recv(alice)
    send_op(alice, 1, Fixtures.select_protocol())
    %{"op" => 4} = recv(alice)

    bob = connect(port)
    recv(bob)
    send_op(bob, 0, Fixtures.identify("bob", "sess-bob", "chan-1"))
    assert %{"op" => 2} = recv(bob)
    assert %{"op" => 11, "d" => %{"user_ids" => ["alice"]}} = recv(bob)
    assert %{"op" => 11, "d" => %{"user_ids" => ["bob"]}} = recv(alice)

    send_op(alice, 5, %{"speaking" => 1, "ssrc" => alice_ssrc})

    assert %{"op" => 5, "d" => %{"speaking" => 1, "user_id" => "alice", "ssrc" => ^alice_ssrc}} =
             recv(bob)

    # The SFU's own audio-level detection is relayed the same way.
    FakeSfuServer.push(sfu, %{
      evt: "speaking",
      session_id: "sess-alice",
      ssrc: alice_ssrc,
      speaking: false
    })

    assert %{"op" => 5, "d" => %{"speaking" => 0, "user_id" => "alice"}} = recv(bob)

    :gun.ws_send(elem(bob, 0), elem(bob, 1), :close)
    assert %{"op" => 13, "d" => %{"user_id" => "bob"}} = recv(alice)

    wait_until(fn ->
      Enum.any?(
        FakeSfuServer.received(sfu),
        &(&1["cmd"] == "close_session" and &1["session_id"] == "sess-bob")
      )
    end)
  end

  test "closes with 4004 on a bad token and 4003 when not identified", %{port: port} do
    ws = connect(port)
    recv(ws)
    send_op(ws, 0, %{Fixtures.identify("a", "s", "c") | "token" => "bad.token.here"})
    assert {:close, 4004, _} = recv(ws)

    ws = connect(port)
    recv(ws)
    send_op(ws, 5, %{"speaking" => 1})
    assert {:close, 4003, _} = recv(ws)
  end

  test "closes with 4012 on garbage frames", %{port: port} do
    {conn, stream} = ws = connect(port)
    recv(ws)
    :gun.ws_send(conn, stream, {:text, "not json"})
    assert {:close, 4012, _} = recv(ws)
  end

  test "a draining node answers new identifies with 4011", %{port: port} do
    DrainControl.drain()
    ws = connect(port)
    recv(ws)
    send_op(ws, 0, Fixtures.identify("a", "s", "c"))
    assert {:close, code, _} = recv(ws)
    assert code == Contracts.close_code(:server_draining)
  end

  test "an sfu-side closure disconnects the client", %{port: port, sfu: sfu} do
    ws = connect(port)
    recv(ws)
    send_op(ws, 0, Fixtures.identify("a", "sess-a", "c"))
    recv(ws)

    FakeSfuServer.push(sfu, %{evt: "session_closed", session_id: "sess-a", reason: "disconnected"})

    assert {:close, 4014, _} = recv(ws)
  end

  test "health, readiness and metrics endpoints", %{port: port} do
    assert {200, "ok"} = http_get(port, "/healthz")

    assert {200, ready} = http_get(port, "/readyz")
    assert %{"sfu" => true, "discovery" => true, "draining" => false} = Jason.decode!(ready)

    Metrics.measure()
    assert {200, metrics} = http_get(port, "/metrics")
    assert metrics =~ "discordy_voice_node_users"
    assert metrics =~ "discordy_voice_node_sfu_connected 1"

    assert {404, _} = http_get(port, "/nope")
  end

  test "internal drain API requires the internal token", %{port: port} do
    assert {401, _} = http_post(port, "/internal/v1/drain", [])
    assert {401, _} = http_post(port, "/internal/v1/drain", [{"x-internal-token", "wrong"}])
    refute DrainControl.draining?()

    assert {200, body} =
             http_post(port, "/internal/v1/drain", [{"x-internal-token", "internal-secret"}])

    assert %{"draining" => true} = Jason.decode!(body)
    assert DrainControl.draining?()

    assert {200, body} =
             http_post(port, "/internal/v1/undrain", [{"x-internal-token", "internal-secret"}])

    assert %{"draining" => false} = Jason.decode!(body)
  end
end
