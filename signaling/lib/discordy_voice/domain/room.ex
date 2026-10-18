defmodule DiscordyVoice.Domain.Room do
  @moduledoc """
  Pure membership state of one voice channel on this node. A user has at most one member entry:
  joining again (a new session) evicts the previous one.
  """

  defmodule Member do
    @moduledoc "A user connected to the room."
    @enforce_keys [:user_id, :session_id, :pid, :audio_ssrc]
    defstruct [:user_id, :session_id, :pid, :audio_ssrc, video: nil]

    @type t :: %__MODULE__{
            user_id: String.t(),
            session_id: String.t(),
            pid: pid(),
            audio_ssrc: pos_integer(),
            video: map() | nil
          }
  end

  defstruct members: %{}

  @type t :: %__MODULE__{members: %{String.t() => Member.t()}}

  @spec new() :: t()
  def new, do: %__MODULE__{}

  @doc "Adds `member`. Returns the member it replaced, if the user was already present."
  @spec join(t(), Member.t()) :: {t(), Member.t() | nil}
  def join(%__MODULE__{members: members} = room, %Member{} = member) do
    evicted = Map.get(members, member.user_id)
    {%{room | members: Map.put(members, member.user_id, member)}, evicted}
  end

  @doc "Removes the member of `user_id` if its process is `pid` (a stale leave is ignored)."
  @spec leave(t(), String.t(), pid()) :: {t(), Member.t() | nil}
  def leave(%__MODULE__{members: members} = room, user_id, pid) do
    case Map.get(members, user_id) do
      %Member{pid: ^pid} = member -> {%{room | members: Map.delete(members, user_id)}, member}
      _ -> {room, nil}
    end
  end

  @doc "Removes whichever member is process `pid`."
  @spec leave_pid(t(), pid()) :: {t(), Member.t() | nil}
  def leave_pid(%__MODULE__{members: members} = room, pid) do
    case Enum.find(members, fn {_user, member} -> member.pid == pid end) do
      {user_id, member} -> {%{room | members: Map.delete(members, user_id)}, member}
      nil -> {room, nil}
    end
  end

  @doc "Records the VIDEO payload a member published."
  @spec put_video(t(), String.t(), map()) :: t()
  def put_video(%__MODULE__{members: members} = room, user_id, video) do
    case members do
      %{^user_id => member} ->
        %{room | members: Map.put(members, user_id, %{member | video: video})}

      _ ->
        room
    end
  end

  @spec empty?(t()) :: boolean()
  def empty?(%__MODULE__{members: members}), do: map_size(members) == 0

  @spec member(t(), String.t()) :: Member.t() | nil
  def member(%__MODULE__{members: members}, user_id), do: Map.get(members, user_id)

  @doc "Everyone except `user_id`."
  @spec others(t(), String.t()) :: [Member.t()]
  def others(%__MODULE__{members: members}, user_id) do
    for {id, member} <- members, id != user_id, do: member
  end

  @doc "The members among `user_ids`, excluding `except`."
  @spec select(t(), [String.t()], String.t()) :: [Member.t()]
  def select(%__MODULE__{members: members}, user_ids, except) do
    for id <- user_ids, id != except, member = Map.get(members, id), do: member
  end
end
