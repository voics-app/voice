defmodule DiscordyVoice.Domain.Protocol.Frame do
  @moduledoc "A voice WebSocket frame: `{op, d}` JSON."

  @enforce_keys [:op, :d]
  defstruct [:op, :d]

  @type t :: %__MODULE__{op: non_neg_integer(), d: term()}

  @doc "Parses a text frame."
  @spec decode(binary()) :: {:ok, t()} | {:error, :invalid_frame}
  def decode(text) when is_binary(text) do
    case Jason.decode(text) do
      {:ok, %{"op" => op} = map} when is_integer(op) and op >= 0 ->
        {:ok, %__MODULE__{op: op, d: Map.get(map, "d")}}

      _ ->
        {:error, :invalid_frame}
    end
  end

  @doc "Serializes a frame."
  @spec encode(non_neg_integer(), term()) :: binary()
  def encode(op, d), do: Jason.encode!(%{"op" => op, "d" => d})
end
