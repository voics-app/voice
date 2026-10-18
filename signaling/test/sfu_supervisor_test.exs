defmodule DiscordyVoice.SfuSupervisorTest do
  use ExUnit.Case, async: false

  alias DiscordyVoice.Infrastructure.SfuSupervisor

  @sh System.find_executable("sh")

  @tag skip: is_nil(@sh) && "no sh on this machine"
  test "restarts the sfu process when it exits" do
    start_supervised!({SfuSupervisor, executable: @sh, args: ["-c", "sleep 0.2; exit 3"]})
    first = SfuSupervisor.os_pid()
    assert is_integer(first)

    second = wait_for_new_pid(first)
    assert second != first
  end

  defp wait_for_new_pid(old, attempts \\ 100) do
    case SfuSupervisor.os_pid() do
      pid when is_integer(pid) and pid != old -> pid
      _ when attempts == 0 -> flunk("sfu was not restarted")
      _ -> Process.sleep(50) && wait_for_new_pid(old, attempts - 1)
    end
  end
end
