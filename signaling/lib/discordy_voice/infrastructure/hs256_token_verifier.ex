defmodule DiscordyVoice.Infrastructure.Hs256TokenVerifier do
  @moduledoc """
  Verifies the gateway-minted voice token (P3): a JWT signed with HS256 under `VOICE_TOKEN_SECRET`.
  Only HS256 is accepted (never `none`), the signature is compared in constant time, and the
  token must carry every claim in `Contracts.token_claims/0` and be unexpired.
  """

  @behaviour DiscordyVoice.Application.Ports.TokenVerifier

  alias DiscordyVoice.Application.Deps
  alias DiscordyVoice.Contracts

  @impl true
  def verify(token) when is_binary(token) do
    settings = Deps.current().settings
    verify(token, settings.token_secret, settings.probe_token_secret, System.os_time(:second))
  end

  @doc "Verifies `token` against `secret` at unix time `now_s`."
  @spec verify(String.t(), String.t(), integer()) :: {:ok, map()} | {:error, atom()}
  def verify(token, secret, now_s), do: verify(token, secret, nil, now_s)

  @doc "Selects the signing key by purpose, then verifies the signature and scoped claims."
  @spec verify(String.t(), String.t(), String.t() | nil, integer()) ::
          {:ok, map()} | {:error, atom()}
  def verify(token, call_secret, probe_secret, now_s) do
    with [header_b64, payload_b64, signature_b64] <- String.split(token, "."),
         {:ok, header} <- decode_json(header_b64),
         :ok <- check_alg(header),
         {:ok, claims} <- decode_json(payload_b64),
         {:ok, secret} <- signing_key(claims, call_secret, probe_secret),
         :ok <- check_signature(header_b64 <> "." <> payload_b64, signature_b64, secret),
         :ok <- check_claims(claims),
         :ok <- check_expiry(claims, now_s),
         :ok <- check_scope(claims, now_s) do
      {:ok, claims}
    else
      {:error, reason} -> {:error, reason}
      _ -> {:error, :malformed}
    end
  end

  defp signing_key(%{"purpose" => "probe"}, _call, nil), do: {:error, :probes_disabled}
  defp signing_key(%{"purpose" => "probe"}, _call, probe), do: {:ok, probe}
  defp signing_key(%{"purpose" => "call"}, call, _probe), do: {:ok, call}
  defp signing_key(%{"purpose" => _other}, _call, _probe), do: {:error, :invalid_purpose}
  defp signing_key(_claims, call, _probe), do: {:ok, call}

  defp check_scope(%{"purpose" => "probe"} = claims, now_s) do
    valid =
      Enum.all?(~w(sub srv chn node_lease), &decimal?(claims[&1])) and
        Enum.all?(~w(sid node), &(is_binary(claims[&1]) and claims[&1] != "")) and
        uuid?(claims["probe_id"]) and claims["e2ee"] == false and
        is_integer(claims["iat"]) and claims["iat"] <= now_s and
        claims["exp"] <= claims["iat"] + 60

    if valid, do: :ok, else: {:error, :invalid_probe_claims}
  end

  defp check_scope(_claims, _now_s), do: :ok
  defp decimal?(value) when is_binary(value), do: Regex.match?(~r/^[1-9][0-9]*$/, value)
  defp decimal?(_value), do: false

  defp uuid?(value) when is_binary(value),
    do: Regex.match?(~r/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i, value)

  defp uuid?(_value), do: false

  @doc "Signs `claims` as a voice token. Used by the gateway's twin implementation and by tests."
  @spec sign(map(), String.t()) :: String.t()
  def sign(claims, secret) do
    header = encode_json(%{"alg" => "HS256", "typ" => "JWT"})
    payload = encode_json(claims)
    signing_input = header <> "." <> payload
    signing_input <> "." <> Base.url_encode64(mac(signing_input, secret), padding: false)
  end

  defp mac(data, secret), do: :crypto.mac(:hmac, :sha256, secret, data)

  defp encode_json(map), do: map |> Jason.encode!() |> Base.url_encode64(padding: false)

  defp decode_json(segment) do
    with {:ok, json} <- Base.url_decode64(segment, padding: false),
         {:ok, %{} = map} <- Jason.decode(json) do
      {:ok, map}
    else
      _ -> {:error, :malformed}
    end
  end

  defp check_alg(%{"alg" => "HS256"}), do: :ok
  defp check_alg(_header), do: {:error, :unsupported_algorithm}

  defp check_signature(signing_input, signature_b64, secret) do
    with {:ok, signature} <- Base.url_decode64(signature_b64, padding: false),
         true <- byte_size(signature) == 32,
         true <- :crypto.hash_equals(signature, mac(signing_input, secret)) do
      :ok
    else
      _ -> {:error, :bad_signature}
    end
  end

  defp check_claims(claims) do
    if Enum.all?(Contracts.token_claims(), &Map.has_key?(claims, &1)),
      do: :ok,
      else: {:error, :missing_claims}
  end

  defp check_expiry(%{"exp" => exp}, now_s) when is_integer(exp) and exp > now_s, do: :ok
  defp check_expiry(_claims, _now_s), do: {:error, :expired}
end
