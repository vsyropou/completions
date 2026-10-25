defmodule Complete.TelemetryEvent do
  @moduledoc "Persisted telemetry event posted by the sidecar."

  use Ecto.Schema
  import Ecto.Changeset

  @outcomes ~w(requested served superseded accepted cancelled error)
  @fields ~w(request_id language file_path prompt response latency_ms provider model finish_reason outcome)a

  schema "telemetry_events" do
    field(:request_id, :string)
    field(:language, :string)
    field(:file_path, :string)
    field(:prompt, :string)
    field(:response, :string)
    field(:latency_ms, :integer)
    field(:provider, :string)
    field(:model, :string)
    field(:finish_reason, :string)
    field(:outcome, :string)

    field(:inserted_at, :utc_datetime_usec)
  end

  # Wire contract: all fields present. `""` is a valid "not applicable"
  # sentinel for prompt/response/model/finish_reason on non-served
  # outcomes — disable Ecto's ""-to-nil coercion per-field so `""`
  # survives to DB.
  def changeset(event, attrs) do
    cs =
      event
      |> cast(attrs, @fields, empty_values: [])
      |> require_present(@fields)
      |> validate_number(:latency_ms, greater_than_or_equal_to: 0)
      |> validate_inclusion(:outcome, @outcomes)

    cs
  end

  # All @fields must be present; nil (unlike "") is a contract violation.
  defp require_present(changeset, fields) do
    Enum.reduce(fields, changeset, fn field, acc ->
      if is_nil(get_field(acc, field)),
        do: add_error(acc, field, "can't be nil", validation: :required),
        else: acc
    end)
  end
end
