defmodule DiscordyVoice.Application.Settings do
  @moduledoc """
  Service configuration from environment variables (P2), validated once at startup.

  `from_env/1` is a pure function over a map of variables so it is unit-testable; the composition
  root feeds it `System.get_env/0` and refuses to boot when it returns errors.
  """

  @enforce_keys [
    :env,
    :region,
    :node_id,
    :public_ws_url,
    :public_ip,
    :udp_port,
    :etcd_endpoints,
    :token_secret,
    :internal_token,
    :sfu_control,
    :http_port
  ]
  defstruct [
    :env,
    :region,
    :node_id,
    :public_ws_url,
    :public_ip,
    :udp_port,
    :etcd_endpoints,
    :token_secret,
    :internal_token,
    :sfu_control,
    :http_port,
    lan_ip: nil,
    sfu_executable: nil,
    drain_on_start: false,
    log_level: :info,
    version: "0.0.0"
  ]

  @type t :: %__MODULE__{
          env: :local | :production,
          region: String.t(),
          node_id: String.t(),
          public_ws_url: String.t(),
          public_ip: String.t(),
          udp_port: 1..65_535,
          etcd_endpoints: [String.t()],
          token_secret: String.t(),
          internal_token: String.t(),
          sfu_control: {String.t(), 1..65_535},
          http_port: 0..65_535,
          lan_ip: String.t() | nil,
          sfu_executable: String.t() | nil,
          drain_on_start: boolean(),
          log_level: Logger.level(),
          version: String.t()
        }

  @levels ~w(debug info notice warning error)

  @doc "Builds settings, or returns every problem found."
  @spec from_env(%{optional(String.t()) => String.t()}) :: {:ok, t()} | {:error, [String.t()]}
  def from_env(vars) do
    checks = [
      {:env, env(vars)},
      {:region, required(vars, "VOICE_REGION")},
      {:node_id, required(vars, "VOICE_NODE_ID")},
      {:public_ws_url, ws_url(vars)},
      {:public_ip, ip(vars)},
      {:lan_ip, optional_ip(vars, "VOICE_LAN_IP")},
      {:udp_port, port(vars, "VOICE_UDP_PORT", "50000")},
      {:etcd_endpoints, endpoints(vars)},
      {:token_secret, secret(vars, "VOICE_TOKEN_SECRET")},
      {:internal_token, required(vars, "INTERNAL_SERVICE_TOKEN")},
      {:sfu_control, host_port(vars, "SFU_CONTROL_ADDR", "127.0.0.1:4101")},
      {:http_port, port(vars, "PORT", "4100")},
      {:sfu_executable, {:ok, blank_to_nil(vars["SFU_EXECUTABLE"])}},
      {:drain_on_start, {:ok, vars["VOICE_DRAIN_ON_START"] in ["1", "true"]}},
      {:log_level, log_level(vars)}
    ]

    case Enum.flat_map(checks, fn {_key, result} -> List.wrap(error_of(result)) end) do
      [] ->
        {:ok, struct!(__MODULE__, Enum.map(checks, fn {key, {:ok, value}} -> {key, value} end))}

      errors ->
        {:error, errors}
    end
  end

  defp error_of({:error, message}), do: message
  defp error_of({:ok, _}), do: nil

  defp blank_to_nil(nil), do: nil
  defp blank_to_nil(""), do: nil
  defp blank_to_nil(value), do: value

  defp required(vars, name) do
    case vars[name] do
      value when value in [nil, ""] -> {:error, "#{name} is required"}
      value -> {:ok, value}
    end
  end

  defp secret(vars, name) do
    with {:ok, value} <- required(vars, name) do
      if byte_size(value) >= 16,
        do: {:ok, value},
        else: {:error, "#{name} must be at least 16 bytes"}
    end
  end

  defp env(vars) do
    case vars["DISCORDY_ENV"] do
      "production" -> {:ok, :production}
      value when value in [nil, "", "local"] -> {:ok, :local}
      other -> {:error, "DISCORDY_ENV must be local or production, got #{other}"}
    end
  end

  defp ws_url(vars) do
    with {:ok, value} <- required(vars, "VOICE_PUBLIC_WS_URL") do
      case URI.parse(value) do
        %URI{scheme: scheme, host: host} when scheme in ["ws", "wss"] and is_binary(host) ->
          {:ok, String.trim_trailing(value, "/")}

        _ ->
          {:error, "VOICE_PUBLIC_WS_URL must be a ws:// or wss:// URL"}
      end
    end
  end

  defp ip(vars) do
    with {:ok, value} <- required(vars, "VOICE_PUBLIC_IP") do
      case :inet.parse_address(String.to_charlist(value)) do
        {:ok, _} -> {:ok, value}
        {:error, _} -> {:error, "VOICE_PUBLIC_IP must be an IP address"}
      end
    end
  end

  # Optional extra ICE host candidate: the node's LAN address, for clients behind the same NAT
  # (a router rarely hairpins UDP to its own public IP).
  defp optional_ip(vars, name) do
    case vars[name] do
      value when value in [nil, ""] ->
        {:ok, nil}

      value ->
        case :inet.parse_address(String.to_charlist(value)) do
          {:ok, _} -> {:ok, value}
          {:error, _} -> {:error, "#{name} must be an IP address"}
        end
    end
  end

  @doc "Distinct ICE host candidate IPs the SFU is reachable on: LAN first, then public."
  @spec candidate_ips(t()) :: [String.t()]
  def candidate_ips(%__MODULE__{lan_ip: lan, public_ip: public}),
    do: Enum.uniq(Enum.reject([lan, public], &is_nil/1))

  defp port(vars, name, default) do
    value = non_blank(vars[name], default)

    case Integer.parse(value) do
      {port, ""} when port in 0..65_535 -> {:ok, port}
      _ -> {:error, "#{name} must be a port number"}
    end
  end

  defp host_port(vars, name, default) do
    value = non_blank(vars[name], default)

    with [host, port] <- String.split(value, ":"),
         true <- host != "",
         {port, ""} when port in 1..65_535 <- Integer.parse(port) do
      {:ok, {host, port}}
    else
      _ -> {:error, "#{name} must be host:port"}
    end
  end

  defp endpoints(vars) do
    with {:ok, value} <- required(vars, "ETCD_ENDPOINTS") do
      endpoints =
        value
        |> String.split(",", trim: true)
        |> Enum.map(
          &(&1
            |> String.trim()
            |> String.replace(~r{^https?://}, "")
            |> String.trim_trailing("/"))
        )

      if Enum.all?(endpoints, &Regex.match?(~r/^[^:\s]+:\d+$/, &1)) and endpoints != [] do
        {:ok, endpoints}
      else
        {:error, "ETCD_ENDPOINTS must be a comma-separated list of host:port"}
      end
    end
  end

  defp log_level(vars) do
    level = non_blank(vars["LOG_LEVEL"], "info")

    if level in @levels,
      do: {:ok, String.to_existing_atom(level)},
      else: {:error, "LOG_LEVEL must be one of #{Enum.join(@levels, ", ")}"}
  end

  defp non_blank(value, default) when value in [nil, ""], do: default
  defp non_blank(value, _default), do: value
end
