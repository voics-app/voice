defmodule DiscordyVoice.Infrastructure.SfuSupervisor do
  @moduledoc """
  Runs the SFU as an OS process owned by this node (`SFU_EXECUTABLE`) and restarts it with
  exponential back-off when it exits. `SfuClient` reconnects and replays the live sessions, so a
  crashed SFU costs a few seconds of media rather than the call. Without `SFU_EXECUTABLE` (the
  SFU runs as a sibling container) this process is not started.
  """

  use GenServer

  require Logger

  @min_backoff 500
  @max_backoff 10_000
  @stable_after 30_000

  @spec start_link(keyword()) :: GenServer.on_start()
  def start_link(opts),
    do: GenServer.start_link(__MODULE__, opts, name: Keyword.get(opts, :name, __MODULE__))

  @doc "OS pid of the running SFU, if any."
  @spec os_pid(GenServer.server()) :: non_neg_integer() | nil
  def os_pid(server \\ __MODULE__), do: GenServer.call(server, :os_pid)

  @impl true
  def init(opts) do
    Process.flag(:trap_exit, true)

    state = %{
      executable: Keyword.fetch!(opts, :executable),
      args: Keyword.get(opts, :args, []),
      port: nil,
      started_at: nil,
      backoff: @min_backoff
    }

    {:ok, start(state)}
  end

  @impl true
  def handle_call(:os_pid, _from, %{port: nil} = state), do: {:reply, nil, state}

  def handle_call(:os_pid, _from, %{port: port} = state) do
    # The port closes as soon as the process exits, slightly before its exit_status message is
    # handled: `Port.info/2` is `nil` in that window.
    os_pid =
      case Port.info(port, :os_pid) do
        {:os_pid, os_pid} -> os_pid
        nil -> nil
      end

    {:reply, os_pid, state}
  end

  @impl true
  def handle_info({port, {:data, {_eol, line}}}, %{port: port} = state) do
    Logger.info("sfu: #{line}")
    {:noreply, state}
  end

  def handle_info({port, {:exit_status, status}}, %{port: port} = state) do
    lived = System.monotonic_time(:millisecond) - state.started_at

    backoff =
      if lived >= @stable_after, do: @min_backoff, else: min(state.backoff * 2, @max_backoff)

    Logger.error("sfu exited with status #{status}, restarting in #{state.backoff} ms")
    Process.send_after(self(), :restart, state.backoff)
    {:noreply, %{state | port: nil, backoff: backoff}}
  end

  def handle_info(:restart, state), do: {:noreply, start(state)}
  def handle_info(_other, state), do: {:noreply, state}

  @impl true
  def terminate(_reason, %{port: port}) when is_port(port) do
    Port.close(port)
    :ok
  rescue
    ArgumentError -> :ok
  end

  def terminate(_reason, _state), do: :ok

  defp start(state) do
    port =
      Port.open({:spawn_executable, state.executable}, [
        :binary,
        :exit_status,
        :stderr_to_stdout,
        {:line, 4_096},
        {:args, state.args}
      ])

    Logger.info("sfu started: #{state.executable}")
    %{state | port: port, started_at: System.monotonic_time(:millisecond)}
  end
end
