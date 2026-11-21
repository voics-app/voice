defmodule DiscordyVoice.Domain.SsrcPlan do
  @moduledoc """
  The SSRCs of one voice session: one audio SSRC, and for each simulcast layer a media SSRC and
  an RTX SSRC. A plan occupies `size/0` consecutive values from a node-unique base, so plans from
  distinct bases never overlap. A screen share is a separate stream (`screen`), only known when the
  client reports the SSRCs of its offer.
  """

  alias DiscordyVoice.Contracts

  @max_ssrc 0xFFFFFFFF

  @enforce_keys [:audio, :layers]
  defstruct [:audio, :layers, screen: nil, soundboard: nil]

  @type layer :: %{rid: String.t(), quality: 0..100, ssrc: pos_integer(), rtx_ssrc: pos_integer()}
  @type stream :: %{ssrc: pos_integer(), rtx_ssrc: non_neg_integer()}
  @type t :: %__MODULE__{
          audio: pos_integer(),
          layers: [layer()],
          screen: stream() | nil,
          soundboard: pos_integer() | nil
        }

  @doc "Number of SSRCs a plan consumes."
  @spec size() :: pos_integer()
  def size, do: 1 + 2 * length(Contracts.video_layers())

  @doc "Lays out a plan starting at `base`, wrapping around the 32-bit space and skipping 0."
  @spec build(non_neg_integer()) :: t()
  def build(base) do
    audio = ssrc_at(base, 0)

    layers =
      Contracts.video_layers()
      |> Enum.with_index()
      |> Enum.map(fn {layer, index} ->
        Map.merge(layer, %{
          ssrc: ssrc_at(base, 1 + 2 * index),
          rtx_ssrc: ssrc_at(base, 2 + 2 * index)
        })
      end)

    %__MODULE__{audio: audio, layers: layers}
  end

  @doc "READY's `streams` array; empty when the user does not publish video."
  @spec streams(t(), boolean()) :: [map()]
  def streams(_plan, false), do: []

  def streams(%__MODULE__{layers: layers}, true) do
    Enum.map(layers, &Map.put(&1, :type, "video"))
  end

  @doc "SSRC subscribers receive this publisher's video on (the first layer)."
  @spec primary_video(t()) :: pos_integer()
  def primary_video(%__MODULE__{layers: [first | _]}), do: first.ssrc

  @doc "RTX SSRC paired with `primary_video/1`."
  @spec primary_rtx(t()) :: pos_integer()
  def primary_rtx(%__MODULE__{layers: [first | _]}), do: first.rtx_ssrc

  defp ssrc_at(base, offset), do: rem(base + offset - 1, @max_ssrc) + 1
end
