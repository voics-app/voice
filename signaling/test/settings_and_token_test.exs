defmodule DiscordyVoice.SettingsAndTokenTest do
  use ExUnit.Case, async: true

  alias DiscordyVoice.Application.Settings
  alias DiscordyVoice.Infrastructure.Hs256TokenVerifier

  @env %{
    "VOICE_REGION" => "nl-home",
    "VOICE_NODE_ID" => "node-1",
    "VOICE_PUBLIC_WS_URL" => "wss://rtc.example.test/",
    "VOICE_PUBLIC_IP" => "203.0.113.7",
    "ETCD_ENDPOINTS" => "http://etcd-a:2379, etcd-b:2379",
    "VOICE_TOKEN_SECRET" => "0123456789abcdef0123",
    "INTERNAL_SERVICE_TOKEN" => "internal"
  }

  describe "Settings.from_env/1" do
    test "applies defaults and normalizes values" do
      assert {:ok, s} = Settings.from_env(@env)
      assert s.udp_port == 50_000
      assert s.http_port == 4100
      assert s.sfu_control == {"127.0.0.1", 4101}
      assert s.public_ws_url == "wss://rtc.example.test"
      assert s.etcd_endpoints == ["etcd-a:2379", "etcd-b:2379"]
      assert s.env == :local
      assert s.log_level == :info
      refute s.drain_on_start
      assert s.sfu_executable == nil
    end

    test "reads optional values" do
      env =
        Map.merge(@env, %{
          "VOICE_DRAIN_ON_START" => "true",
          "SFU_EXECUTABLE" => "/bin/sfu",
          "DISCORDY_ENV" => "production",
          "LOG_LEVEL" => "debug",
          "SFU_CONTROL_ADDR" => "sfu:9"
        })

      assert {:ok, s} = Settings.from_env(env)
      assert s.drain_on_start and s.env == :production and s.log_level == :debug
      assert s.sfu_executable == "/bin/sfu" and s.sfu_control == {"sfu", 9}
    end

    test "control address accepts only private IPv4" do
      assert {:ok, settings} = Settings.from_env(Map.put(@env, "VOICE_CONTROL_IP", "10.42.0.8"))
      assert settings.control_ip == "10.42.0.8"
      assert {:error, errors} = Settings.from_env(Map.put(@env, "VOICE_CONTROL_IP", "203.0.113.1"))
      assert Enum.any?(errors, &String.contains?(&1, "VOICE_CONTROL_IP"))
      production = Map.merge(@env, %{"DISCORDY_ENV" => "production", "VOICE_CONTROL_IP" => "127.0.0.1"})
      assert {:error, _} = Settings.from_env(production)
    end

    test "reports every problem at once" do
      assert {:error, errors} =
               Settings.from_env(%{
                 "VOICE_PUBLIC_IP" => "nope",
                 "VOICE_TOKEN_SECRET" => "short",
                 "VOICE_UDP_PORT" => "x"
               })

      text = Enum.join(errors, "\n")
      assert text =~ "VOICE_REGION is required"
      assert text =~ "VOICE_NODE_ID is required"
      assert text =~ "VOICE_PUBLIC_IP must be an IP"
      assert text =~ "VOICE_TOKEN_SECRET must be at least 16 bytes"
      assert text =~ "VOICE_UDP_PORT must be a port"
      assert text =~ "ETCD_ENDPOINTS is required"
      assert text =~ "INTERNAL_SERVICE_TOKEN is required"
    end

    test "rejects malformed urls, endpoints and levels" do
      bad =
        Map.merge(@env, %{
          "VOICE_PUBLIC_WS_URL" => "https://x",
          "ETCD_ENDPOINTS" => "nohostport",
          "LOG_LEVEL" => "loud",
          "DISCORDY_ENV" => "staging",
          "SFU_CONTROL_ADDR" => "x"
        })

      assert {:error, errors} = Settings.from_env(bad)
      assert length(errors) == 5
    end
  end

  describe "Hs256TokenVerifier" do
    @secret "0123456789abcdef0123"
    @claims %{
      "sub" => "u",
      "sid" => "s",
      "srv" => "g",
      "chn" => "c",
      "node" => "n",
      "e2ee" => false,
      "exp" => 2_000
    }

    test "accepts a valid token" do
      token = Hs256TokenVerifier.sign(@claims, @secret)
      assert {:ok, @claims} = Hs256TokenVerifier.verify(token, @secret, 1_000)
    end

    test "rejects expired tokens" do
      token = Hs256TokenVerifier.sign(@claims, @secret)
      assert {:error, :expired} = Hs256TokenVerifier.verify(token, @secret, 2_000)
    end

    test "rejects a wrong secret and a tampered payload" do
      token = Hs256TokenVerifier.sign(@claims, @secret)

      assert {:error, :bad_signature} =
               Hs256TokenVerifier.verify(token, "another-secret-another", 1_000)

      [h, _p, sig] = String.split(token, ".")
      forged = Base.url_encode64(Jason.encode!(%{@claims | "sub" => "admin"}), padding: false)

      assert {:error, :bad_signature} =
               Hs256TokenVerifier.verify(Enum.join([h, forged, sig], "."), @secret, 1_000)
    end

    test "rejects alg none and other algorithms" do
      enc = &Base.url_encode64(Jason.encode!(&1), padding: false)
      none = enc.(%{"alg" => "none"}) <> "." <> enc.(@claims) <> "."
      assert {:error, :unsupported_algorithm} = Hs256TokenVerifier.verify(none, @secret, 1_000)

      hs512 = enc.(%{"alg" => "HS512"}) <> "." <> enc.(@claims) <> ".AAAA"
      assert {:error, :unsupported_algorithm} = Hs256TokenVerifier.verify(hs512, @secret, 1_000)
    end

    test "rejects missing claims and garbage" do
      token = Hs256TokenVerifier.sign(Map.delete(@claims, "chn"), @secret)
      assert {:error, :missing_claims} = Hs256TokenVerifier.verify(token, @secret, 1_000)
      assert {:error, :malformed} = Hs256TokenVerifier.verify("garbage", @secret, 1_000)
      assert {:error, :malformed} = Hs256TokenVerifier.verify("a.b.c", @secret, 1_000)
    end
  end
end
