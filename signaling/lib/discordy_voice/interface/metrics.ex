defmodule DiscordyVoice.Interface.Metrics do
  @moduledoc "Prometheus metrics: node gauges sampled every few seconds, plus VM and endpoint metrics."

  use Supervisor

  import Telemetry.Metrics

  alias DiscordyVoice.Infrastructure.{DrainControl, NodeLoad}

  @spec start_link(keyword()) :: Supervisor.on_start()
  def start_link(opts), do: Supervisor.start_link(__MODULE__, opts, name: __MODULE__)

  @impl true
  def init(_opts) do
    children = [
      {TelemetryMetricsPrometheus.Core, metrics: metrics(), name: :discordy_voice_metrics},
      {:telemetry_poller,
       measurements: [{__MODULE__, :measure, []}], period: 5_000, name: :discordy_voice_poller}
    ]

    Supervisor.init(children, strategy: :one_for_one)
  end

  @doc "Emits the node gauges."
  @spec measure() :: :ok
  def measure do
    :telemetry.execute([:discordy_voice, :node], %{
      users: NodeLoad.users(),
      rooms: NodeLoad.rooms(),
      egress_bps: NodeLoad.egress_bps(),
      sfu_connected: if(NodeLoad.sfu_connected?(), do: 1, else: 0),
      draining: if(DrainControl.draining?(), do: 1, else: 0)
    })
  end

  defp metrics do
    [
      last_value("discordy_voice.node.users", description: "Connected voice users."),
      last_value("discordy_voice.node.rooms", description: "Non-empty voice rooms."),
      last_value("discordy_voice.node.egress_bps", description: "SFU egress bitrate."),
      last_value("discordy_voice.node.sfu_connected",
        description: "1 when the SFU control link is up."
      ),
      last_value("discordy_voice.node.draining", description: "1 when the node is draining."),
      last_value("vm.memory.total", unit: :byte),
      last_value("vm.total_run_queue_lengths.total")
    ]
  end
end
