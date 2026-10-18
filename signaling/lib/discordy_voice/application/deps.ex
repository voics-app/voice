defmodule DiscordyVoice.Application.Deps do
  @moduledoc "The ports and settings the use cases run against, wired by the composition root."

  alias DiscordyVoice.Application.Settings

  @enforce_keys [:settings, :token_verifier, :sfu, :rooms, :ssrc, :drain]
  defstruct [:settings, :token_verifier, :sfu, :rooms, :ssrc, :drain]

  @type t :: %__MODULE__{
          settings: Settings.t(),
          token_verifier: module(),
          sfu: module(),
          rooms: module(),
          ssrc: module(),
          drain: module()
        }

  @doc "The wiring configured under `:discordy_voice, :deps` (set by the composition root)."
  @spec current() :: t()
  def current, do: Application.fetch_env!(:discordy_voice, :deps)
end
