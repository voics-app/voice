defmodule DiscordyVoice.UseCasesTest do
  use ExUnit.Case, async: false

  alias DiscordyVoice.{Conn, FakeSfu, Fixtures}
  alias DiscordyVoice.Infrastructure.{DrainControl, NodeLoad}

  setup do
    start_supervised!(FakeSfu)
    stop_rooms()
    on_exit(&stop_rooms/0)
    deps = Fixtures.deps()
    on_exit(fn -> DrainControl.undrain() end)
    {:ok, deps: deps}
  end

  defp stop_rooms do
    for {_, pid, _, _} <- DynamicSupervisor.which_children(DiscordyVoice.RoomSupervisor) do
      DynamicSupervisor.terminate_child(DiscordyVoice.RoomSupervisor, pid)
    end
  end

  defp join(name, user, channel, deps, opts \\ []) do
    conn = Conn.start(name, deps)
    effects = Conn.send_frame(conn, 0, Fixtures.identify(user, "sess-" <> user, channel, opts))
    {conn, effects}
  end

  defp drain_messages(timeout \\ 100) do
    receive do
      msg -> [msg | drain_messages(timeout)]
    after
      timeout -> []
    end
  end

  describe "IDENTIFY" do
    test "answers READY with the server-assigned ssrc and the UDP endpoint", %{deps: deps} do
      {conn, [{:reply, 2, ready}]} = join(:a, "a", "ch-1", deps)

      assert ready.ip == "203.0.113.7" and ready.port == 50_000
      assert ready.modes == ["dtls-srtp"]
      assert ready.streams == []
      assert ready.ssrc == Conn.session(conn).ssrcs.audio
    end

    test "advertises simulcast layers to video publishers", %{deps: deps} do
      {_conn, [{:reply, 2, ready}]} = join(:a, "a", "ch-1", deps, video: true)
      assert [%{rid: "q", type: "video"}, %{rid: "h"}, %{rid: "f"}] = ready.streams
      assert Enum.all?(ready.streams, &(&1.ssrc != &1.rtx_ssrc))
    end

    test "rejects tokens that do not match the identify fields", %{deps: deps} do
      conn = Conn.start(:a, deps)
      bad = Fixtures.identify("a", "sess-a", "ch-1", claims: %{"sub" => "someone-else"})
      assert [{:close, :authentication_failed}] = Conn.send_frame(conn, 0, bad)
    end

    test "rejects expired tokens, forged tokens and tokens for another node", %{deps: deps} do
      conn = Conn.start(:a, deps)
      expired = Fixtures.identify("a", "s", "c", claims: %{"exp" => 1})
      assert [{:close, :authentication_failed}] = Conn.send_frame(conn, 0, expired)

      forged = %{Fixtures.identify("a", "s", "c") | "token" => "x.y.z"}
      assert [{:close, :authentication_failed}] = Conn.send_frame(conn, 0, forged)

      other = Fixtures.identify("a", "s", "c", claims: %{"node" => "node-2"})
      assert [{:close, :wrong_node}] = Conn.send_frame(conn, 0, other)
    end

    test "rejects malformed payloads and second identifies", %{deps: deps} do
      conn = Conn.start(:a, deps)
      assert [{:close, :invalid_payload}] = Conn.send_frame(conn, 0, %{"user_id" => "a"})
      assert [{:reply, 2, _}] = Conn.send_frame(conn, 0, Fixtures.identify("a", "s", "c"))

      assert [{:close, :already_authenticated}] =
               Conn.send_frame(conn, 0, Fixtures.identify("a", "s", "c"))
    end

    test "an unregistered replica refuses even a valid token for its node", %{deps: deps} do
      conn = Conn.start(:unregistered, %{deps | registration: DiscordyVoice.UnregisteredNode})

      assert [{:close, :sfu_unavailable}] =
               Conn.send_frame(conn, 0, Fixtures.identify("a", "s", "c"))
    end

    test "a draining node refuses new sessions but keeps existing ones", %{deps: deps} do
      {conn, _} = join(:a, "a", "ch-1", deps)
      DrainControl.drain()

      {_late, effects} = join(:b, "b", "ch-1", deps)
      assert effects == [{:close, :server_draining}]

      assert [{:reply, 6, %{t: 1}}] = Conn.send_frame(conn, 3, %{"t" => 1})
    end

    test "frames before identify are refused, heartbeats are not", %{deps: deps} do
      conn = Conn.start(:a, deps)
      assert [{:close, :not_authenticated}] = Conn.send_frame(conn, 5, %{"speaking" => 1})
      assert [{:reply, 6, %{t: 42}}] = Conn.send_frame(conn, 3, %{"t" => 42})
      assert [] = Conn.send_frame(conn, 99, %{})
    end
  end

  describe "room announcements" do
    test "newcomers and existing members learn about each other", %{deps: deps} do
      {_a, _} = join(:a, "a", "ch-1", deps)
      {_b, [{:reply, 2, _}, {:reply, 11, %{user_ids: ["a"]}}]} = join(:b, "b", "ch-1", deps)

      assert_receive {:pushed, :a, 11, %{user_ids: ["b"]}}
    end

    test "rooms are isolated per channel", %{deps: deps} do
      {_a, _} = join(:a, "a", "ch-1", deps)
      {_b, [{:reply, 2, _}]} = join(:b, "b", "ch-2", deps)
      refute_receive {:pushed, _, _, _}, 100
    end

    test "leaving closes the sfu session and tells the others", %{deps: deps} do
      {_a, _} = join(:a, "a", "ch-1", deps)
      {b, _} = join(:b, "b", "ch-1", deps)
      drain_messages()

      Conn.leave(b)
      assert_receive {:pushed, :a, 13, %{user_id: "b"}}
      assert {:close_session, "sess-b"} in FakeSfu.calls()
    end

    test "a crashed connection still leaves", %{deps: deps} do
      {_a, _} = join(:a, "a", "ch-1", deps)
      {b, _} = join(:b, "b", "ch-1", deps)
      drain_messages()

      Conn.stop(b)
      assert_receive {:pushed, :a, 13, %{user_id: "b"}}
      assert {:close_session, "sess-b"} in FakeSfu.calls()
    end

    test "identifying again as the same user evicts the old connection", %{deps: deps} do
      {_old, _} = join(:old, "a", "ch-1", deps)
      conn = Conn.start(:new, deps)

      assert [{:reply, 2, _}] =
               Conn.send_frame(conn, 0, Fixtures.identify("a", "sess-new", "ch-1"))

      assert_receive {:evicted, :old}
      assert {:close_session, "sess-a"} in FakeSfu.calls()
    end

    test "the room and user gauges follow membership", %{deps: deps} do
      users = NodeLoad.users()
      rooms = NodeLoad.rooms()
      {a, _} = join(:a, "a", "ch-gauge", deps)
      assert NodeLoad.users() == users + 1 and NodeLoad.rooms() == rooms + 1

      Conn.leave(a)
      Process.sleep(50)
      assert NodeLoad.users() == users and NodeLoad.rooms() == rooms
    end
  end

  describe "SELECT_PROTOCOL" do
    test "creates the sfu session and answers SESSION_DESCRIPTION", %{deps: deps} do
      {conn, _} = join(:a, "a", "ch-1", deps)
      [{:reply, 4, desc}] = Conn.send_frame(conn, 1, Fixtures.select_protocol())

      assert desc.ice_ufrag == "sfuUfrag" and desc.fingerprint == "sha-256 11:22:33"
      assert desc.candidates == [%{ip: "203.0.113.7", port: 50_000, protocol: "udp"}]
      assert desc.audio_codec == "opus" and desc.video_codec == nil and desc.e2ee == nil

      assert [{:create_session, payload}] = FakeSfu.calls()
      assert payload.session_id == "sess-a" and payload.room_id == "ch-1"
      assert payload.audio_ssrc == Conn.session(conn).ssrcs.audio
      assert payload.remote_fingerprint == "sha-256 AB:CD:EF"
      assert payload.video_layers == []
    end

    test "video publishers get their layers and the preferred codec", %{deps: deps} do
      {conn, _} = join(:a, "a", "ch-1", deps, video: true)
      [{:reply, 4, desc}] = Conn.send_frame(conn, 1, Fixtures.select_protocol(video: true))

      assert desc.video_codec == "VP9"
      assert [{:create_session, payload}] = FakeSfu.calls()
      assert length(payload.video_layers) == 3
      assert Enum.any?(payload.codecs, &(&1.name == "VP9" and &1.rtx_payload_type == 99))
    end

    test "ssrcs from the client's offer replace the plan", %{deps: deps} do
      {conn, _} = join(:a, "a", "ch-1", deps, video: true)
      ssrcs = %{"audio" => 111_111, "video" => 222_222, "rtx" => 333_333}
      offer = put_in(Fixtures.select_protocol(video: true), ["data", "ssrcs"], ssrcs)
      [{:reply, 4, _}] = Conn.send_frame(conn, 1, offer)

      assert [{:create_session, payload}] = FakeSfu.calls()
      assert payload.audio_ssrc == 111_111
      assert [%{ssrc: 222_222, rtx_ssrc: 333_333, quality: 100}] = payload.video_layers
      assert Conn.session(conn).ssrcs.audio == 111_111
    end

    test "the client's screen share ssrcs become a separate sfu stream", %{deps: deps} do
      {conn, _} = join(:a, "a", "ch-1", deps, video: true)

      ssrcs = %{
        "audio" => 111_111,
        "video" => 222_222,
        "rtx" => 333_333,
        "screen" => 444_444,
        "screen_rtx" => 555_555
      }

      offer = put_in(Fixtures.select_protocol(video: true), ["data", "ssrcs"], ssrcs)
      [{:reply, 4, _}] = Conn.send_frame(conn, 1, offer)

      assert [{:create_session, payload}] = FakeSfu.calls()
      assert [%{ssrc: 222_222}] = payload.video_layers
      assert payload.screen == %{ssrc: 444_444, rtx_ssrc: 555_555}
    end

    test "announces the separate soundboard SSRC for audio-only and late participants", %{
      deps: deps
    } do
      {a, _} = join(:a, "a", "ch-1", deps)
      {_b, _} = join(:b, "b", "ch-1", deps)

      offer =
        put_in(Fixtures.select_protocol(), ["data", "ssrcs"], %{
          "audio" => 111,
          "soundboard" => 222
        })

      [{:reply, 4, _}] = Conn.send_frame(a, 1, offer)
      assert [{:create_session, %{audio_ssrc: 111, soundboard_ssrc: 222}}] = FakeSfu.calls()
      drain_messages()
      assert [] = Conn.send_frame(a, 12, %{"streams" => []})
      assert_receive {:pushed, :b, 12, %{audio_ssrc: 111, soundboard_ssrc: 222}}
      {_c, replies} = join(:c, "c", "ch-1", deps)
      assert Enum.any?(replies, &match?({:reply, 12, %{soundboard_ssrc: 222}}, &1))
    end

    test "invalid client ssrcs fall back to the plan", %{deps: deps} do
      {conn, _} = join(:a, "a", "ch-1", deps)
      planned = Conn.session(conn).ssrcs.audio
      offer = put_in(Fixtures.select_protocol(), ["data", "ssrcs"], %{"audio" => 0})
      [{:reply, 4, _}] = Conn.send_frame(conn, 1, offer)
      assert [{:create_session, %{audio_ssrc: ^planned}}] = FakeSfu.calls()
    end

    test "a bare colon-hex fingerprint gets a hash prefix", %{deps: deps} do
      {conn, _} = join(:a, "a", "ch-1", deps)
      offer = put_in(Fixtures.select_protocol(), ["data", "fingerprint"], "AB:CD")
      [{:reply, 4, _}] = Conn.send_frame(conn, 1, offer)
      assert [{:create_session, %{remote_fingerprint: "sha-256 AB:CD"}}] = FakeSfu.calls()
    end

    test "e2ee channels announce DAVE in the description and the external sender on join", %{
      deps: deps
    } do
      {conn, [{:reply, 2, _}, {:reply, 25, sender}]} =
        join(:a, "a", "ch-e", deps, claims: %{"e2ee" => true})

      assert sender["protocol_version"] == 2
      assert sender["founder"] == true

      [{:reply, 4, desc}] = Conn.send_frame(conn, 1, Fixtures.select_protocol())
      assert desc.e2ee == %{protocol_version: 2}
    end

    test "invalid offers and sfu failures close the connection", %{deps: deps} do
      {conn, _} = join(:a, "a", "ch-1", deps)
      assert [{:close, :invalid_payload}] = Conn.send_frame(conn, 1, %{"protocol" => "webrtc"})

      assert [{:close, :invalid_payload}] =
               Conn.send_frame(conn, 1, %{Fixtures.select_protocol() | "codecs" => []})

      FakeSfu.fail_with(:sfu_unavailable)
      assert [{:close, :sfu_unavailable}] = Conn.send_frame(conn, 1, Fixtures.select_protocol())

      FakeSfu.fail_with({:sfu, "invalid_session", "bad"})
      assert [{:close, :invalid_payload}] = Conn.send_frame(conn, 1, Fixtures.select_protocol())
    end

    test "negotiating twice is refused", %{deps: deps} do
      {conn, _} = join(:a, "a", "ch-1", deps)
      [{:reply, 4, _}] = Conn.send_frame(conn, 1, Fixtures.select_protocol())

      assert [{:close, :already_authenticated}] =
               Conn.send_frame(conn, 1, Fixtures.select_protocol())
    end
  end

  describe "SPEAKING" do
    test "is relayed with the server ssrc and the user id", %{deps: deps} do
      {a, _} = join(:a, "a", "ch-1", deps)
      {_b, _} = join(:b, "b", "ch-1", deps)
      drain_messages()

      assert [] = Conn.send_frame(a, 5, %{"speaking" => 5, "ssrc" => 123})
      ssrc = Conn.session(a).ssrcs.audio
      assert_receive {:pushed, :b, 5, %{speaking: 5, ssrc: ^ssrc, user_id: "a"}}
      refute_receive {:pushed, :a, 5, _}, 50
    end

    test "invalid bitmasks close the connection", %{deps: deps} do
      {a, _} = join(:a, "a", "ch-1", deps)
      assert [{:close, :invalid_payload}] = Conn.send_frame(a, 5, %{"speaking" => 9})
      assert [{:close, :invalid_payload}] = Conn.send_frame(a, 5, %{"speaking" => "x"})
    end
  end

  describe "VIDEO" do
    test "is rebuilt from the server plan and relayed, and replayed to late joiners", %{
      deps: deps
    } do
      {a, _} = join(:a, "a", "ch-1", deps, video: true)
      {_b, _} = join(:b, "b", "ch-1", deps)
      drain_messages()

      streams = [
        %{"rid" => "q", "active" => true},
        %{"rid" => "h", "active" => false},
        %{"rid" => "f", "active" => true, "ssrc" => 1}
      ]

      assert [] = Conn.send_frame(a, 12, %{"streams" => streams, "video_ssrc" => 666})

      plan = Conn.session(a).ssrcs
      video_ssrc = hd(plan.layers).ssrc
      assert_receive {:pushed, :b, 12, %{user_id: "a", video_ssrc: ^video_ssrc, streams: relayed}}
      assert Enum.map(relayed, & &1.active) == [true, false, true]

      {_c, [_ready, _connect, {:reply, 12, %{user_id: "a", video_ssrc: ^video_ssrc}}]} =
        join(:c, "c", "ch-1", deps)
    end

    test "camera and screen share are relayed as separate streams with their own active flag", %{
      deps: deps
    } do
      {a, _} = join(:a, "a", "ch-1", deps, video: true)
      {_b, _} = join(:b, "b", "ch-1", deps)

      ssrcs = %{"audio" => 11, "video" => 22, "rtx" => 33, "screen" => 44, "screen_rtx" => 55}
      offer = put_in(Fixtures.select_protocol(video: true), ["data", "ssrcs"], ssrcs)
      [{:reply, 4, _}] = Conn.send_frame(a, 1, offer)
      drain_messages()

      streams = [
        %{"type" => "video", "rid" => "100", "active" => false},
        %{"type" => "screen", "rid" => "100", "active" => true}
      ]

      assert [] = Conn.send_frame(a, 12, %{"streams" => streams})
      assert_receive {:pushed, :b, 12, %{video_ssrc: 0, streams: relayed}}

      assert [
               %{type: "video", ssrc: 22, active: false},
               %{type: "screen", ssrc: 44, rtx_ssrc: 55, active: true}
             ] = relayed

      both = [%{"type" => "video", "rid" => "100", "active" => true} | tl(streams)]
      assert [] = Conn.send_frame(a, 12, %{"streams" => both})

      assert_receive {:pushed, :b, 12,
                      %{video_ssrc: 22, streams: [%{active: true}, %{active: true}]}}
    end

    test "all layers off sends zero ssrcs; non-video sessions ignore VIDEO", %{deps: deps} do
      {a, _} = join(:a, "a", "ch-1", deps, video: true)
      {_b, _} = join(:b, "b", "ch-1", deps)
      drain_messages()

      Conn.send_frame(a, 12, %{"streams" => []})
      assert_receive {:pushed, :b, 12, %{video_ssrc: 0, rtx_ssrc: 0}}

      {c, _} = join(:c, "c", "ch-1", deps)
      drain_messages()
      assert [] = Conn.send_frame(c, 12, %{"streams" => [%{"rid" => "q", "active" => true}]})
      refute_receive {:pushed, _, 12, _}, 50
      assert [{:close, :invalid_payload}] = Conn.send_frame(a, 12, "bad")
    end
  end

  describe "MEDIA_SINK_WANTS" do
    test "forwards valid wants to the sfu once negotiated", %{deps: deps} do
      {a, _} = join(:a, "a", "ch-1", deps)
      assert [] = Conn.send_frame(a, 15, %{"1" => 50})
      assert FakeSfu.calls() == []

      [{:reply, 4, _}] = Conn.send_frame(a, 1, Fixtures.select_protocol())

      assert [] =
               Conn.send_frame(a, 15, %{
                 "111" => 0,
                 "222" => 100,
                 "bad" => 5,
                 "333" => 101,
                 "0" => 1,
                 "444" => "x"
               })

      assert {:set_sink_wants, "sess-a", %{"111" => 0, "222" => 100}} in FakeSfu.calls()
      assert [{:close, :invalid_payload}] = Conn.send_frame(a, 15, "nope")
    end
  end

  describe "DAVE relay" do
    test "relays MLS messages in e2ee channels only", %{deps: deps} do
      {a, _} = join(:a, "a", "ch-e", deps, claims: %{"e2ee" => true})
      {_b, _} = join(:b, "b", "ch-e", deps, claims: %{"e2ee" => true})
      {_c, _} = join(:c, "c", "ch-e", deps, claims: %{"e2ee" => true})
      drain_messages()

      Conn.send_frame(a, 27, %{"proposals" => "p"})
      assert_receive {:pushed, :b, 27, %{"proposals" => "p", "user_id" => "a"}}
      assert_receive {:pushed, :c, 27, _}

      Conn.send_frame(a, 28, %{
        "commit" => "c",
        "welcome" => "w",
        "transition_id" => 1,
        "recipient_ids" => ["c"]
      })

      assert_receive {:pushed, :b, 29, %{"commit" => "c"}}
      assert_receive {:pushed, :c, 29, _}
      assert_receive {:pushed, :c, 30, %{"welcome" => "w"}}
      refute_receive {:pushed, :b, 30, _}, 50

      {x, _} = join(:x, "x", "ch-plain", deps)
      {_y, _} = join(:y, "y", "ch-plain", deps)
      drain_messages()
      Conn.send_frame(x, 27, %{"proposals" => "p"})
      refute_receive {:pushed, :y, 27, _}, 50
    end
  end
end
