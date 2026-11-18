defmodule DiscordyVoice.DomainTest do
  use ExUnit.Case, async: true

  alias DiscordyVoice.Domain.{Codecs, Dave, NodeStatus, Room, SsrcPlan}
  alias DiscordyVoice.Domain.Protocol.Frame
  alias DiscordyVoice.Domain.Room.Member

  describe "Frame" do
    test "round-trips op and payload" do
      assert {:ok, %Frame{op: 3, d: %{"t" => 7}}} = Frame.decode(Frame.encode(3, %{t: 7}))
    end

    test "rejects non-frames" do
      for bad <- ["nope", ~s({"d":1}), ~s({"op":"x"}), ~s({"op":-1}), ~s([1])] do
        assert {:error, :invalid_frame} = Frame.decode(bad)
      end
    end
  end

  describe "SsrcPlan" do
    test "lays out distinct ssrcs and consecutive plans do not overlap" do
      a = SsrcPlan.build(1_000)
      b = SsrcPlan.build(1_000 + SsrcPlan.size())
      all = fn p -> [p.audio | Enum.flat_map(p.layers, &[&1.ssrc, &1.rtx_ssrc])] end

      assert length(all.(a)) == SsrcPlan.size()
      assert length(Enum.uniq(all.(a) ++ all.(b))) == 2 * SsrcPlan.size()
    end

    test "wraps around the 32-bit space and never yields 0" do
      plan = SsrcPlan.build(0xFFFFFFFE)
      ssrcs = [plan.audio | Enum.flat_map(plan.layers, &[&1.ssrc, &1.rtx_ssrc])]
      assert Enum.all?(ssrcs, &(&1 in 1..0xFFFFFFFF))
      assert length(Enum.uniq(ssrcs)) == length(ssrcs)
    end

    test "streams are empty without video" do
      plan = SsrcPlan.build(5)
      assert SsrcPlan.streams(plan, false) == []
      assert [%{type: "video", rid: "q"} | _] = SsrcPlan.streams(plan, true)
      assert SsrcPlan.primary_video(plan) == hd(plan.layers).ssrc
    end
  end

  describe "Codecs" do
    @opus %{"name" => "opus", "type" => "audio", "priority" => 1, "payload_type" => 111}

    test "requires opus" do
      video = %{"name" => "VP9", "type" => "video", "priority" => 1, "payload_type" => 98}
      assert {:error, :invalid_codecs} = Codecs.negotiate([video], true)
      assert {:error, :invalid_codecs} = Codecs.negotiate("x", true)
    end

    test "picks the highest priority video codec and ignores unknown ones" do
      h264 = %{"name" => "h264", "type" => "video", "priority" => 5, "payload_type" => 100}
      av1 = %{"name" => "AV1", "type" => "video", "priority" => 9, "payload_type" => 101}
      theora = %{"name" => "theora", "type" => "video", "priority" => 99, "payload_type" => 102}

      assert {:ok, %{video_codec: "AV1", audio_codec: "opus", codecs: codecs}} =
               Codecs.negotiate([@opus, h264, av1, theora], true)

      assert length(codecs) == 3
    end

    test "drops video codecs when the user does not publish video" do
      av1 = %{"name" => "AV1", "type" => "video", "priority" => 9, "payload_type" => 101}

      assert {:ok, %{video_codec: nil, codecs: [%{name: "opus"}]}} =
               Codecs.negotiate([@opus, av1], false)
    end

    test "rejects duplicate payload types and malformed entries" do
      dup = %{"name" => "VP8", "type" => "video", "priority" => 1, "payload_type" => 111}
      assert {:error, :invalid_codecs} = Codecs.negotiate([@opus, dup], true)
      assert {:error, :invalid_codecs} = Codecs.negotiate([@opus, %{"name" => "x"}], true)

      assert {:error, :invalid_codecs} =
               Codecs.negotiate([%{@opus | "payload_type" => 300}], true)
    end
  end

  describe "Room" do
    defp member(user, pid \\ self()),
      do: %Member{user_id: user, session_id: "s-" <> user, pid: pid, audio_ssrc: 1}

    test "join evicts the previous session of the same user" do
      {room, nil} = Room.join(Room.new(), member("a"))
      other = spawn(fn -> :ok end)
      {room, evicted} = Room.join(room, member("a", other))

      assert evicted.pid == self()
      assert Room.member(room, "a").pid == other
    end

    test "stale leave is ignored, matching leave removes" do
      {room, _} = Room.join(Room.new(), member("a"))
      stranger = spawn(fn -> :ok end)

      assert {^room, nil} = Room.leave(room, "a", stranger)
      {empty, left} = Room.leave(room, "a", self())
      assert left.user_id == "a"
      assert Room.empty?(empty)
    end

    test "others, select and video" do
      {room, _} = Room.join(Room.new(), member("a"))
      {room, _} = Room.join(room, member("b"))
      {room, _} = Room.join(room, member("c"))

      assert room |> Room.others("a") |> Enum.map(& &1.user_id) |> Enum.sort() == ["b", "c"]
      assert room |> Room.select(["a", "c", "zzz"], "a") |> Enum.map(& &1.user_id) == ["c"]

      assert Room.put_video(room, "b", %{x: 1}) |> Room.member("b") |> Map.fetch!(:video) == %{
               x: 1
             }

      assert Room.put_video(room, "nobody", %{}) == room
    end

    test "leave_pid removes whichever member owns the pid" do
      {room, _} = Room.join(Room.new(), member("a"))
      assert {_, %Member{user_id: "a"}} = Room.leave_pid(room, self())
      assert {^room, nil} = Room.leave_pid(room, spawn(fn -> :ok end))
    end
  end

  describe "Dave" do
    test "proposals and key packages are broadcast with the sender" do
      assert {:broadcast, 27, %{"user_id" => "a", "p" => 1}} = Dave.route(27, %{"p" => 1}, "a")
      assert {:broadcast, 26, _} = Dave.route(26, %{"k" => 1}, "a")
      assert {:broadcast, 22, _} = Dave.route(22, %{"transition_id" => 3}, "a")
    end

    test "commit/welcome fans out to announce_commit and welcome" do
      d = %{"transition_id" => 4, "commit" => "c", "welcome" => "w", "recipient_ids" => ["b", 7]}

      assert {:fanout, [commit, welcome]} = Dave.route(28, d, "a")
      assert {:broadcast, 29, %{"commit" => "c", "transition_id" => 4, "user_id" => "a"}} = commit
      assert {:to, ["b"], 30, %{"welcome" => "w"}} = welcome
    end

    test "welcome without recipients goes to everybody; empty commit_welcome is dropped" do
      assert {:fanout, [{:broadcast, 30, _}]} = Dave.route(28, %{"welcome" => "w"}, "a")
      assert :drop = Dave.route(28, %{}, "a")
    end

    test "server-originated and malformed opcodes are not accepted from clients" do
      for op <- [25, 29, 30], do: assert(:drop = Dave.route(op, %{}, "a"))
      assert :drop = Dave.route(27, "not a map", "a")
      assert :drop = Dave.route(5, %{}, "a")
      assert Dave.dave_op?(21) and Dave.dave_op?(31) and not Dave.dave_op?(20)
    end

    test "external sender announcement carries version, credential and key" do
      assert %{
               "protocol_version" => 2,
               "external_sender" => %{"credential" => c, "signature_key" => k}
             } =
               Dave.external_sender("room", <<1, 2, 3>>)

      assert Base.url_decode64!(c, padding: false) == "discordy-voice:room"
      assert Base.url_decode64!(k, padding: false) == <<1, 2, 3>>
      assert %{"founder" => false} = Dave.external_sender("room", <<1>>)
      assert %{"founder" => true} = Dave.external_sender("room", <<1>>, true)
    end
  end

  test "NodeStatus encodes the P8 value" do
    json =
      %NodeStatus{
        node_id: "n",
        region: "r",
        ws_url: "wss://x",
        public_ip: "1.2.3.4",
        udp_port: 50_000,
        version: "0.1.0",
        users: 3,
        cpu_pct: 12.34,
        egress_bps: 9,
        draining: true
      }
      |> NodeStatus.to_json()
      |> Jason.decode!()

    assert json == %{
             "node_id" => "n",
             "region" => "r",
             "ws_url" => "wss://x",
             "public_ip" => "1.2.3.4",
             "udp_port" => 50_000,
             "users" => 3,
             "cpu_pct" => 12.3,
             "egress_bps" => 9,
             "draining" => true,
             "probe_required" => false,
             "version" => "0.1.0"
           }
  end
end
