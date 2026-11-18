defmodule DiscordyVoice.CompositionRoot do
  @moduledoc """
  Application entry point: validates configuration, wires the ports to their adapters and starts
  the supervision tree. With `config :discordy_voice, edge: false` (tests) only the in-process
  core is started and the edge services (SFU link, etcd, HTTP) are started by the test itself.
  """

  use Application

  alias DiscordyVoice.Application.{Deps, Settings}
  alias DiscordyVoice.{Infrastructure, Interface}
  alias LoggerJSON.Formatters.Basic

  require Logger

  @impl true
  def start(_type, _args) do
    Infrastructure.NodeLoad.init()
    Infrastructure.SsrcCounter.init()

    children =
      if Application.get_env(:discordy_voice, :edge, true) do
        settings = load_settings!()
        core_children(settings.drain_on_start) ++ edge_children(settings)
      else
        core_children(false)
      end

    Supervisor.start_link(children, strategy: :one_for_one, name: DiscordyVoice.Supervisor)
  end

  defp core_children(draining) do
    [
      {Registry, keys: :unique, name: DiscordyVoice.RoomRegistry},
      {Registry, keys: :duplicate, name: DiscordyVoice.SessionRegistry},
      {DynamicSupervisor, strategy: :one_for_one, name: DiscordyVoice.RoomSupervisor},
      {Infrastructure.DrainControl, draining: draining}
    ]
  end

  defp edge_children(settings) do
    Application.put_env(:discordy_voice, :deps, deps(settings))
    Application.put_env(:logger, :level, settings.log_level)

    :logger.update_handler_config(
      :default,
      :formatter,
      Basic.new(metadata: [:request_id])
    )

    Application.put_env(:discordy_voice, Interface.Endpoint,
      adapter: Bandit.PhoenixAdapter,
      http: [ip: {0, 0, 0, 0}, port: settings.http_port],
      server: true
    )

    if settings.drain_on_start, do: Logger.info("starting in drain mode")

    sfu =
      if settings.sfu_executable,
        do: [{Infrastructure.SfuSupervisor, executable: settings.sfu_executable}],
        else: []

    sfu ++
      [
        {Infrastructure.SfuClient, address: settings.sfu_control},
        {Infrastructure.NodePresence,
         settings: settings, registry: Infrastructure.EtcdNodeRegistry},
        Interface.Metrics,
        Interface.Endpoint
      ]
  end

  defp deps(settings) do
    %Deps{
      settings: settings,
      token_verifier: Infrastructure.Hs256TokenVerifier,
      sfu: Infrastructure.SfuClient,
      rooms: Infrastructure.Rooms,
      ssrc: Infrastructure.SsrcCounter,
      drain: Infrastructure.DrainControl,
      registration: Infrastructure.NodePresence
    }
  end

  defp load_settings! do
    version = to_string(Application.spec(:discordy_voice, :vsn))

    case Settings.from_env(System.get_env()) do
      {:ok, settings} ->
        %{settings | version: version}

      {:error, errors} ->
        Enum.each(errors, &IO.puts(:stderr, "invalid configuration: #{&1}"))
        System.halt(2)
    end
  end
end
