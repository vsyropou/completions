defmodule Complete.Repo.Migrations.CreateTelemetryEvents do
  use Ecto.Migration

  def change do
    create table(:telemetry_events) do
      add(:request_id, :text, null: false)
      add(:language, :text, null: false)
      add(:file_path, :text, null: false)
      # Nullable: non-served outcomes send "" for these as sentinels.
      add(:prompt, :text, null: true)
      add(:response, :text, null: true)
      add(:model, :text, null: true)
      add(:finish_reason, :text, null: true)
      add(:latency_ms, :integer, null: false)
      add(:provider, :text, null: false)
      add(:outcome, :text, null: false)

      add(:inserted_at, :timestamptz, null: false, default: fragment("now()"))
    end

    execute(
      """
      ALTER TABLE telemetry_events
      ADD COLUMN search tsvector
      GENERATED ALWAYS AS (
        to_tsvector('english', coalesce(prompt, '') || ' ' || coalesce(response, ''))
      ) STORED
      """,
      "ALTER TABLE telemetry_events DROP COLUMN search"
    )

    create(index(:telemetry_events, [:request_id]))
    create(index(:telemetry_events, [:inserted_at]))
    create(index(:telemetry_events, [:language]))
    create(index(:telemetry_events, [:outcome]))
    create(index(:telemetry_events, [:search], using: :gin))
  end
end
