defmodule DiscordyVoice.Contracts do
  @moduledoc """
  Constants and wire shapes pinned by interface P8 (and P3 for the voice token).

  Everything the generated `contracts` package will eventually provide lives in this one module
  so it can be swapped without touching the rest of the service.
  """

  # -- Voice WebSocket opcodes (P8) ---------------------------------------------------------

  @doc "Opcode numbers of the voice WebSocket protocol."
  @spec op(atom()) :: non_neg_integer()
  def op(:identify), do: 0
  def op(:select_protocol), do: 1
  def op(:ready), do: 2
  def op(:heartbeat), do: 3
  def op(:session_description), do: 4
  def op(:speaking), do: 5
  def op(:heartbeat_ack), do: 6
  def op(:hello), do: 8
  def op(:clients_connect), do: 11
  def op(:video), do: 12
  def op(:client_disconnect), do: 13
  def op(:media_sink_wants), do: 15
  def op(:stream_viewers), do: 40
  def op(:dave_prepare_transition), do: 21
  def op(:dave_execute_transition), do: 22
  def op(:dave_mls_external_sender), do: 25
  def op(:dave_mls_key_package), do: 26
  def op(:dave_mls_proposals), do: 27
  def op(:dave_mls_commit_welcome), do: 28
  def op(:dave_mls_announce_commit), do: 29
  def op(:dave_mls_welcome), do: 30
  def op(:dave_mls_invalid_commit), do: 31

  @doc "First and last DAVE opcode (MLS media end-to-end encryption)."
  @spec dave_range() :: Range.t()
  def dave_range, do: 21..31

  @doc "HELLO's `heartbeat_interval`, milliseconds."
  @spec heartbeat_interval_ms() :: pos_integer()
  def heartbeat_interval_ms, do: 13_750

  @doc "Missed-heartbeat budget: the socket closes after this many intervals of silence."
  @spec heartbeat_grace_factor() :: float()
  def heartbeat_grace_factor, do: 2.5

  @doc "Time a fresh connection has to send IDENTIFY, milliseconds."
  @spec identify_timeout_ms() :: pos_integer()
  def identify_timeout_ms, do: 10_000

  @doc "DAVE protocol version announced in SESSION_DESCRIPTION."
  @spec dave_protocol_version() :: pos_integer()
  def dave_protocol_version, do: 1

  # -- WebSocket close codes ----------------------------------------------------------------

  @doc "Application close codes sent before the socket shuts down."
  @spec close_code(atom()) :: pos_integer()
  def close_code(:not_authenticated), do: 4003
  def close_code(:authentication_failed), do: 4004
  def close_code(:already_authenticated), do: 4005
  def close_code(:session_timeout), do: 4009
  def close_code(:server_draining), do: 4011
  def close_code(:invalid_payload), do: 4012
  def close_code(:disconnected), do: 4014
  def close_code(:wrong_node), do: 4015
  def close_code(:sfu_unavailable), do: 4016

  # -- Media negotiation --------------------------------------------------------------------

  @doc "Transport modes offered in READY."
  @spec modes() :: [String.t()]
  def modes, do: ["dtls-srtp"]

  @doc "Simulcast layers offered to video publishers, ascending quality."
  @spec video_layers() :: [%{rid: String.t(), quality: 0..100}]
  def video_layers do
    [%{rid: "q", quality: 25}, %{rid: "h", quality: 50}, %{rid: "f", quality: 100}]
  end

  @doc "Video codecs the SFU can forward."
  @spec video_codecs() :: [String.t()]
  def video_codecs, do: ["AV1", "VP9", "H264", "VP8"]

  # -- Speaking bitmask ---------------------------------------------------------------------

  @doc "SPEAKING flag bits."
  @spec speaking_flag(atom()) :: pos_integer()
  def speaking_flag(:microphone), do: 1
  def speaking_flag(:soundshare), do: 2
  def speaking_flag(:priority), do: 4

  # -- Discovery (P8) -----------------------------------------------------------------------

  @doc "etcd key a node registers itself under."
  @spec node_key(String.t(), String.t()) :: String.t()
  def node_key(region, node_id), do: "/discordy/voice/nodes/#{region}/#{node_id}"

  @doc "etcd key whose presence asks a node to drain (written by the gateway, see its `VoiceNodeControl`)."
  @spec drain_key(String.t()) :: String.t()
  def drain_key(node_id), do: "/discordy/voice/drain/#{node_id}"

  @doc "Lease lifetime, seconds."
  @spec lease_ttl_s() :: pos_integer()
  def lease_ttl_s, do: 10

  @doc "Lease refresh and load report cadence, milliseconds."
  @spec refresh_interval_ms() :: pos_integer()
  def refresh_interval_ms, do: 3_000

  # -- Voice token (P3) ---------------------------------------------------------------------

  @doc "Claims every voice token must carry."
  @spec token_claims() :: [String.t()]
  def token_claims, do: ["sub", "sid", "srv", "chn", "node", "e2ee", "exp"]
end
