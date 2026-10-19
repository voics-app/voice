defmodule DiscordyVoice.MixProject do
  use Mix.Project

  @version "0.1.0"

  def project do
    [
      app: :discordy_voice,
      version: @version,
      elixir: "~> 1.18",
      elixirc_paths: elixirc_paths(Mix.env()),
      start_permanent: Mix.env() == :prod,
      deps: deps(),
      aliases: aliases(),
      releases: releases(),
      test_coverage: [summary: [threshold: 0]]
    ]
  end

  def application do
    [
      mod: {DiscordyVoice.CompositionRoot, []},
      extra_applications: [:logger, :runtime_tools, :crypto, :os_mon]
    ]
  end

  def cli do
    [preferred_envs: [precommit: :test]]
  end

  defp elixirc_paths(:test), do: ["lib", "test/support"]
  defp elixirc_paths(_), do: ["lib"]

  defp deps do
    [
      {:phoenix, "~> 1.8.1"},
      {:bandit, "~> 1.8"},
      {:websock_adapter, "~> 0.5"},
      {:jason, "~> 1.4"},
      {:eetcd, "~> 0.6"},
      {:logger_json, "~> 7.0"},
      {:telemetry_metrics, "~> 1.1"},
      {:telemetry_poller, "~> 1.3"},
      {:telemetry_metrics_prometheus_core, "~> 1.2"},
      {:opentelemetry, "~> 1.5"},
      {:opentelemetry_api, "~> 1.4"},
      {:opentelemetry_exporter, "~> 1.8"},
      {:gun, "~> 2.7", override: true},
      {:cowlib, "~> 2.21", override: true},
      {:credo, "~> 1.7", only: [:dev, :test], runtime: false}
    ]
  end

  defp aliases do
    [
      lint: ["format --check-formatted", "credo --strict"],
      precommit: ["compile --warnings-as-errors", "format", "credo --strict", "test"]
    ]
  end

  defp releases do
    [
      discordy_voice: [
        include_executables_for: [:unix],
        applications: [opentelemetry: :temporary]
      ]
    ]
  end
end
