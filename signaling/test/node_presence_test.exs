defmodule DiscordyVoice.NodePresenceTest do
  use ExUnit.Case, async: false

  alias DiscordyVoice.{Contracts, FakeNodeRegistry, Fixtures}
  alias DiscordyVoice.Infrastructure.{DrainControl, NodePresence}

  @key Contracts.node_key("nl-home", "node-1")
  @drain_key Contracts.drain_key("node-1")

  setup do
    start_supervised!(FakeNodeRegistry)
    on_exit(fn -> DrainControl.undrain() end)

    start_supervised!(
      {NodePresence, settings: Fixtures.settings(), registry: FakeNodeRegistry, interval_ms: 20}
    )

    :ok
  end

  defp eventually(fun, attempts \\ 100) do
    cond do
      fun.() -> :ok
      attempts == 0 -> flunk("condition not reached")
      true -> Process.sleep(10) && eventually(fun, attempts - 1)
    end
  end

  test "registers the P8 value under the node key with a lease" do
    eventually(fn -> FakeNodeRegistry.get(@key) != nil end)
    {json, lease} = FakeNodeRegistry.get(@key)

    assert lease > 0
    assert NodePresence.registered?()

    assert %{
             "node_id" => "node-1",
             "region" => "nl-home",
             "ws_url" => "wss://rtc.example.test",
             "public_ip" => "203.0.113.7",
             "udp_port" => 50_000,
             "users" => users,
             "cpu_pct" => cpu,
             "egress_bps" => _,
             "draining" => false,
             "version" => "0.1.0"
           } = Jason.decode!(json)

    assert is_integer(users) and is_number(cpu)
  end

  test "keeps the lease alive on every tick" do
    eventually(fn -> Enum.count(FakeNodeRegistry.ops(), &(&1 == :keep_alive)) >= 2 end)
    assert Enum.count(FakeNodeRegistry.ops(), &(&1 == :grant)) == 1
  end

  test "re-grants a lost lease and re-registers" do
    eventually(fn -> FakeNodeRegistry.get(@key) != nil end)
    FakeNodeRegistry.expire_leases()

    eventually(fn -> FakeNodeRegistry.get(@key) != nil end)
    assert Enum.count(FakeNodeRegistry.ops(), &(&1 == :grant)) == 2
  end

  test "the gateway's drain marker drains the node and removing it undrains" do
    eventually(fn -> FakeNodeRegistry.get(@key) != nil end)
    refute DrainControl.draining?()

    FakeNodeRegistry.put_marker(@drain_key)
    eventually(&DrainControl.draining?/0)

    eventually(fn ->
      {json, _} = FakeNodeRegistry.get(@key)
      Jason.decode!(json)["draining"]
    end)

    FakeNodeRegistry.remove_marker(@drain_key)
    eventually(fn -> not DrainControl.draining?() end)
  end

  test "a drain triggered another way is not undrained by the absent marker" do
    eventually(fn -> FakeNodeRegistry.get(@key) != nil end)
    DrainControl.drain()
    Process.sleep(100)
    assert DrainControl.draining?()
  end

  test "deletes its key on graceful shutdown" do
    eventually(fn -> FakeNodeRegistry.get(@key) != nil end)
    stop_supervised!(NodePresence)
    assert FakeNodeRegistry.get(@key) == nil
  end
end
