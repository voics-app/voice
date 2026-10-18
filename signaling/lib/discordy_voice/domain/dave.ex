defmodule DiscordyVoice.Domain.Dave do
  @moduledoc """
  Routing of the DAVE opcodes (21-31, MLS media end-to-end encryption).

  The server is the MLS delivery service and external sender: it relays key packages, proposals,
  commits and welcomes between members and never sees key material (the payloads are opaque
  to it). `route/3` decides who receives what a member sent.
  """

  alias DiscordyVoice.Contracts

  @type routing ::
          {:broadcast, non_neg_integer(), map()}
          | {:to, [String.t()], non_neg_integer(), map()}
          | {:fanout, [routing()]}
          | :drop

  @doc "Whether `op` belongs to DAVE."
  @spec dave_op?(integer()) :: boolean()
  def dave_op?(op), do: op in Contracts.dave_range()

  @doc """
  Routes an opcode from `sender`. Every relayed payload carries the sender's `user_id`.
  A commit/welcome from a member is announced to everybody else, and the welcome only goes
  to `recipient_ids` when the sender named them.
  """
  @spec route(integer(), term(), String.t()) :: routing()
  def route(op, d, sender) when is_map(d) do
    commit_welcome = Contracts.op(:dave_mls_commit_welcome)

    case op do
      ^commit_welcome -> commit_welcome(d, sender)
      op when op in 21..31 -> relay(op, d, sender)
      _ -> :drop
    end
  end

  def route(_op, _d, _sender), do: :drop

  # Server-originated opcodes are never accepted from a client.
  defp relay(25, _d, _sender), do: :drop
  defp relay(29, _d, _sender), do: :drop
  defp relay(30, _d, _sender), do: :drop
  defp relay(op, d, sender), do: {:broadcast, op, Map.put(d, "user_id", sender)}

  defp commit_welcome(d, sender) do
    transition = Map.get(d, "transition_id")

    commit =
      case d do
        %{"commit" => commit} ->
          [
            {:broadcast, Contracts.op(:dave_mls_announce_commit),
             %{"user_id" => sender, "transition_id" => transition, "commit" => commit}}
          ]

        _ ->
          []
      end

    welcome =
      case d do
        %{"welcome" => welcome} ->
          payload = %{"user_id" => sender, "transition_id" => transition, "welcome" => welcome}
          op = Contracts.op(:dave_mls_welcome)

          case d["recipient_ids"] do
            ids when is_list(ids) -> [{:to, Enum.filter(ids, &is_binary/1), op, payload}]
            _ -> [{:broadcast, op, payload}]
          end

        _ ->
          []
      end

    case commit ++ welcome do
      [] -> :drop
      routes -> {:fanout, routes}
    end
  end

  @doc "The MLS external sender announcement (op 25) a new member receives when E2EE is on."
  @spec external_sender(String.t(), binary()) :: map()
  def external_sender(room_id, public_key) do
    %{
      "protocol_version" => Contracts.dave_protocol_version(),
      "external_sender" => %{
        "credential" => Base.url_encode64("discordy-voice:" <> room_id, padding: false),
        "signature_key" => Base.url_encode64(public_key, padding: false)
      }
    }
  end
end
