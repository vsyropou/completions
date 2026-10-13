defmodule Complete.MixProject do
  use Mix.Project

  def project do
    [
      app: :complete,
      version: "0.1.0",
      elixir: "~> 1.18",
      elixirc_paths: elixirc_paths(Mix.env()),
      start_permanent: Mix.env() == :prod,
      aliases: aliases(),
      deps: deps()
    ]
  end

  # Configuration for the OTP application.
  #
  # Type `mix help compile.app` for more information.
  def application do
    [
      mod: {Complete.Application, []},
      extra_applications: [:logger, :runtime_tools]
    ]
  end

  def cli do
    [preferred_envs: []]
  end

  # Specifies which paths to compile per environment.
  defp elixirc_paths(:test), do: ["lib", "test/support"]
  defp elixirc_paths(_), do: ["lib"]

  # Specifies your project dependencies.
  #
  # Type `mix help deps` for examples and options.
  defp deps do
    [
      {:phoenix, "~> 1.7"},
      {:bandit, "~> 1.5"},
      {:jason, "~> 1.4"},
      {:finch, "~> 0.19"}
    ]
  end

  defp aliases do
    [setup: ["deps.get"]]
  end
end
