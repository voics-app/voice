defmodule DiscordyVoice.Infrastructure.EtcdNodeRegistry do
  @moduledoc "etcd v3 adapter of the node registry, on the `eetcd` client."

  @behaviour DiscordyVoice.Application.Ports.NodeRegistry

  @conn :discordy_voice_etcd

  @impl true
  def connect(endpoints) do
    case :eetcd.open(@conn, Enum.map(endpoints, &String.to_charlist/1)) do
      {:ok, _pid} -> :ok
      {:error, {:already_started, _pid}} -> :ok
      {:error, reason} -> if already_started?(reason), do: :ok, else: {:error, reason}
    end
  end

  # A restarted presence process reopens the connection the client already holds: eetcd reports
  # that per endpoint as `[{{host, port}, :already_started}]`.
  defp already_started?([_ | _] = failures),
    do: Enum.all?(failures, &match?({_endpoint, :already_started}, &1))

  defp already_started?(_reason), do: false

  @impl true
  def grant(ttl_s) do
    case :eetcd_lease.grant(@conn, ttl_s) do
      {:ok, %{ID: lease}} -> {:ok, lease}
      {:error, reason} -> {:error, reason}
    end
  end

  @impl true
  def keep_alive(lease) do
    case :eetcd_lease.keep_alive_once(@conn, lease) do
      {:ok, %{TTL: ttl}} when ttl > 0 -> :ok
      {:ok, _expired} -> {:error, :lease_expired}
      {:error, reason} -> {:error, reason}
    end
  end

  @impl true
  def put(key, value, lease) do
    ctx = :eetcd_kv.new(@conn)
    ctx = :eetcd_kv.with_key(ctx, key)
    ctx = :eetcd_kv.with_value(ctx, value)
    ctx = :eetcd_kv.with_lease(ctx, lease)

    case :eetcd_kv.put(ctx) do
      {:ok, _response} -> :ok
      {:error, reason} -> {:error, reason}
    end
  end

  @impl true
  def delete(key) do
    case :eetcd_kv.delete(@conn, key) do
      {:ok, _response} -> :ok
      {:error, reason} -> {:error, reason}
    end
  end

  @impl true
  def exists?(key) do
    case :eetcd_kv.get(@conn, key) do
      {:ok, %{count: count}} -> {:ok, count > 0}
      {:error, reason} -> {:error, reason}
    end
  end
end
