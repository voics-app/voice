defmodule DiscordyVoice.NodeOwnershipTest do
  use ExUnit.Case, async: false

  alias DiscordyVoice.{Contracts, FakeNodeRegistry, Fixtures}
  alias DiscordyVoice.Infrastructure.NodePresence

  @key Contracts.node_key("nl-home", "node-1")

  setup do
    start_supervised!(FakeNodeRegistry)
    :ok
  end

  test "a second replica cannot replace a live node identity or delete its registration" do
    owner = presence(:owner)
    assert NodePresence.registered?(owner)
    original = FakeNodeRegistry.get(@key)

    contender = presence(:contender)
    refute NodePresence.registered?(contender)
    assert FakeNodeRegistry.get(@key) == original

    stop_supervised!(:contender)
    assert FakeNodeRegistry.get(@key) == original
    assert NodePresence.registered?(owner)
  end

  test "a stale process cannot delete the replacement after losing its lease" do
    owner = presence(:owner)
    assert NodePresence.registered?(owner)
    FakeNodeRegistry.expire_leases()
    replacement = presence(:replacement)
    assert NodePresence.registered?(replacement)
    registration = FakeNodeRegistry.get(@key)

    stop_supervised!(:owner)
    assert FakeNodeRegistry.get(@key) == registration
  end

  test "local lease expiry fences admission even before the next refresh" do
    owner = presence(:owner)
    assert NodePresence.registered?(owner)
    :sys.replace_state(owner, &%{&1 | valid_until: System.monotonic_time(:millisecond) - 1})
    refute NodePresence.registered?(owner)
  end

  defp presence(name) do
    start_supervised!(%{
      id: name,
      start:
        {NodePresence, :start_link,
         [
           [
             settings: Fixtures.settings(),
             registry: FakeNodeRegistry,
             name: name,
             interval_ms: 60_000
           ]
         ]}
    })
  end
end
