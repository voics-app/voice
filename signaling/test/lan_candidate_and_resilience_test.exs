defmodule DiscordyVoice.LanCandidateAndResilienceTest do
  use ExUnit.Case, async: false

  alias DiscordyVoice.Application.Settings
  alias DiscordyVoice.{FakeNodeRegistry, Fixtures}
  alias DiscordyVoice.Infrastructure.NodePresence
  alias DiscordyVoice.Interface.ErrorJSON

  @env %{
    "VOICE_REGION" => "nl-home",
    "VOICE_NODE_ID" => "n1",
    "VOICE_PUBLIC_WS_URL" => "wss://rtc.example.test",
    "VOICE_PUBLIC_IP" => "203.0.113.7",
    "ETCD_ENDPOINTS" => "http://etcd:2379",
    "VOICE_TOKEN_SECRET" => "0123456789abcdef0123",
    "INTERNAL_SERVICE_TOKEN" => "x"
  }

  test "LAN IP is optional and validated" do
    assert {:ok, %Settings{lan_ip: nil}} = Settings.from_env(@env)

    assert {:ok, %Settings{lan_ip: "192.168.1.212"} = s} =
             Settings.from_env(Map.put(@env, "VOICE_LAN_IP", "192.168.1.212"))

    assert Settings.candidate_ips(s) == ["192.168.1.212", "203.0.113.7"]
    assert {:error, [msg]} = Settings.from_env(Map.put(@env, "VOICE_LAN_IP", "nope"))
    assert msg =~ "VOICE_LAN_IP"
  end

  test "candidate list has public only, deduplicated" do
    assert Settings.candidate_ips(Fixtures.settings()) == ["203.0.113.7"]
    assert Settings.candidate_ips(Fixtures.settings(lan_ip: "203.0.113.7")) == ["203.0.113.7"]
  end

  test "presence survives stray messages" do
    start_supervised!(FakeNodeRegistry)

    pid =
      start_supervised!(
        {NodePresence, settings: Fixtures.settings(), registry: FakeNodeRegistry, interval_ms: 20}
      )

    send(pid, {make_ref(), {:ok, :late_reply}})
    send(pid, :whatever)
    Process.sleep(80)
    assert Process.alive?(pid)
    assert NodePresence.registered?()
  end

  test "error view renders JSON" do
    assert ErrorJSON.render("500.json", %{}) == %{error: "Internal Server Error"}
  end
end
