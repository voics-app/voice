defmodule DiscordyVoice.Domain.Codecs do
  @moduledoc "Validation and selection of the codecs announced in SELECT_PROTOCOL."

  alias DiscordyVoice.Contracts

  @type codec :: %{
          name: String.t(),
          type: String.t(),
          priority: non_neg_integer(),
          payload_type: 0..127,
          rtx_payload_type: 0..127 | nil
        }

  @type negotiated :: %{codecs: [codec()], audio_codec: String.t(), video_codec: String.t() | nil}

  @doc """
  Parses and validates the client's codec list. Opus is mandatory; unknown video codecs are
  ignored. When `video?` is false the video codecs are dropped.
  """
  @spec negotiate(term(), boolean()) :: {:ok, negotiated()} | {:error, :invalid_codecs}
  def negotiate(raw, video?) when is_list(raw) do
    with {:ok, codecs} <- parse_all(raw),
         true <- Enum.any?(codecs, &(&1.name == "opus")) || {:error, :invalid_codecs},
         usable = Enum.filter(codecs, &usable?(&1, video?)),
         :ok <- payload_types_unique(usable) do
      {:ok,
       %{
         codecs: usable,
         audio_codec: "opus",
         video_codec: usable |> Enum.filter(&(&1.type == "video")) |> best_video()
       }}
    end
  end

  def negotiate(_raw, _video?), do: {:error, :invalid_codecs}

  defp parse_all(raw) do
    Enum.reduce_while(raw, {:ok, []}, fn item, {:ok, acc} ->
      case parse(item) do
        {:ok, codec} -> {:cont, {:ok, [codec | acc]}}
        :ignore -> {:cont, {:ok, acc}}
        :error -> {:halt, {:error, :invalid_codecs}}
      end
    end)
    |> case do
      {:ok, codecs} -> {:ok, Enum.reverse(codecs)}
      error -> error
    end
  end

  defp parse(%{"name" => name, "type" => type, "payload_type" => pt} = item)
       when is_binary(name) and type in ["audio", "video"] and is_integer(pt) and pt in 0..127 do
    rtx = item["rtx_payload_type"]
    priority = item["priority"]

    cond do
      not optional_payload_type?(rtx) or not optional_priority?(priority) -> :error
      not known?(name, type) -> :ignore
      true -> {:ok, codec(name, type, priority, pt, rtx)}
    end
  end

  defp parse(_item), do: :error

  defp optional_payload_type?(pt), do: is_nil(pt) or (is_integer(pt) and pt in 0..127)

  defp optional_priority?(priority),
    do: is_nil(priority) or (is_integer(priority) and priority >= 0)

  defp codec(name, type, priority, pt, rtx) do
    %{
      name: normalize(name, type),
      type: type,
      priority: priority || 0,
      payload_type: pt,
      rtx_payload_type: rtx
    }
  end

  defp known?(name, "audio"), do: String.downcase(name) == "opus"
  defp known?(name, "video"), do: String.upcase(name) in Contracts.video_codecs()

  defp normalize(name, "audio"), do: String.downcase(name)
  defp normalize(name, "video"), do: String.upcase(name)

  defp usable?(%{type: "audio"}, _video?), do: true
  defp usable?(%{type: "video"}, video?), do: video?

  defp payload_types_unique(codecs) do
    all =
      Enum.flat_map(
        codecs,
        &Enum.reject([&1.payload_type, &1.rtx_payload_type], fn pt -> is_nil(pt) end)
      )

    if length(all) == length(Enum.uniq(all)), do: :ok, else: {:error, :invalid_codecs}
  end

  defp best_video([]), do: nil
  defp best_video(videos), do: Enum.max_by(videos, & &1.priority).name
end
