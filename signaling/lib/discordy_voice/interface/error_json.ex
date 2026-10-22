defmodule DiscordyVoice.Interface.ErrorJSON do
  @moduledoc "JSON bodies for errors raised while serving a request (instead of a missing HTML template)."

  def render(template, _assigns),
    do: %{error: Phoenix.Controller.status_message_from_template(template)}
end
