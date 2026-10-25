defmodule Complete.TelemetryEvent do
  @moduledoc """
  Persisted telemetry event posted by the sidecar.
  """
  use Ecto.Schema
  import Ecto.Changeset

  @outcomes ~w(requested served superseded accepted cancelled error)

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

  # Non-served outcomes (requested/superseded/accepted/cancelled/error) send
  # `""` for prompt/response/model/finish_reason. Those columns are nullable
  # Postgres-side (migration AllowEmptyOutcomeSentinels) and presence-as-binary
  # is already enforced at the controller; the changeset keeps only outcome.
  def changeset(event, attrs) do
    # `empty_strings_to_null` is a schema-level default in Ecto 3.13+: we
    # disable it module-locally (see @empty_strings_to_null above) so `""`
    # survives `cast` -> DB. `validate_required([])` is kept minimal on
    # purpose: `""` for prompt/response/model/finish_reason is a valid
    # "not applicable" value for non-served outcomes, and presence-as-binary
    # is already enforced in the controller upstream.
    event
    |> cast(attrs, [
      :request_id,
      :language,
      :file_path,
      :prompt,
      :response,
      :latency_ms,
      :provider,
      :model,
      :finish_reason,
      :outcome
    ])
    |> validate_required([:request_id, :outcome])
    |> validate_inclusion(:outcome, @outcomes)
  end
end
