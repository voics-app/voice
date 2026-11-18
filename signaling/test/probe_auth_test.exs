defmodule DiscordyVoice.ProbeAuthTest do
  use ExUnit.Case, async: false

  alias DiscordyVoice.Application.Settings
  alias DiscordyVoice.{Conn, FakeSfu, Fixtures}
  alias DiscordyVoice.Infrastructure.{DrainControl, Hs256TokenVerifier}

  @probe_secret "probe-only-test-secret-0123456789abcdef"
  @probe_id "810083d1-78d6-4de4-b792-7da268949454"

  setup do
    start_supervised!(FakeSfu)
    stop_rooms()

    on_exit(fn ->
      stop_rooms()
      DrainControl.undrain()
    end)

    settings = Map.merge(Fixtures.settings(), %{probe_token_secret: @probe_secret})
    {:ok, deps: Fixtures.deps(settings: settings)}
  end

  test "probe credentials are disabled by default and must be distinct strong keys" do
    vars = %{
      "VOICE_REGION" => "test",
      "VOICE_NODE_ID" => "test",
      "VOICE_PUBLIC_WS_URL" => "ws://localhost:4100/voice",
      "VOICE_PUBLIC_IP" => "127.0.0.1",
      "ETCD_ENDPOINTS" => "localhost:2379",
      "VOICE_TOKEN_SECRET" => Fixtures.secret(),
      "INTERNAL_SERVICE_TOKEN" => "internal"
    }

    assert {:ok, settings} = Settings.from_env(vars)
    assert Map.get(settings, :probe_token_secret) == nil
    assert Map.get(settings, :probe_required) == false

    assert {:error, errors} =
             Settings.from_env(Map.put(vars, "VOICE_PROBE_TOKEN_SECRET", Fixtures.secret()))

    assert Enum.any?(errors, &String.contains?(&1, "distinct"))
    assert {:error, _} = Settings.from_env(Map.put(vars, "VOICE_PROBE_TOKEN_SECRET", "short"))
    assert {:error, _} = Settings.from_env(Map.put(vars, "VOICE_REQUIRE_PROBE", "perhaps"))

    assert {:ok, settings} =
             Settings.from_env(
               Map.merge(vars, %{
                 "VOICE_PROBE_TOKEN_SECRET" => @probe_secret,
                 "VOICE_REQUIRE_PROBE" => "true"
               })
             )

    assert settings.probe_required
    assert settings.probe_token_secret == @probe_secret
  end

  test "a probe key cannot mint ordinary call tokens and a call key cannot mint probes" do
    now = System.os_time(:second)
    claims = claims("101", "a", %{})
    token = Hs256TokenVerifier.sign(claims, Fixtures.secret())

    assert {:error, :bad_signature} =
             Hs256TokenVerifier.verify(token, Fixtures.secret(), @probe_secret, now)

    for purpose <- [nil, "call"] do
      claims =
        if purpose, do: Map.put(claims, "purpose", purpose), else: Map.delete(claims, "purpose")

      token = Hs256TokenVerifier.sign(claims, @probe_secret)

      assert {:error, :bad_signature} =
               Hs256TokenVerifier.verify(token, Fixtures.secret(), @probe_secret, now)
    end

    probe = Hs256TokenVerifier.sign(claims, @probe_secret)

    assert {:error, :probes_disabled} =
             Hs256TokenVerifier.verify(probe, Fixtures.secret(), nil, now)
  end

  test "probe claims require a short lifetime, a lease, an id and no E2EE" do
    now = System.os_time(:second)

    for overrides <- [
          %{"node_lease" => nil},
          %{"node_lease" => "0"},
          %{"node_lease" => 1},
          %{"probe_id" => "not-a-uuid"},
          %{"e2ee" => true},
          %{"iat" => now + 1},
          %{"exp" => now + 61},
          %{"purpose" => "admin"}
        ] do
      token = Hs256TokenVerifier.sign(claims("101", "a", overrides), @probe_secret)

      assert {:error, _} =
               Hs256TokenVerifier.verify(token, Fixtures.secret(), @probe_secret, now)
    end
  end

  test "paired probes join a private room and cannot enter the claimed user channel", %{
    deps: deps
  } do
    ordinary = Conn.start(:ordinary, deps)

    assert [{:reply, 2, _}] =
             Conn.send_frame(ordinary, 0, Fixtures.identify("user", "call", "800"))

    a = Conn.start(:probe_a, deps)
    b = Conn.start(:probe_b, deps)
    assert [{:reply, 2, _}] = Conn.send_frame(a, 0, identify("101", "a"))

    assert [{:reply, 2, _}, {:reply, 11, %{user_ids: ["101"]}}] =
             Conn.send_frame(b, 0, identify("102", "b"))

    assert_receive {:pushed, :probe_a, 11, %{user_ids: ["102"]}}
    refute_receive {:pushed, :ordinary, _, _}
    assert Conn.session(a).channel_id == "probe:1:" <> @probe_id

    assert [{:reply, 4, _}] = Conn.send_frame(a, 1, Fixtures.select_protocol())
    assert [{:create_session, payload}] = FakeSfu.calls()
    assert payload.room_id == "probe:1:" <> @probe_id
  end

  test "only authenticated probes of the current lease bypass draining", %{deps: deps} do
    DrainControl.drain()
    probe = Conn.start(:valid_probe, deps)
    assert [{:reply, 2, _}] = Conn.send_frame(probe, 0, identify("101", "a"))

    for {name, frame} <- [
          {:stale, identify("102", "b", %{"node_lease" => "2"})},
          {:video, Map.put(identify("103", "c"), "video", true)}
        ] do
      conn = Conn.start(name, deps)
      assert [{:close, :authentication_failed}] = Conn.send_frame(conn, 0, frame)
    end

    missing =
      Conn.start(:missing_registration, %{deps | registration: DiscordyVoice.UnregisteredNode})

    assert [{:close, :sfu_unavailable}] = Conn.send_frame(missing, 0, identify("104", "d"))
  end

  defp identify(user, session, overrides \\ %{}) do
    %{
      "server_id" => "900",
      "user_id" => user,
      "session_id" => session,
      "token" => Hs256TokenVerifier.sign(claims(user, session, overrides), @probe_secret),
      "video" => false
    }
  end

  defp claims(user, session, overrides) do
    Map.merge(
      %{
        "sub" => user,
        "sid" => session,
        "srv" => "900",
        "chn" => "800",
        "node" => "node-1",
        "e2ee" => false,
        "purpose" => "probe",
        "node_lease" => "1",
        "probe_id" => @probe_id,
        "iat" => System.os_time(:second),
        "exp" => System.os_time(:second) + 60
      },
      overrides
    )
  end

  defp stop_rooms do
    for {_, pid, _, _} <- DynamicSupervisor.which_children(DiscordyVoice.RoomSupervisor) do
      DynamicSupervisor.terminate_child(DiscordyVoice.RoomSupervisor, pid)
    end
  end
end
