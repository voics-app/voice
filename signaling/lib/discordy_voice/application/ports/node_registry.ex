defmodule DiscordyVoice.Application.Ports.NodeRegistry do
  @moduledoc "Service discovery store (etcd): leased registration of this node and the drain marker."

  @type lease :: integer()

  @callback connect(endpoints :: [String.t()]) :: :ok | {:error, term()}
  @callback grant(ttl_s :: pos_integer()) :: {:ok, lease()} | {:error, term()}
  @callback keep_alive(lease()) :: :ok | {:error, term()}
  @callback put(key :: String.t(), value :: binary(), lease()) :: :ok | {:error, term()}
  @callback delete(key :: String.t()) :: :ok | {:error, term()}
  @callback claim(key :: String.t(), value :: binary(), lease()) :: :ok | {:error, term()}
  @callback refresh(key :: String.t(), value :: binary(), lease()) :: :ok | {:error, term()}
  @callback release(key :: String.t(), lease()) :: :ok | {:error, term()}
  @callback exists?(key :: String.t()) :: {:ok, boolean()} | {:error, term()}
end
