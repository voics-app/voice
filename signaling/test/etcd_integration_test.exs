defmodule DiscordyVoice.EtcdIntegrationTest do
  @moduledoc """
  The etcd adapter and `NodePresence` against a real etcd in a throwaway container (random host
  port, named `voice-signaling-etcd-*`). Skipped when Docker is unavailable.
  """

  use ExUnit.Case, async: false

  alias DiscordyVoice.{Contracts, Fixtures}
  alias DiscordyVoice.Infrastructure.{DrainControl, EtcdNodeRegistry, NodePresence}

  @image "quay.io/coreos/etcd:v3.5.17"
  @docker_up (case System.find_executable("docker") do
                nil -> false
                docker -> match?({_, 0}, System.cmd(docker, ["info"], stderr_to_stdout: true))
              end)

  @endpoint System.get_env("ETCD_TEST_ENDPOINT")
  @moduletag skip: if(@docker_up or @endpoint != nil, do: false, else: "docker is unavailable")

  setup_all do
    if @endpoint do
      wait_for_etcd([@endpoint])
      {:ok, etcd_endpoints: [@endpoint]}
    else
      start_etcd()
    end
  end

  defp start_etcd do
    name = "voice-signaling-etcd-#{System.unique_integer([:positive])}"

    {_, 0} =
      System.cmd("docker", [
        "run",
        "-d",
        "--rm",
        "--name",
        name,
        "-p",
        "127.0.0.1::2379",
        @image,
        "etcd",
        "--listen-client-urls",
        "http://0.0.0.0:2379",
        "--advertise-client-urls",
        "http://0.0.0.0:2379"
      ])

    on_exit(fn -> System.cmd("docker", ["rm", "-f", name], stderr_to_stdout: true) end)
    {out, 0} = System.cmd("docker", ["port", name, "2379/tcp"])
    port = out |> String.split("\n", trim: true) |> hd() |> String.split(":") |> List.last()
    wait_for_etcd(["127.0.0.1:#{port}"])
    {:ok, etcd_endpoints: ["127.0.0.1:#{port}"]}
  end

  # The container accepts connections a moment after `docker run` returns: connecting once
  # raced it (econnrefused). Retry until a real request is answered.
  defp wait_for_etcd(endpoints, attempts \\ 120) do
    with :ok <- EtcdNodeRegistry.connect(endpoints),
         {:ok, _} <- EtcdNodeRegistry.exists?("/ping") do
      :ok
    else
      _ when attempts == 0 -> flunk("etcd did not come up")
      _ -> Process.sleep(500) && wait_for_etcd(endpoints, attempts - 1)
    end
  end

  defp eventually(fun, attempts \\ 100) do
    cond do
      fun.() -> :ok
      attempts == 0 -> flunk("condition not reached")
      true -> Process.sleep(50) && eventually(fun, attempts - 1)
    end
  end

  test "leased keys are written, refreshed, listed under the P8 prefix and expire without refresh" do
    {:ok, lease} = EtcdNodeRegistry.grant(2)
    key = Contracts.node_key("nl-home", "lease-test")
    assert :ok = EtcdNodeRegistry.put(key, ~s({"x":1}), lease)
    assert {:ok, true} = EtcdNodeRegistry.exists?(key)
    assert :ok = EtcdNodeRegistry.keep_alive(lease)

    eventually(fn -> EtcdNodeRegistry.exists?(key) == {:ok, false} end, 200)
    assert {:error, _} = EtcdNodeRegistry.keep_alive(lease)

    assert :ok = EtcdNodeRegistry.delete("/missing")
  end

  test "NodePresence registers the node, follows the drain marker and cleans up", %{
    etcd_endpoints: endpoints
  } do
    settings = Fixtures.settings(node_id: "etcd-node", etcd_endpoints: endpoints)
    key = Contracts.node_key("nl-home", "etcd-node")
    on_exit(&DrainControl.undrain/0)

    start_supervised!(
      {NodePresence, settings: settings, registry: EtcdNodeRegistry, interval_ms: 100}
    )

    eventually(fn -> EtcdNodeRegistry.exists?(key) == {:ok, true} end)
    assert NodePresence.registered?()

    {:ok, lease} = EtcdNodeRegistry.grant(60)
    :ok = EtcdNodeRegistry.put(Contracts.drain_key("etcd-node"), "1", lease)
    eventually(&DrainControl.draining?/0)

    :ok = EtcdNodeRegistry.delete(Contracts.drain_key("etcd-node"))
    eventually(fn -> not DrainControl.draining?() end)

    stop_supervised!(NodePresence)
    assert {:ok, false} = EtcdNodeRegistry.exists?(key)
  end

  test "node ownership is atomic and a stale owner cannot overwrite or remove its successor" do
    key = Contracts.node_key("nl-home", "exclusive-node")
    {:ok, first} = EtcdNodeRegistry.grant(10)
    {:ok, second} = EtcdNodeRegistry.grant(10)
    assert :ok = EtcdNodeRegistry.claim(key, "first", first)
    assert {:error, :registration_conflict} = EtcdNodeRegistry.claim(key, "second", second)
    assert {:error, :registration_conflict} = EtcdNodeRegistry.refresh(key, "second", second)
    assert {:error, :registration_conflict} = EtcdNodeRegistry.release(key, second)
    assert :ok = EtcdNodeRegistry.refresh(key, "first-updated", first)
    assert :ok = EtcdNodeRegistry.release(key, first)
    assert :ok = EtcdNodeRegistry.claim(key, "second", second)
    assert {:error, :registration_conflict} = EtcdNodeRegistry.release(key, first)
    assert {:ok, true} = EtcdNodeRegistry.exists?(key)
    assert :ok = EtcdNodeRegistry.release(key, second)
  end
end
