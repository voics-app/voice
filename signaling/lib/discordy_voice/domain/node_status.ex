defmodule DiscordyVoice.Domain.NodeStatus do
  @moduledoc "The value a voice node writes to etcd (P8) and its JSON form."

  @enforce_keys [:node_id, :region, :ws_url, :public_ip, :udp_port, :version]
  defstruct [
    :node_id,
    :region,
    :ws_url,
    :public_ip,
    :udp_port,
    :version,
    :control_ip,
    :control_port,
    :registration_lease,
    users: 0,
    cpu_pct: 0.0,
    egress_bps: 0,
    draining: false,
    probe_required: false
  ]

  @type t :: %__MODULE__{}

  @doc "Encodes the P8 value `{node_id, region, ws_url, public_ip, udp_port, users, cpu_pct, egress_bps, draining, version}`."
  @spec to_json(t()) :: binary()
  def to_json(%__MODULE__{} = status) do
    fields = %{
      node_id: status.node_id,
      region: status.region,
      ws_url: status.ws_url,
      public_ip: status.public_ip,
      udp_port: status.udp_port,
      users: status.users,
      cpu_pct: Float.round(status.cpu_pct * 1.0, 1),
      egress_bps: status.egress_bps,
      draining: status.draining,
      probe_required: status.probe_required,
      version: status.version
    }

    fields =
      if status.control_ip do
        Map.merge(fields, %{
          control_ip: status.control_ip,
          control_port: status.control_port,
          registration_lease: status.registration_lease
        })
      else
        fields
      end

    Jason.encode!(fields)
  end
end
