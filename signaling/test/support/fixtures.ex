defmodule DiscordyVoice.Fixtures do
  @moduledoc false

  alias DiscordyVoice.Application.{Deps, Settings}
  alias DiscordyVoice.Infrastructure.{DrainControl, Hs256TokenVerifier, Rooms, SsrcCounter}

  @secret "test-voice-token-secret-0123456789"

  def secret, do: @secret

  def settings(overrides \\ []) do
    base = %Settings{
      env: :local,
      region: "nl-home",
      node_id: "node-1",
      public_ws_url: "wss://rtc.example.test",
      public_ip: "203.0.113.7",
      udp_port: 50_000,
      etcd_endpoints: ["127.0.0.1:2379"],
      token_secret: @secret,
      internal_token: "internal-secret",
      sfu_control: {"127.0.0.1", 4101},
      http_port: 0,
      version: "0.1.0"
    }

    struct!(base, overrides)
  end

  def deps(overrides \\ []) do
    base = %Deps{
      settings: settings(),
      token_verifier: Hs256TokenVerifier,
      sfu: DiscordyVoice.FakeSfu,
      rooms: Rooms,
      ssrc: SsrcCounter,
      drain: DrainControl,
      registration: DiscordyVoice.RegisteredNode
    }

    deps = struct!(base, overrides)
    Application.put_env(:discordy_voice, :deps, deps)
    deps
  end

  def token(user, session, channel, overrides \\ %{}) do
    claims =
      Map.merge(
        %{
          "sub" => user,
          "sid" => session,
          "srv" => "guild-1",
          "chn" => channel,
          "node" => "node-1",
          "e2ee" => false,
          "iat" => System.os_time(:second),
          "exp" => System.os_time(:second) + 60
        },
        overrides
      )

    Hs256TokenVerifier.sign(claims, @secret)
  end

  def identify(user, session, channel, opts \\ []) do
    %{
      "server_id" => "guild-1",
      "user_id" => user,
      "session_id" => session,
      "token" => token(user, session, channel, Keyword.get(opts, :claims, %{})),
      "video" => Keyword.get(opts, :video, false)
    }
  end

  def select_protocol(opts \\ []) do
    video = Keyword.get(opts, :video, false)

    codecs =
      [%{"name" => "opus", "type" => "audio", "priority" => 1000, "payload_type" => 111}] ++
        if video,
          do: [
            %{
              "name" => "VP9",
              "type" => "video",
              "priority" => 2000,
              "payload_type" => 98,
              "rtx_payload_type" => 99
            },
            %{
              "name" => "H264",
              "type" => "video",
              "priority" => 1000,
              "payload_type" => 100,
              "rtx_payload_type" => 101
            }
          ],
          else: []

    %{
      "protocol" => "webrtc",
      "data" => %{
        "ice_ufrag" => "clientUfrag",
        "ice_pwd" => "clientIcePasswordClientIcePwd",
        "fingerprint" => "sha-256 AB:CD:EF",
        "setup" => "actpass"
      },
      "codecs" => codecs
    }
  end
end

defmodule DiscordyVoice.RegisteredNode do
  @moduledoc false
  @behaviour DiscordyVoice.Application.Ports.NodeRegistration
  @impl true
  def registered?, do: true
  @impl true
  def lease_id, do: {:ok, "1"}
end

defmodule DiscordyVoice.UnregisteredNode do
  @moduledoc false
  @behaviour DiscordyVoice.Application.Ports.NodeRegistration
  @impl true
  def registered?, do: false
  @impl true
  def lease_id, do: {:error, :unregistered}
end

defmodule DiscordyVoice.FakeSfu do
  @moduledoc false
  @behaviour DiscordyVoice.Application.Ports.SfuControl

  use Agent

  def start_link(opts \\ []) do
    Agent.start_link(fn -> %{calls: [], fail: Keyword.get(opts, :fail)} end, name: __MODULE__)
  end

  def calls, do: Agent.get(__MODULE__, &Enum.reverse(&1.calls))
  def fail_with(reason), do: Agent.update(__MODULE__, &%{&1 | fail: reason})

  @impl true
  def create_session(payload) do
    Agent.get_and_update(__MODULE__, fn state ->
      state = %{state | calls: [{:create_session, payload} | state.calls]}

      reply =
        case state.fail do
          nil ->
            {:ok,
             %{
               local_ufrag: "sfuUfrag",
               local_pwd: "sfuPasswordSfuPasswordSfu",
               fingerprint: "sha-256 11:22:33"
             }}

          reason ->
            {:error, reason}
        end

      {reply, state}
    end)
  end

  @impl true
  def close_session(session_id), do: record({:close_session, session_id})

  @impl true
  def set_publish_permissions(session_id, update),
    do: record({:set_publish_permissions, session_id, update})

  @impl true
  def set_publish_audio(session_id, allowed),
    do: record({:set_publish_audio, session_id, allowed})

  @impl true
  def set_publish_video(session_id, allowed),
    do: record({:set_publish_video, session_id, allowed})

  @impl true
  def set_sink_wants(session_id, wants) do
    record({:set_sink_wants, session_id, wants})
    :ok
  end

  defp record(call) do
    Agent.update(__MODULE__, &%{&1 | calls: [call | &1.calls]})
  end
end

defmodule DiscordyVoice.FakeNodeRegistry do
  @moduledoc false
  @behaviour DiscordyVoice.Application.Ports.NodeRegistry

  use Agent

  def start_link(_opts \\ []) do
    Agent.start_link(fn -> %{keys: %{}, leases: MapSet.new(), next: 1, ops: []} end,
      name: __MODULE__
    )
  end

  def get(key), do: Agent.get(__MODULE__, &get_in(&1, [:keys, key]))
  def ops, do: Agent.get(__MODULE__, &Enum.reverse(&1.ops))
  def expire_leases, do: Agent.update(__MODULE__, &%{&1 | leases: MapSet.new(), keys: %{}})
  def put_marker(key), do: Agent.update(__MODULE__, &put_in(&1, [:keys, key], {"", 0}))
  def remove_marker(key), do: Agent.update(__MODULE__, &%{&1 | keys: Map.delete(&1.keys, key)})

  @impl true
  def connect(_endpoints), do: :ok

  @impl true
  def grant(_ttl) do
    Agent.get_and_update(__MODULE__, fn s ->
      {{:ok, s.next},
       %{s | next: s.next + 1, leases: MapSet.put(s.leases, s.next), ops: [:grant | s.ops]}}
    end)
  end

  @impl true
  def keep_alive(lease) do
    Agent.get_and_update(__MODULE__, fn s ->
      result = if MapSet.member?(s.leases, lease), do: :ok, else: {:error, :lease_expired}
      {result, %{s | ops: [:keep_alive | s.ops]}}
    end)
  end

  @impl true
  def put(key, value, lease) do
    Agent.update(
      __MODULE__,
      &%{&1 | keys: Map.put(&1.keys, key, {value, lease}), ops: [:put | &1.ops]}
    )
  end

  @impl true
  def delete(key),
    do: Agent.update(__MODULE__, &%{&1 | keys: Map.delete(&1.keys, key), ops: [:delete | &1.ops]})

  @impl true
  def claim(key, value, lease) do
    Agent.get_and_update(__MODULE__, fn state ->
      if Map.has_key?(state.keys, key),
        do: {{:error, :registration_conflict}, state},
        else: {:ok, %{state | keys: Map.put(state.keys, key, {value, lease})}}
    end)
  end

  @impl true
  def refresh(key, value, lease), do: owned_update(key, lease, &Map.put(&1, key, {value, lease}))

  @impl true
  def release(key, lease), do: owned_update(key, lease, &Map.delete(&1, key))

  defp owned_update(key, lease, update) do
    Agent.get_and_update(__MODULE__, fn state ->
      case state.keys[key] do
        {_value, ^lease} -> {:ok, %{state | keys: update.(state.keys)}}
        _ -> {{:error, :registration_conflict}, state}
      end
    end)
  end

  @impl true
  def exists?(key), do: {:ok, Agent.get(__MODULE__, &Map.has_key?(&1.keys, key))}
end
