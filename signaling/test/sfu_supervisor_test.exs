defmodule DiscordyVoice.SfuSupervisorTest do
  use ExUnit.Case, async: false

  alias DiscordyVoice.Infrastructure.SfuSupervisor

  @sh System.find_executable("sh")

  @tag skip: is_nil(@sh) && "no sh on this machine"
  test "restarts the sfu process when it exits" do
    start_supervised!({SfuSupervisor, executable: @sh, args: ["-c", "sleep 0.2; exit 3"]})
    first = wait_for_pid(&is_integer/1)
    second = wait_for_pid(&(is_integer(&1) and &1 != first))
    assert second != first
  end

  # The port is closed (os_pid nil) between the process exiting and the restart: poll until the
  # predicate holds instead of assuming a timing.
  defp wait_for_pid(predicate, attempts \\ 200) do
    pid = SfuSupervisor.os_pid()

    cond do
      predicate.(pid) -> pid
      attempts == 0 -> flunk("sfu os pid never satisfied the condition, last: #{inspect(pid)}")
      true -> Process.sleep(25) && wait_for_pid(predicate, attempts - 1)
    end
  end
end
