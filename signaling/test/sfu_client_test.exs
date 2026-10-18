defmodule DiscordyVoice.SfuClientTest do
  use ExUnit.Case, async: false

  alias DiscordyVoice.FakeSfuServer
  alias DiscordyVoice.Infrastructure.{NodeLoad, SfuClient}

  @payload %{
    session_id: "sess-1",
    room_id: "room",
    user_id: "u1",
    ice_ufrag: "clientUfrag",
    ice_pwd: "clientIcePasswordClientIcePwd",
    remote_fingerprint: "sha-256 AB:CD",
    audio_ssrc: 10,
    codecs: [%{name: "opus", type: "audio", priority: 1, payload_type: 111}],
    video_layers: []
  }

  setup do
    {:ok, server} = FakeSfuServer.start_link()
    start_supervised!({SfuClient, address: {"127.0.0.1", FakeSfuServer.port(server)}})
    wait_until(&SfuClient.connected?/0)
    # The server has registered the connection once it has seen the initial sync.
    wait_until(fn -> commands(server, "sync") != [] end)
    {:ok, server: server}
  end

  defp wait_until(fun, attempts \\ 100) do
    cond do
      fun.() -> :ok
      attempts == 0 -> flunk("condition not reached")
      true -> Process.sleep(25) && wait_until(fun, attempts - 1)
    end
  end

  defp commands(server, cmd),
    do: server |> FakeSfuServer.received() |> Enum.filter(&(&1["cmd"] == cmd))

  test "syncs on connect and tracks connectivity", %{server: server} do
    assert [%{"session_ids" => []}] = commands(server, "sync")
    assert NodeLoad.sfu_connected?()
  end

  test "create_session returns the local description", %{server: server} do
    assert {:ok, %{local_ufrag: "fakeUfrag", local_pwd: _, fingerprint: "sha-256 AA:BB:CC"}} =
             SfuClient.create_session(@payload)

    assert [%{"session_id" => "sess-1", "room_id" => "room", "audio_ssrc" => 10}] =
             commands(server, "create_session")
  end

  test "sfu errors are returned with their code", %{server: server} do
    FakeSfuServer.reject_creates(server, "invalid_session")
    assert {:error, {:sfu, "invalid_session", "rejected"}} = SfuClient.create_session(@payload)
  end

  test "close_session and sink wants reach the sfu", %{server: server} do
    {:ok, _} = SfuClient.create_session(@payload)
    assert :ok = SfuClient.set_sink_wants("sess-1", %{"10" => 50})
    SfuClient.close_session("sess-1")
    wait_until(fn -> commands(server, "close_session") != [] end)

    assert [%{"wants" => %{"10" => 50}}] = commands(server, "set_sink_wants")
  end

  test "speaking and closure events are routed to the owning connection", %{server: server} do
    {:ok, _} = Registry.register(DiscordyVoice.SessionRegistry, "sess-1", nil)

    FakeSfuServer.push(server, %{evt: "speaking", session_id: "sess-1", ssrc: 10, speaking: true})
    assert_receive {:sfu, {:speaking, 10, true}}

    FakeSfuServer.push(server, %{
      evt: "session_closed",
      session_id: "sess-1",
      reason: "disconnected"
    })

    assert_receive {:sfu, {:closed, "disconnected"}}

    FakeSfuServer.push(server, %{evt: "session_closed", session_id: "sess-1", reason: "requested"})

    refute_receive {:sfu, {:closed, "requested"}}, 100
  end

  test "stats feed the egress gauge", %{server: server} do
    FakeSfuServer.push(server, %{
      evt: "stats",
      sessions: 1,
      rooms: 1,
      ingress_bps: 5,
      egress_bps: 4242
    })

    wait_until(fn -> NodeLoad.egress_bps() == 4242 end)
  end

  test "reconnects and replays sessions the restarted sfu lost", %{server: server} do
    {:ok, local} = SfuClient.create_session(@payload)

    FakeSfuServer.forget_sessions(server)
    FakeSfuServer.drop_connection(server)
    wait_until(fn -> length(commands(server, "sync")) == 2 end)
    wait_until(fn -> length(commands(server, "create_session")) == 2 end)

    [_first, replay] = commands(server, "create_session")
    assert replay["session_id"] == "sess-1"
    assert replay["local_ufrag"] == local.local_ufrag
    assert replay["local_pwd"] == local.local_pwd
    assert [_, %{"session_ids" => ["sess-1"]}] = commands(server, "sync")
  end

  test "sessions closed before a reconnect are not replayed", %{server: server} do
    {:ok, _} = SfuClient.create_session(@payload)
    SfuClient.close_session("sess-1")
    FakeSfuServer.drop_connection(server)
    wait_until(fn -> length(commands(server, "sync")) == 2 end)

    assert [_, %{"session_ids" => []}] = commands(server, "sync")
    assert length(commands(server, "create_session")) == 1
  end

  test "calls fail fast while the sfu is unreachable" do
    stop_supervised!(SfuClient)
    start_supervised!({SfuClient, address: {"127.0.0.1", 1}})
    assert {:error, :sfu_unavailable} = SfuClient.create_session(@payload)
    assert {:error, :sfu_unavailable} = SfuClient.set_sink_wants("x", %{})
    refute SfuClient.connected?()
  end
end
